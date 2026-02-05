use crate::direct_forwarder::DirectForwarder;
use crate::forwarder::Forwarder;
use crate::http1_codec::Http1Codec;
use crate::http2_codec::Http2Codec;
use crate::http3_codec::Http3Codec;
use crate::http_codec::HttpCodec;
use crate::http_downstream::HttpDownstream;
use crate::icmp_forwarder::IcmpForwarder;
use crate::metrics::Metrics;
use crate::net_utils::PeerAddr;
use crate::quic_multiplexer::{QuicMultiplexer, QuicSocket};
use crate::settings::{ForwardProtocolSettings, Settings};
use crate::shutdown::Shutdown;
use crate::socks5_forwarder::Socks5Forwarder;
use crate::tls_demultiplexer::TlsDemux;
use crate::tls_listener::{TlsAcceptor, TlsListener};
use crate::tunnel::Tunnel;
use crate::{
    authentication, http_deny_handler, http_ping_handler, http_speedtest_handler, log_id,
    log_utils, metrics, net_utils, reverse_proxy, rules, settings, tls_demultiplexer, tunnel,
};
use log::warn;
use socket2::SockRef;
use std::io;
use std::io::ErrorKind;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::watch;

#[derive(Debug)]
pub enum Error {
    /// Passed settings did not pass the validation
    SettingsValidation(settings::ValidationError),
    /// TLS demultiplexer initialization failed
    TlsDemultiplexer(String),
    /// Metrics module initialization failed
    Metrics(String),
}

pub struct Core {
    context: Arc<Context>,
}

#[derive(Debug, Clone)]
pub(crate) struct FatalIoError {
    kind: ErrorKind,
    raw_os_error: Option<i32>,
    message: String,
}

impl FatalIoError {
    fn from_io_error(e: &io::Error) -> Self {
        Self {
            kind: e.kind(),
            raw_os_error: e.raw_os_error(),
            message: e.to_string(),
        }
    }

    fn into_io_error(self) -> io::Error {
        match self.raw_os_error {
            Some(code) => {
                io::Error::new(self.kind, format!("{} (os error {})", self.message, code))
            }
            None => io::Error::new(self.kind, self.message),
        }
    }
}

pub(crate) struct Context {
    pub settings: Arc<Settings>,
    pub authenticator: Option<Arc<dyn authentication::Authenticator>>,
    tls_demux: Arc<RwLock<TlsDemux>>,
    pub icmp_forwarder: Option<Arc<IcmpForwarder>>,
    pub shutdown: Arc<Mutex<Shutdown>>,
    /// Channel for propagating fatal IO errors (e.g., EMFILE/ENFILE) from spawned tasks
    /// to the main Core::listen() loop.
    /// Spawned tasks report errors via Context::report_fatal_io_error().
    fatal_error: watch::Sender<Option<FatalIoError>>,
    pub metrics: Arc<Metrics>,
    next_client_id: Arc<AtomicU64>,
    next_tunnel_id: Arc<AtomicU64>,
}

impl Context {
    pub(crate) fn report_fatal_io_error(&self, e: &io::Error) {
        let _ = self.fatal_error.send(Some(FatalIoError::from_io_error(e)));
    }
}

impl Core {
    pub(crate) fn is_too_many_open_files_error(e: &io::Error) -> bool {
        matches!(
            e.raw_os_error(),
            Some(code) if code == libc::EMFILE || code == libc::ENFILE
        )
    }

    pub fn new(
        settings: Settings,
        authenticator: Option<Arc<dyn authentication::Authenticator>>,
        tls_hosts_settings: settings::TlsHostsSettings,
        shutdown: Arc<Mutex<Shutdown>>,
    ) -> Result<Self, Error> {
        if !settings.is_built() {
            settings.validate().map_err(Error::SettingsValidation)?;
        }
        if !tls_hosts_settings.is_built() {
            tls_hosts_settings
                .validate()
                .map_err(Error::SettingsValidation)?;
        }

        Self::warn_legacy_settings(&settings);

        let settings = Arc::new(settings);

        let (fatal_error, _fatal_error_rx) = watch::channel(None);

        Ok(Self {
            context: Arc::new(Context {
                settings: settings.clone(),
                authenticator,
                tls_demux: Arc::new(RwLock::new(
                    TlsDemux::new(&settings, &tls_hosts_settings)
                        .map_err(|e| Error::TlsDemultiplexer(e.to_string()))?,
                )),
                icmp_forwarder: if settings.icmp.is_none() {
                    None
                } else {
                    Some(Arc::new(IcmpForwarder::new(settings)))
                },
                shutdown,
                fatal_error,
                metrics: Metrics::new().map_err(|e| Error::Metrics(e.to_string()))?,
                next_client_id: Default::default(),
                next_tunnel_id: Default::default(),
            }),
        })
    }

    fn warn_legacy_settings(settings: &Settings) {
        if settings.ping_enable && settings.ping_path.is_none() {
            warn!("Ping is enabled but ping_path is not set; ping requests will not match.");
        }
        if settings.speedtest_enable && settings.speedtest_path.is_none() {
            warn!(
                "Speedtest is enabled but speedtest_path is not set; speedtest requests will not match."
            );
        }
        if settings.reverse_proxy.is_none() {
            warn!(
                "Reverse proxy is not configured; non-matching requests will be denied with 404."
            );
        }
        if settings.allow_without_token {
            warn!(
                "Tunnel token is not enforced; tunnel requests without token are allowed."
            );
        }
    }

    /// Run an endpoint instance inside the caller provided asynchronous runtime.
    pub async fn listen(&self) -> io::Result<()> {
        let listen_tcp = async {
            self.listen_tcp()
                .await
                .map_err(|e| io::Error::new(e.kind(), format!("TCP listener failure: {}", e)))
        };

        let listen_udp = async {
            self.listen_udp()
                .await
                .map_err(|e| io::Error::new(e.kind(), format!("UDP listener failure: {}", e)))
        };

        let listen_icmp = async {
            self.listen_icmp()
                .await
                .map_err(|e| io::Error::new(e.kind(), format!("ICMP listener failure: {}", e)))
        };

        let listen_metrics = async {
            metrics::listen(self.context.clone(), log_utils::IdChain::empty())
                .await
                .map_err(|e| io::Error::new(e.kind(), format!("Metrics listener failure: {}", e)))
        };

        let (mut shutdown_notification, _shutdown_completion) = {
            let shutdown = self.context.shutdown.lock().unwrap();
            (
                shutdown.notification_handler(),
                shutdown.completion_guard().ok_or_else(|| {
                    io::Error::new(ErrorKind::Other, "Shutdown is already submitted")
                })?,
            )
        };

        let mut fatal_error_rx = self.context.fatal_error.subscribe();

        tokio::select! {
            x = shutdown_notification.wait() => {
                x.map_err(|e| io::Error::new(ErrorKind::Other, format!("{}", e)))
            },
            x = fatal_error_rx.changed() => match x {
                Ok(()) => match fatal_error_rx.borrow().clone() {
                    Some(e) => Err(e.into_io_error()),
                    None => Err(io::Error::new(ErrorKind::Other, "Unknown fatal error reported")),
                },
                Err(_) => Err(io::Error::new(ErrorKind::Other, "Fatal error channel is unexpectedly closed")),
            },
            x = futures::future::try_join4(
                listen_tcp,
                listen_udp,
                listen_icmp,
                listen_metrics,
            ) => x.map(|_| ()),
        }
    }

    /// Reload the TLS hosts settings
    pub fn reload_tls_hosts_settings(
        &self,
        settings: settings::TlsHostsSettings,
    ) -> io::Result<()> {
        let mut demux = self.context.tls_demux.write().unwrap();

        if !settings.is_built() {
            settings.validate().map_err(|e| {
                io::Error::new(
                    ErrorKind::Other,
                    format!("Settings validation failure: {:?}", e),
                )
            })?;
        }

        *demux = TlsDemux::new(&self.context.settings, &settings)?;
        Ok(())
    }

    async fn listen_tcp(&self) -> io::Result<()> {
        let settings = self.context.settings.clone();
        let has_tcp_based_codec =
            settings.listen_protocols.http1.is_some() || settings.listen_protocols.http2.is_some();

        let tcp_listener = TcpListener::bind(settings.listen_address).await?;
        info!("Listening to TCP {}", settings.listen_address);

        let tls_listener = Arc::new(TlsListener::new());
        loop {
            let client_id = log_utils::IdChain::from(log_utils::IdItem::new(
                log_utils::CLIENT_ID_FMT,
                self.context.next_client_id.fetch_add(1, Ordering::Relaxed),
            ));
            log_id!(trace, client_id, "Accepting TCP connection");
            let (stream, client_addr) = match tcp_listener.accept().await.and_then(|(s, a)| {
                s.set_nodelay(true)?;

                // Enable TCP keepalive to detect broken connections.
                let sock_ref = SockRef::from(&s);
                sock_ref.set_keepalive(true)?;
                Ok((s, a))
            }) {
                Ok((stream, addr)) => {
                    if has_tcp_based_codec {
                        log_id!(debug, client_id, "New TCP client: {}", addr);
                        (stream, addr)
                    } else {
                        continue; // accept just for pings
                    }
                }
                Err(e) => {
                    log_id!(debug, client_id, "TCP connection failed: {}", e);
                    continue;
                }
            };

            tokio::spawn({
                let context = self.context.clone();
                let tls_listener = tls_listener.clone();
                async move {
                    log_id!(trace, client_id, "Starting TLS handshake");
                    let handshake_timeout = context.settings.tls_handshake_timeout;
                    match tokio::time::timeout(handshake_timeout, tls_listener.listen(stream))
                        .await
                        .unwrap_or_else(|_| Err(io::Error::from(ErrorKind::TimedOut)))
                    {
                        Ok(acceptor) => {
                            log_id!(
                                trace,
                                client_id,
                                "TLS handshake complete, processing connection"
                            );
                            if let Err((client_id, message)) = Core::on_new_tls_connection(
                                context.clone(),
                                acceptor,
                                client_addr.ip(),
                                client_id,
                            )
                            .await
                            {
                                log_id!(debug, client_id, "{}", message);
                            }
                        }
                        Err(e) => log_id!(trace, client_id, "TLS handshake failed: {}", e),
                    }
                }
            });
        }
    }

    async fn listen_udp(&self) -> io::Result<()> {
        let settings = self.context.settings.clone();
        if settings.listen_protocols.quic.is_none() {
            return Ok(());
        }

        let socket = UdpSocket::bind(settings.listen_address).await?;
        info!("Listening to UDP {}", settings.listen_address);

        let mut quic_listener = QuicMultiplexer::new(
            settings,
            socket,
            self.context.tls_demux.clone(),
            self.context.next_client_id.clone(),
        )?;

        loop {
            let socket = quic_listener.listen().await?;

            tokio::spawn({
                let context = self.context.clone();
                let socket_id = socket.id();
                async move {
                    log_id!(debug, socket_id, "New QUIC connection");
                    Self::on_new_quic_connection(context, socket, socket_id).await;
                }
            });
        }
    }

    async fn listen_icmp(&self) -> io::Result<()> {
        let forwarder = match &self.context.icmp_forwarder {
            None => return Ok(()),
            Some(x) => x.clone(),
        };

        forwarder.listen().await
    }

    async fn on_new_tls_connection(
        context: Arc<Context>,
        acceptor: TlsAcceptor,
        client_ip: std::net::IpAddr,
        client_id: log_utils::IdChain<u64>,
    ) -> Result<(), (log_utils::IdChain<u64>, String)> {
        log_id!(
            trace,
            client_id,
            "Processing TLS connection from {}",
            client_ip
        );
        let sni = match acceptor.sni() {
            Some(s) => s,
            None => {
                return Err((
                    client_id,
                    "Drop TLS connection due to absence of SNI".to_string(),
                ))
            }
        };
        log_id!(
            trace,
            client_id,
            "TLS SNI: {}",
            net_utils::scrub_sni(sni.to_string())
        );
        // Apply connection filtering rules
        if let Err(deny_reason) = Self::evaluate_connection_rules(
            &context,
            Some(client_ip),
            acceptor.client_random().as_deref(),
            &client_id,
        ) {
            return Err((client_id, deny_reason));
        }

        let core_settings = context.settings.clone();
        let tls_connection_meta = match context
            .tls_demux
            .read()
            .unwrap()
            .select(acceptor.alpn().iter().map(Vec::as_slice), sni)
        {
            Ok(x) if x.protocol == tls_demultiplexer::Protocol::Http3 => {
                return Err((
                    client_id,
                    format!("Dropping connection due to unexpected protocol: {:?}", x),
                ))
            }
            Ok(x) => x,
            Err(e) => {
                return Err((
                    client_id,
                    format!("Dropping connection due to error: {}", e),
                ))
            }
        };
        log_id!(
            debug,
            client_id,
            "Connection meta: {:?}",
            tls_connection_meta
        );

        log_id!(
            trace,
            client_id,
            "Accepting TLS connection with protocol {:?}",
            tls_connection_meta.protocol
        );
        let stream = match tokio::time::timeout(
            context.settings.tls_handshake_timeout,
            acceptor.accept(
                tls_connection_meta.protocol,
                tls_connection_meta.cert_chain,
                tls_connection_meta.key,
                &client_id,
            ),
        )
        .await
        {
            Ok(Ok(s)) => {
                log_id!(debug, client_id, "New TLS client: {:?}", s);
                s
            }
            Ok(Err(e)) => {
                return Err((client_id, format!("TLS connection failed: {}", e)));
            }
            Err(_) => {
                return Err((
                    client_id,
                    "TLS connection failed: handshake timed out".to_string(),
                ));
            }
        };

        log_id!(
            trace,
            client_id,
            "Routing to channel: {:?}",
            tls_connection_meta.channel
        );
        match tls_connection_meta.channel {
            net_utils::Channel::Tunnel => {
                let tunnel_id = client_id.extended(log_utils::IdItem::new(
                    log_utils::TUNNEL_ID_FMT,
                    context.next_tunnel_id.fetch_add(1, Ordering::Relaxed),
                ));
                log_id!(trace, tunnel_id, "Creating tunnel");
                Self::on_tunnel_request(
                    context,
                    tls_connection_meta.protocol,
                    match Self::make_tcp_http_codec(
                        tls_connection_meta.protocol,
                        core_settings,
                        stream,
                        tunnel_id.clone(),
                    ) {
                        Ok(x) => x,
                        Err(e) => {
                            return Err((client_id, format!("Failed to create HTTP codec: {}", e)))
                        }
                    },
                    tls_connection_meta.sni,
                    tls_connection_meta.sni_auth_creds,
                    tunnel_id,
                )
                .await
            }
            net_utils::Channel::Ping => {
                http_ping_handler::listen(
                    context.shutdown.clone(),
                    match Self::make_tcp_http_codec(
                        tls_connection_meta.protocol,
                        core_settings,
                        stream,
                        client_id.clone(),
                    ) {
                        Ok(x) => x,
                        Err(e) => {
                            return Err((client_id, format!("Failed to create HTTP codec: {}", e)))
                        }
                    },
                    context.settings.tls_handshake_timeout,
                    client_id,
                )
                .await
            }
            net_utils::Channel::Speedtest => {
                http_speedtest_handler::listen(
                    context.shutdown.clone(),
                    match Self::make_tcp_http_codec(
                        tls_connection_meta.protocol,
                        core_settings,
                        stream,
                        client_id.clone(),
                    ) {
                        Ok(x) => x,
                        Err(e) => {
                            return Err((client_id, format!("Failed to create HTTP codec: {}", e)))
                        }
                    },
                    context.settings.tls_handshake_timeout,
                    context.settings.speedtest_path.clone(),
                    client_id,
                )
                .await
            }
            net_utils::Channel::ReverseProxy => {
                reverse_proxy::listen(
                    context.clone(),
                    match Self::make_tcp_http_codec(
                        tls_connection_meta.protocol,
                        core_settings,
                        stream,
                        client_id.clone(),
                    ) {
                        Ok(x) => x,
                        Err(e) => {
                            return Err((client_id, format!("Failed to create HTTP codec: {}", e)))
                        }
                    },
                    tls_connection_meta.sni,
                    client_id,
                )
                .await
            }
            net_utils::Channel::Deny => {
                http_deny_handler::listen(
                    context.shutdown.clone(),
                    match Self::make_tcp_http_codec(
                        tls_connection_meta.protocol,
                        core_settings,
                        stream,
                        client_id.clone(),
                    ) {
                        Ok(x) => x,
                        Err(e) => {
                            return Err((client_id, format!("Failed to create HTTP codec: {}", e)))
                        }
                    },
                    context.settings.tls_handshake_timeout,
                    client_id,
                )
                .await
            }
        }

        Ok(())
    }

    async fn on_new_quic_connection(
        context: Arc<Context>,
        socket: QuicSocket,
        client_id: log_utils::IdChain<u64>,
    ) {
        // Apply connection filtering rules
        let client_ip = socket.peer_addr().ok().map(|addr| addr.ip());
        let client_random = Some(socket.client_random());

        if let Err(deny_reason) = Self::evaluate_connection_rules(
            &context,
            client_ip,
            client_random.as_deref(),
            &client_id,
        ) {
            log_id!(debug, client_id, "{}", deny_reason);
            return; // Drop the connection
        }

        let tls_connection_meta = socket.tls_connection_meta();
        log_id!(
            debug,
            client_id,
            "Connection meta: {:?}",
            tls_connection_meta
        );

        match tls_connection_meta.channel {
            net_utils::Channel::Tunnel => {
                let tunnel_id = client_id.extended(log_utils::IdItem::new(
                    log_utils::TUNNEL_ID_FMT,
                    context.next_tunnel_id.fetch_add(1, Ordering::Relaxed),
                ));

                let sni = tls_connection_meta.sni.clone();
                let sni_auth_creds = tls_connection_meta.sni_auth_creds.clone();

                Self::on_tunnel_request(
                    context,
                    tls_connection_meta.protocol,
                    Box::new(Http3Codec::new(socket, tunnel_id.clone())),
                    sni,
                    sni_auth_creds,
                    tunnel_id,
                )
                .await
            }
            net_utils::Channel::Ping => {
                http_ping_handler::listen(
                    context.shutdown.clone(),
                    Box::new(Http3Codec::new(socket, client_id.clone())),
                    context.settings.tls_handshake_timeout,
                    client_id,
                )
                .await
            }
            net_utils::Channel::Speedtest => {
                http_speedtest_handler::listen(
                    context.shutdown.clone(),
                    Box::new(Http3Codec::new(socket, client_id.clone())),
                    context.settings.tls_handshake_timeout,
                    context.settings.speedtest_path.clone(),
                    client_id,
                )
                .await
            }
            net_utils::Channel::ReverseProxy => {
                let sni = tls_connection_meta.sni.clone();

                reverse_proxy::listen(
                    context.clone(),
                    Box::new(Http3Codec::new(socket, client_id.clone())),
                    sni,
                    client_id,
                )
                .await
            }
            net_utils::Channel::Deny => {
                http_deny_handler::listen(
                    context.shutdown.clone(),
                    Box::new(Http3Codec::new(socket, client_id.clone())),
                    context.settings.tls_handshake_timeout,
                    client_id,
                )
                .await
            }
        }
    }

    /// Helper function to evaluate connection filtering rules
    fn evaluate_connection_rules(
        context: &Arc<Context>,
        client_ip: Option<std::net::IpAddr>,
        client_random: Option<&[u8]>,
        log_id: &log_utils::IdChain<u64>,
    ) -> Result<(), String> {
        if let Some(rules_engine) = &context.settings.rules_engine {
            if let Some(ip) = client_ip {
                let rule_result = rules_engine.evaluate(&ip, client_random);
                match rule_result {
                    rules::RuleEvaluation::Deny => {
                        log_id!(
                            debug,
                            log_id,
                            "Connection denied by filtering rules for IP: {}",
                            ip
                        );
                        return Err("Connection denied by filtering rules".to_string());
                    }
                    rules::RuleEvaluation::Allow => {
                        log_id!(debug, log_id, "Connection allowed by filtering rules");
                    }
                }
            } else {
                log_id!(
                    warn,
                    log_id,
                    "Could not extract client IP for rules evaluation"
                );
            }
        }
        Ok(())
    }

    async fn on_tunnel_request(
        context: Arc<Context>,
        protocol: tls_demultiplexer::Protocol,
        codec: Box<dyn HttpCodec>,
        server_name: String,
        sni_auth_creds: Option<String>,
        tunnel_id: log_utils::IdChain<u64>,
    ) {
        let _metrics_guard = Metrics::client_sessions_counter(context.metrics.clone(), protocol);

        let authentication_policy = match context.authenticator.as_ref().zip(sni_auth_creds) {
            None => tunnel::AuthenticationPolicy::Default,
            Some((authenticator, credentials)) => {
                let auth = authentication::Source::Sni(credentials.into());
                match authenticator.authenticate(&auth, &tunnel_id) {
                    authentication::Status::Pass => {
                        tunnel::AuthenticationPolicy::Authenticated(auth)
                    }
                    authentication::Status::Reject => {
                        log_id!(debug, tunnel_id, "SNI authentication failed");
                        return;
                    }
                }
            }
        };

        log_id!(debug, tunnel_id, "New tunnel for client");
        let mut tunnel = Tunnel::new(
            context.clone(),
            Box::new(HttpDownstream::new(context.clone(), codec, server_name)),
            Self::make_forwarder(context),
            authentication_policy,
            tunnel_id.clone(),
        );

        log_id!(trace, tunnel_id, "Listening for client tunnel");
        match tunnel.listen().await {
            Ok(_) => log_id!(debug, tunnel_id, "Tunnel stopped gracefully"),
            Err(e) => log_id!(debug, tunnel_id, "Tunnel stopped with error: {}", e),
        }
    }

    fn make_tcp_http_codec<IO>(
        protocol: tls_demultiplexer::Protocol,
        core_settings: Arc<Settings>,
        io: IO,
        log_id: log_utils::IdChain<u64>,
    ) -> io::Result<Box<dyn HttpCodec>>
    where
        IO: 'static + AsyncRead + AsyncWrite + Unpin + Send + PeerAddr,
    {
        match protocol {
            tls_demultiplexer::Protocol::Http1 => {
                Ok(Box::new(Http1Codec::new(core_settings, io, log_id)))
            }
            tls_demultiplexer::Protocol::Http2 => {
                Ok(Box::new(Http2Codec::new(core_settings, io, log_id)?))
            }
            tls_demultiplexer::Protocol::Http3 => unreachable!(),
        }
    }

    fn make_forwarder(context: Arc<Context>) -> Box<dyn Forwarder> {
        match &context.settings.forward_protocol {
            ForwardProtocolSettings::Direct(_) => Box::new(DirectForwarder::new(context)),
            ForwardProtocolSettings::Socks5(_) => Box::new(Socks5Forwarder::new(context)),
        }
    }
}

#[cfg(test)]
impl Default for Context {
    fn default() -> Self {
        let settings = Arc::new(Settings::default());
        let (fatal_error, _fatal_error_rx) = watch::channel(None);
        Self {
            settings: settings.clone(),
            authenticator: None,
            tls_demux: Arc::new(RwLock::new(
                TlsDemux::new(&settings, &settings::TlsHostsSettings::default()).unwrap(),
            )),
            icmp_forwarder: None,
            shutdown: Shutdown::new(),
            fatal_error,
            metrics: Metrics::new().unwrap(),
            next_client_id: Default::default(),
            next_tunnel_id: Default::default(),
        }
    }
}
