use std::io;
use std::io::ErrorKind;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::{TcpListener, UdpSocket};
use crate::direct_forwarder::DirectForwarder;
use crate::{downstream_protocol_selector, log_id, log_utils};
use crate::downstream_protocol_selector::{DownstreamProtocol, TunnelProtocol};
use crate::forwarder::Forwarder;
use crate::http1_codec::Http1Codec;
use crate::http2_codec::Http2Codec;
use crate::http3_codec::Http3Codec;
use crate::http_downstream::HttpDownstream;
use crate::icmp_forwarder::IcmpForwarder;
use crate::quic_multiplexer::QuicMultiplexer;
use crate::settings::{ForwardProtocolSettings, ListenProtocolSettings, Settings};
use crate::socks5_forwarder::Socks5Forwarder;
use crate::tls_listener::{TlsAcceptor, TlsListener};
use crate::tunnel::Tunnel;


pub struct Core {
    context: Arc<Context>,
}


struct Context {
    core_settings: Arc<Settings>,
    next_client_id: Arc<AtomicU64>,
    next_tunnel_id: Arc<AtomicU64>,
    icmp_forwarder: Option<Arc<IcmpForwarder>>,
}

impl Core {
    pub fn new(
        settings: Settings,
    ) -> Self {
        let settings = Arc::new(settings);

        Self {
            context: Arc::new(Context {
                core_settings: settings.clone(),
                next_client_id: Arc::new(AtomicU64::new(0)),
                next_tunnel_id: Arc::new(AtomicU64::new(0)),
                icmp_forwarder: if settings.icmp.is_none() {
                    None
                } else {
                    Some(Arc::new(IcmpForwarder::new(settings)))
                },
            }),
        }
    }

    /// Run an endpoint instance inside the caller provided asynchronous runtime.
    /// In this case some of the endpoint settings are ignored as they do not have any sense,
    /// like [`Settings::threads_number`].
    pub async fn listen_async(&mut self) -> io::Result<()> {
        let listen_tcp = async {
            self.listen_tcp().await
                .map_err(|e| io::Error::new(e.kind(), format!("TCP listener failure: {}", e)))
        };
        futures::pin_mut!(listen_tcp);
        let listen_udp = async {
            self.listen_udp().await
                .map_err(|e| io::Error::new(e.kind(), format!("UDP listener failure: {}", e)))
        };
        futures::pin_mut!(listen_udp);

        let listen_icmp = async {
            self.listen_icmp().await
                .map_err(|e| io::Error::new(e.kind(), format!("ICMP listener failure: {}", e)))
        };
        futures::pin_mut!(listen_icmp);

        futures::future::try_join3(
            listen_tcp,
            listen_udp,
            listen_icmp,
        ).await.map(|_| ())
    }

    /// Run an endpoint instance in a blocking way.
    /// This one will set up its own asynchronous runtime.
    pub fn listen(&mut self) -> io::Result<()> {
        let runtime = {
            let context = self.context.clone();
            let threads_num = context.core_settings.threads_number;
            tokio::runtime::Builder::new_multi_thread()
                .enable_io()
                .enable_time()
                .max_blocking_threads(1) // tokio does not allow to turn it off
                .worker_threads(threads_num)
                .build()?
        };

        let _guard = runtime.enter();

        runtime.block_on(async {
            self.listen_async().await
        })
    }

    async fn listen_tcp(&self) -> io::Result<()> {
        let settings = self.context.core_settings.clone();
        let has_tcp_based_codec = settings.listen_protocols.iter()
            .any(|x| match x {
                ListenProtocolSettings::Http1(_) | ListenProtocolSettings::Http2(_) => true,
                ListenProtocolSettings::Quic(_) => false,
            });

        let tcp_listener = TcpListener::bind(settings.listen_address).await?;
        info!("Listening to TCP {}", settings.listen_address);

        let tls_listener = Arc::new(TlsListener::new(settings.clone()));
        loop {
            let client_id = log_utils::IdChain::from(log_utils::IdItem::new(
                log_utils::CLIENT_ID_FMT, self.context.next_client_id.fetch_add(1, Ordering::Relaxed)
            ));
            let stream = match tcp_listener.accept().await {
                Ok((stream, addr)) => if has_tcp_based_codec {
                    log_id!(debug, client_id, "New TCP client: {}", addr);
                    stream
                } else {
                    continue; // accept just for pings
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
                    let handshake_timeout = context.core_settings.tls_handshake_timeout;
                    match tokio::time::timeout(handshake_timeout, tls_listener.listen(stream))
                        .await
                        .unwrap_or_else(|_| Err(io::Error::from(ErrorKind::TimedOut)))
                    {
                        Ok(stream) => Core::on_new_tls_connection(context.clone(), stream, client_id).await,
                        Err(e) => log_id!(debug, client_id, "TLS handshake failed: {}", e),
                    }
                }
            });
        }
    }

    async fn listen_udp(&self) -> io::Result<()> {
        let settings = self.context.core_settings.clone();
        if !settings.listen_protocols.iter()
            .any(|x| match x {
                ListenProtocolSettings::Http1(_) | ListenProtocolSettings::Http2(_) => false,
                ListenProtocolSettings::Quic(_) => true,
            })
        {
            return Ok(());
        }

        let socket = UdpSocket::bind(settings.listen_address).await?;
        info!("Listening to UDP {}", settings.listen_address);

        let mut quic_listener = QuicMultiplexer::new(
            settings,
            socket,
            self.context.next_client_id.clone(),
        );

        loop {
            let socket = quic_listener.listen().await?;

            tokio::spawn({
                let context = self.context.clone();
                let socket_id = socket.id();
                async move {
                    log_id!(debug, socket_id, "New QUIC connection");
                    let mut tunnel = Tunnel::new(
                        context.core_settings.clone(),
                        Box::new(HttpDownstream::new(
                            context.core_settings.clone(),
                            Box::new(Http3Codec::new(socket, socket_id.clone())),
                        )),
                        Self::make_forwarder(&context),
                        socket_id.clone(),
                    );

                    log_id!(trace, socket_id, "Listening for client tunnel");
                    match tunnel.listen().await {
                        Ok(_) => log_id!(debug, socket_id, "Tunnel stopped gracefully"),
                        Err(e) => log_id!(debug, socket_id, "Tunnel stopped with error: {}", e),
                    }
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

    async fn on_new_tls_connection(context: Arc<Context>, acceptor: TlsAcceptor, client_id: log_utils::IdChain<u64>) {
        let sni = match acceptor.sni() {
            Some(s) => s,
            None => {
                log_id!(debug, client_id, "Drop TLS connection due to absence of SNI");
                return;
            }
        };

        let alpn = match acceptor.alpn().map(String::from_utf8) {
            Some(Ok(p)) => Some(p),
            Some(Err(e)) => {
                log_id!(debug, client_id, "Drop TLS connection due to malformed ALPN: {:?} (error: {})",
                    acceptor.alpn().unwrap(), e);
                return;
            }
            None => None,
        };

        let core_settings = context.core_settings.clone();
        let proto =
            match downstream_protocol_selector::select(core_settings.clone(), alpn.as_deref(), &sni) {
                Ok(DownstreamProtocol::Tunnel(TunnelProtocol::Http3)) => {
                    log_id!(debug, client_id, "Unexpected connection protocol - dropping tunnel");
                    return;
                }
                Ok(x) => x,
                Err(e) => {
                    log_id!(debug, client_id, "Dropping tunnel due to error: {}", e);
                    return;
                }
            };
        log_id!(trace, client_id, "Selected protocol: {:?}", proto);

        let stream = match acceptor.accept(proto.as_alpn().as_bytes().to_vec()).await {
            Ok(s) => {
                log_id!(debug, client_id, "New TLS client: {:?}", s);
                s
            }
            Err(e) => {
                log_id!(debug, client_id, "TLS connection failed: {}", e);
                return;
            }
        };

        match proto {
            DownstreamProtocol::Tunnel(TunnelProtocol::Http3) => unreachable!(),
            DownstreamProtocol::Tunnel(protocol) => {
                let tunnel_id = client_id.extended(log_utils::IdItem::new(
                    log_utils::TUNNEL_ID_FMT, context.next_tunnel_id.fetch_add(1, Ordering::Relaxed)
                ));

                log_id!(debug, tunnel_id, "New tunnel for client");
                let mut tunnel = Tunnel::new(
                    core_settings.clone(),
                    Box::new(HttpDownstream::new(
                        core_settings.clone(),
                        match protocol {
                            TunnelProtocol::Http1 => Box::new(Http1Codec::new(
                                core_settings, stream, tunnel_id.clone(),
                            )),
                            TunnelProtocol::Http2 => Box::new(Http2Codec::new(
                                core_settings, stream, tunnel_id.clone(),
                            )),
                            TunnelProtocol::Http3 => unreachable!(),
                        },
                    )),
                    Self::make_forwarder(&context),
                    tunnel_id.clone(),
                );

                log_id!(trace, tunnel_id, "Listening for client tunnel");
                match tunnel.listen().await {
                    Ok(_) => log_id!(debug, tunnel_id, "Tunnel stopped gracefully"),
                    Err(e) => log_id!(debug, tunnel_id, "Tunnel stopped with error: {}", e),
                }
            }
            DownstreamProtocol::ServiceMessenger(_) => { todo!() }
        }
    }

    fn make_forwarder(context: &Context) -> Box<dyn Forwarder> {
        match &context.core_settings.forward_protocol {
            ForwardProtocolSettings::Direct(_) => Box::new(DirectForwarder::new(
                context.core_settings.clone(),
                context.icmp_forwarder.clone(),
            )),
            ForwardProtocolSettings::Socks5(_) => Box::new(Socks5Forwarder::new(
                context.core_settings.clone(),
                context.icmp_forwarder.clone(),
            )),
        }
    }
}
