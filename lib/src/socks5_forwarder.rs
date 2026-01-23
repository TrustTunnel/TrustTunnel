use crate::forwarder::{Forwarder, IcmpMultiplexer, UdpMultiplexer};
use crate::settings::{ForwardProtocolSettings, Settings, Socks5ForwarderSettings};
use crate::tcp_forwarder::TcpForwarder;
use crate::{
    authentication, core, datagram_pipe, downstream, forwarder, log_id, log_utils, net_utils, pipe,
    socks5_client, tunnel,
};
use async_trait::async_trait;
use base64::Engine;
use bytes::BytesMut;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet, LinkedList};
use std::io;
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr};
use std::ops::Deref;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

pub(crate) struct Socks5Forwarder {
    context: Arc<core::Context>,
}

struct TcpConnector {
    context: Arc<core::Context>,
}

struct DatagramSource {
    shared: Arc<DatagramTransceiverShared>,
    new_socket_rx: mpsc::Receiver<()>,
    pending_read: Option<SocketAddr>,
    pending_closures: LinkedList<(forwarder::UdpDatagramMeta, io::Error)>,
}

struct DatagramSink {
    shared: Arc<DatagramTransceiverShared>,
}

struct DatagramTransceiverShared {
    context: Arc<core::Context>,
    /// Key is the source address received in packet from client
    associations: Mutex<HashMap<SocketAddr, UdpAssociation>>,
    new_socket_tx: mpsc::Sender<()>,
    auth: Option<socks5_client::Authentication<'static>>,
    id: log_utils::IdChain<u64>,
}

type UdpAssociationSocket = socks5_client::UdpAssociation<TcpStream>;

struct UdpAssociation {
    socket: Arc<UdpAssociationSocket>,
    peers: HashSet<SocketAddr>,
    _metrics_guard: crate::metrics::OutboundUdpSocketCounter,
}

struct SocketError {
    source: SocketAddr,
    io: io::Error,
}

struct DatagramMuxAuthenticator {
    context: Arc<core::Context>,
}

impl Socks5Forwarder {
    pub fn new(context: Arc<core::Context>) -> Self {
        Self { context }
    }
}

#[async_trait]
impl forwarder::UdpDatagramPipeShared for DatagramTransceiverShared {
    async fn on_new_udp_connection(&self, meta: &downstream::UdpDatagramMeta) -> io::Result<()> {
        if let Some(x) = self.associations.lock().unwrap().get_mut(&meta.source) {
            let is_new = x.peers.insert(meta.destination);
            debug_assert!(is_new, "{:?}", meta);
            return Ok(());
        }

        let socket = match socks5_client::connect(
            TcpStream::connect(socks_settings(&self.context.settings).address).await?,
            self.auth.clone(),
            socks5_client::Request::UdpAssociate,
        )
        .await
        {
            Ok(socks5_client::ConnectResult::TcpConnection(_)) => unreachable!(),
            Ok(socks5_client::ConnectResult::UdpAssociation(x)) => Arc::new(x),
            Ok(socks5_client::ConnectResult::Failure(x)) => {
                return Err(io::Error::new(
                    ErrorKind::Other,
                    format!("SOCKS server replied with error code: {:?}", x),
                ))
            }
            Err(socks5_client::Error::Io(x)) => return Err(x),
            Err(socks5_client::Error::Protocol(x)) => {
                return Err(io::Error::new(
                    ErrorKind::Other,
                    format!("SOCKS protocol error: {}", x),
                ))
            }
            Err(socks5_client::Error::Authentication(x)) => {
                self.associations.lock().unwrap().clear();
                return Err(io::Error::new(
                    ErrorKind::Other,
                    format!("Authentication error: {}", x),
                ));
            }
        };

        let metrics_guard = self.context.metrics.clone().outbound_udp_socket_counter();
        self.associations.lock().unwrap().insert(
            meta.source,
            UdpAssociation {
                socket,
                peers: HashSet::from([meta.destination]),
                _metrics_guard: metrics_guard,
            },
        );

        match self.new_socket_tx.try_send(()) {
            Ok(_) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.associations.lock().unwrap().remove(&meta.source);
                Err(io::Error::new(
                    ErrorKind::Other,
                    "Source waker is unexpectedly closed",
                ))
            }
        }
    }

    fn on_connection_closed(&self, meta: &forwarder::UdpDatagramMeta) {
        let mut associations = self.associations.lock().unwrap();
        if let Some(mut x) = associations.remove(&meta.destination) {
            x.peers.remove(&meta.source);
            if !x.peers.is_empty() {
                associations.insert(meta.destination, x);
            }
        }
    }
}

impl Forwarder for Socks5Forwarder {
    fn tcp_connector(&self) -> Box<dyn forwarder::TcpConnector> {
        Box::new(TcpConnector {
            context: self.context.clone(),
        })
    }

    fn datagram_mux_authenticator(&self) -> Box<dyn forwarder::DatagramMultiplexerAuthenticator> {
        Box::new(DatagramMuxAuthenticator {
            context: self.context.clone(),
        })
    }

    fn make_udp_datagram_multiplexer(
        &self,
        id: log_utils::IdChain<u64>,
        meta: forwarder::UdpMultiplexerMeta,
    ) -> io::Result<UdpMultiplexer> {
        let (tx, rx) = mpsc::channel(1);
        let shared = Arc::new(DatagramTransceiverShared {
            context: self.context.clone(),
            associations: Default::default(),
            new_socket_tx: tx,
            auth: meta
                .auth
                .map(|x| {
                    if socks_settings(&self.context.settings).extended_auth {
                        make_extended_auth(
                            x,
                            &meta.tls_domain,
                            &meta.client_address,
                            meta.user_agent.as_ref().map(|x| x.as_ref()),
                        )
                        .map(socks5_client::Authentication::into_owned)
                    } else {
                        make_auth(x)
                    }
                })
                .transpose()
                .map_err(|x| io::Error::new(ErrorKind::Other, x))?,
            id,
        });

        Ok((
            shared.clone(),
            Box::new(DatagramSource {
                shared: shared.clone(),
                new_socket_rx: rx,
                pending_read: None,
                pending_closures: Default::default(),
            }),
            Box::new(DatagramSink { shared }),
        ))
    }

    fn make_icmp_datagram_multiplexer(
        &self,
        id: log_utils::IdChain<u64>,
    ) -> io::Result<Option<IcmpMultiplexer>> {
        self.context
            .icmp_forwarder
            .as_ref()
            .map(|x| x.make_multiplexer(id))
            .transpose()
    }
}

#[async_trait]
impl forwarder::TcpConnector for TcpConnector {
    async fn connect(
        self: Box<Self>,
        id: log_utils::IdChain<u64>,
        meta: forwarder::TcpConnectionMeta,
    ) -> Result<(Box<dyn pipe::Source>, Box<dyn pipe::Sink>), tunnel::ConnectionError> {
        let (destination, port) = match &meta.destination {
            net_utils::TcpDestination::Address(x) => {
                (socks5_client::Address::IpAddress(x.ip()), x.port())
            }
            net_utils::TcpDestination::HostName(x) => {
                (socks5_client::Address::DomainName(Cow::Borrowed(&x.0)), x.1)
            }
        };

        let stream = match TcpStream::connect(socks_settings(&self.context.settings).address).await {
            Ok(s) => s,
            Err(e) => return Err(tunnel::ConnectionError::Io(e)),
        };

        match socks5_client::connect(
            stream,
            meta.auth
                .map(|x| {
                    if socks_settings(&self.context.settings).extended_auth {
                        make_extended_auth(
                            x,
                            &meta.tls_domain,
                            &meta.client_address,
                            meta.user_agent.as_ref().map(|x| x.as_ref()),
                        )
                    } else {
                        make_auth(x)
                    }
                })
                .transpose()
                .map_err(|e| tunnel::ConnectionError::Other(e.into()))?,
            socks5_client::Request::Connect(destination, port),
        )
        .await
        {
            Ok(socks5_client::ConnectResult::TcpConnection(stream)) => {
                let metrics_guard = self.context.metrics.clone().outbound_tcp_socket_counter();
                Ok(TcpForwarder::pipe_from_stream(stream, id, metrics_guard))
            }
            Ok(socks5_client::ConnectResult::UdpAssociation(_)) => unreachable!(),
            Ok(socks5_client::ConnectResult::Failure(
                socks5_client::ReplyCode::HostUnreachable,
            )) => Err(tunnel::ConnectionError::HostUnreachable),
            Ok(socks5_client::ConnectResult::Failure(
                socks5_client::ReplyCode::NetworkUnreachable,
            )) => Err(tunnel::ConnectionError::HostUnreachable),
            Ok(socks5_client::ConnectResult::Failure(
                socks5_client::ReplyCode::ConnectionRefused,
            )) => Err(tunnel::ConnectionError::Io(
                ErrorKind::ConnectionRefused.into(),
            )),
            Ok(socks5_client::ConnectResult::Failure(socks5_client::ReplyCode::TtlExpired)) => {
                Err(tunnel::ConnectionError::Timeout)
            }
            Ok(socks5_client::ConnectResult::Failure(x)) => Err(tunnel::ConnectionError::Other(
                format!("SOCKS server replied with error code: {:?}", x).into(),
            )),
            Err(socks5_client::Error::Io(x)) => Err(tunnel::ConnectionError::Io(x)),
            Err(socks5_client::Error::Protocol(x)) => Err(tunnel::ConnectionError::Other(format!(
                "SOCKS protocol error: {}",
                x
            ).into())),
            Err(socks5_client::Error::Authentication(x)) => {
                Err(tunnel::ConnectionError::Authentication(x.into()))
            }
        }
    }
}

#[async_trait]
impl forwarder::DatagramMultiplexerAuthenticator for DatagramMuxAuthenticator {
    async fn check_auth(
        self: Box<Self>,
        client_address: IpAddr,
        tls_domain: &'_ str,
        auth: authentication::Source<'_>,
        user_agent: Option<&'_ str>,
    ) -> Result<(), tunnel::ConnectionError> {
        match socks5_client::connect(
            TcpStream::connect(socks_settings(&self.context.settings).address)
                .await
                .map_err(tunnel::ConnectionError::Io)?,
            Some(
                if socks_settings(&self.context.settings).extended_auth {
                    make_extended_auth(auth, tls_domain, &client_address, user_agent)
                } else {
                    make_auth(auth)
                }
                .map_err(|x| tunnel::ConnectionError::Io(io::Error::new(ErrorKind::Other, x)))?,
            ),
            socks5_client::Request::UdpAssociate,
        )
        .await
        {
            Ok(socks5_client::ConnectResult::TcpConnection(_)) => unreachable!(),
            Ok(socks5_client::ConnectResult::UdpAssociation(_)) => Ok(()),
            Ok(socks5_client::ConnectResult::Failure(x)) => Err(tunnel::ConnectionError::Other(
                format!("SOCKS server replied with error code: {:?}", x).into(),
            )),
            Err(socks5_client::Error::Io(x)) => Err(tunnel::ConnectionError::Io(x)),
            Err(socks5_client::Error::Protocol(x)) => Err(tunnel::ConnectionError::Other(format!(
                "SOCKS protocol error: {}",
                x
            ).into())),
            Err(socks5_client::Error::Authentication(x)) => {
                Err(tunnel::ConnectionError::Authentication(x.into()))
            }
        }
    }
}

impl DatagramSource {
    async fn read_pending_socket(
        &self,
        source: &SocketAddr,
    ) -> io::Result<Option<forwarder::UdpDatagramReadStatus>> {
        let socket = match self.shared.associations.lock().unwrap().get(source) {
            None => {
                log_id!(
                    debug,
                    self.shared.id,
                    "UDP association not found: source={}",
                    source
                );
                return Ok(None);
            }
            Some(x) => x.socket.clone(),
        };

        let mut buffer = BytesMut::zeroed(net_utils::MAX_UDP_PAYLOAD_SIZE);
        let (n, peer) = socket
            .recv_from(buffer.as_mut())
            .await
            .map_err(socks_to_io_error)?;
        buffer.truncate(n);

        Ok(Some(forwarder::UdpDatagramReadStatus::Read(
            forwarder::UdpDatagram {
                meta: forwarder::UdpDatagramMeta {
                    source: peer,
                    destination: *source,
                },
                payload: buffer.freeze(),
            },
        )))
    }

    fn on_socket_error(&mut self, source: &SocketAddr, error: io::Error) {
        if let Some(a) = self.shared.associations.lock().unwrap().remove(source) {
            self.pending_closures
                .extend(a.peers.into_iter().map(|peer| {
                    (
                        forwarder::UdpDatagramMeta {
                            source: peer,
                            destination: *source,
                        },
                        io::Error::new(error.kind(), error.to_string()),
                    )
                }));
        }
    }

    async fn poll_events(&mut self) -> io::Result<Option<SocketError>> {
        let futures = {
            type Future = Box<dyn futures::Future<Output = Result<SocketAddr, SocketError>> + Send>;

            let associations = self.shared.associations.lock().unwrap();
            let mut futures: Vec<Pin<Future>> = Vec::with_capacity(1 + associations.len());
            // add always pending future to avoid a busy loop in case of connection absence
            futures.push(Box::pin(futures::future::pending()));
            for (meta, assoc) in associations.deref() {
                futures.push(Box::pin(listen_socket_read(*meta, assoc.socket.clone())));
            }
            futures
        };

        let wait_reads = futures::future::select_all(futures);
        tokio::pin!(wait_reads);

        let wait_new_socket = self.new_socket_rx.recv();
        tokio::pin!(wait_new_socket);

        tokio::select! {
            reads = wait_reads => match reads.0 {
                Ok(ready) => {
                    debug_assert!(self.pending_read.is_none(), "{:?}", self.pending_read);
                    self.pending_read = Some(ready);
                    Ok(None)
                }
                Err(e) => {
                    log_id!(debug, self.shared.id, "Error waiting for UDP read: source={} error={}",
                        e.source, e.io);
                    Ok(Some(e))
                }
            },
            r = wait_new_socket => match r {
                Some(_) => Ok(None),
                None => {
                    log_id!(debug, self.shared.id, "Wake sender dropped");
                    Err(io::Error::from(ErrorKind::UnexpectedEof))
                }
            }
        }
    }
}

async fn listen_socket_read(
    source: SocketAddr,
    socket: Arc<UdpAssociationSocket>,
) -> Result<SocketAddr, SocketError> {
    socket
        .get_ref()
        .readable()
        .await
        .map(|_| source)
        .map_err(|io| SocketError { source, io })
}

#[async_trait]
impl datagram_pipe::Source for DatagramSource {
    type Output = forwarder::UdpDatagramReadStatus;

    fn id(&self) -> log_utils::IdChain<u64> {
        self.shared.id.clone()
    }

    async fn read(&mut self) -> io::Result<forwarder::UdpDatagramReadStatus> {
        loop {
            if let Some(source) = self.pending_read.take() {
                match self.read_pending_socket(&source).await {
                    Ok(None) => (),
                    Ok(Some(x)) => return Ok(x),
                    Err(e) => {
                        log_id!(
                            debug,
                            self.shared.id,
                            "Error reading UDP socket: source={} error={}",
                            source,
                            e
                        );
                        self.on_socket_error(&source, e);
                    }
                }
            }

            if let Some((meta, error)) = self.pending_closures.pop_front() {
                return Ok(forwarder::UdpDatagramReadStatus::UdpClose(meta, error));
            }

            if let Some(err) = self.poll_events().await? {
                self.on_socket_error(&err.source, err.io);
            }
        }
    }
}

#[async_trait]
impl datagram_pipe::Sink for DatagramSink {
    type Input = downstream::UdpDatagram;

    async fn write(
        &mut self,
        datagram: downstream::UdpDatagram,
    ) -> io::Result<datagram_pipe::SendStatus> {
        let meta = forwarder::UdpDatagramMeta::from(&datagram.meta);
        let socket = self
            .shared
            .associations
            .lock()
            .unwrap()
            .get(&meta.source)
            .map(|x| x.socket.clone())
            .ok_or_else(|| io::Error::from(ErrorKind::NotFound))?;

        socket
            .send_to(datagram.payload.as_ref(), meta.destination)
            .await
            .map(|_| datagram_pipe::SendStatus::Sent)
            .map_err(socks_to_io_error)
    }
}

fn make_auth(auth: authentication::Source) -> Result<socks5_client::Authentication, String> {
    Ok(match auth {
        authentication::Source::Sni(x) => {
            socks5_client::Authentication::UsernamePassword(x.clone(), x)
        }
        authentication::Source::ProxyBasic(x) => {
            let credentials = base64::engine::general_purpose::STANDARD
                .decode(x.as_ref())
                .map_err(|e| e.to_string())
                .and_then(|x| String::from_utf8(x).map_err(|e| e.to_string()))?;
            let mut split = credentials.splitn(2, ':');

            socks5_client::Authentication::UsernamePassword(
                Cow::Owned(String::from(split.next().unwrap())),
                Cow::Owned(
                    split
                        .next()
                        .map(String::from)
                        .ok_or_else(|| "Expected colon-separated credentials".to_string())?,
                ),
            )
        }
    })
}

fn make_extended_auth<'a>(
    auth: authentication::Source<'a>,
    tls_domain: &'a str,
    client_address: &IpAddr,
    user_agent: Option<&'a str>,
) -> Result<socks5_client::Authentication<'a>, String> {
    let mut values = vec![
        socks5_client::ExtendedAuthenticationValue::Domain(Cow::Borrowed(tls_domain)),
        socks5_client::ExtendedAuthenticationValue::ClientAddress(*client_address),
    ];

    if let Some(user_agent) = user_agent {
        values.push(socks5_client::ExtendedAuthenticationValue::UserAgent(
            Cow::Borrowed(user_agent),
        ));
    }

    match auth {
        authentication::Source::Sni(_) => {
            values.push(socks5_client::ExtendedAuthenticationValue::SniAuth)
        }
        authentication::Source::ProxyBasic(x) => values.push(
            socks5_client::ExtendedAuthenticationValue::BasicProxyAuth(x),
        ),
    }

    Ok(socks5_client::Authentication::Extended(values))
}

const fn socks_settings(settings: &Settings) -> &Socks5ForwarderSettings {
    match &settings.forward_protocol {
        ForwardProtocolSettings::Socks5(x) => x,
        ForwardProtocolSettings::Direct(_) => unreachable!(),
    }
}

fn socks_to_io_error(err: socks5_client::Error) -> io::Error {
    match err {
        socks5_client::Error::Io(e) => e,
        socks5_client::Error::Protocol(e) => {
            io::Error::new(ErrorKind::Other, format!("SOCKS protocol error: {}", e))
        }
        socks5_client::Error::Authentication(e) => {
            io::Error::new(ErrorKind::Other, format!("Authentication error: {}", e))
        }
    }
}
