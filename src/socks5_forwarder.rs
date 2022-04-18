use std::collections::{HashMap, HashSet, LinkedList};
use std::io;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use async_socks5::AddrKind;
use async_trait::async_trait;
use bytes::BytesMut;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc;
use crate::forwarder::Forwarder;
use crate::{datagram_pipe, downstream, forwarder, log_id, log_utils, net_utils, pipe};
use crate::icmp_forwarder::IcmpForwarder;
use crate::net_utils::TcpDestination;
use crate::settings::{ForwardProtocolSettings, Settings};
use crate::tcp_forwarder::TcpForwarder;


pub(crate) struct Socks5Forwarder {
    core_settings: Arc<Settings>,
    icmp_forwarder: Option<Arc<IcmpForwarder>>,
}

struct TcpConnector {
    core_settings: Arc<Settings>,
    destination: net_utils::TcpDestination,
    id: log_utils::IdChain<u64>,
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
    core_settings: Arc<Settings>,
    /// Key is the source address received in packet from client
    associations: Mutex<HashMap<SocketAddr, UdpAssociation>>,
    new_socket_tx: mpsc::Sender<()>,
    id: log_utils::IdChain<u64>,
}

type UdpAssociationSocket = async_socks5::SocksDatagram<TcpStream>;

struct UdpAssociation {
    socket: Arc<UdpAssociationSocket>,
    peers: HashSet<SocketAddr>,
}

struct SocketError {
    source: SocketAddr,
    io: io::Error,
}


impl Socks5Forwarder {
    pub fn new(
        core_settings: Arc<Settings>,
        icmp_forwarder: Option<Arc<IcmpForwarder>>,
    ) -> Self {
        Self {
            core_settings: core_settings.clone(),
            icmp_forwarder,
        }
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

        let server_address = *server_address(&self.core_settings);
        let socket = Arc::new(
            async_socks5::SocksDatagram::associate(
                TcpStream::connect(server_address).await?,
                UdpSocket::from_std(net_utils::make_udp_socket(server_address.is_ipv4())?)?,
                None,
                None::<AddrKind>,
            ).await.map_err(socks_to_io_error)?
        );

        self.associations.lock().unwrap().insert(
            meta.source,
            UdpAssociation {
                socket,
                peers: HashSet::from([meta.destination]),
            }
        );

        match self.new_socket_tx.try_send(()) {
            Ok(_) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.associations.lock().unwrap().remove(&meta.source);
                Err(io::Error::new(
                    ErrorKind::Other, "Source waker is unexpectedly closed"
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
    fn tcp_connector(
        &mut self, id: log_utils::IdChain<u64>, destination: net_utils::TcpDestination
    ) -> io::Result<Box<dyn forwarder::TcpConnector>> {
        Ok(Box::new(TcpConnector {
            core_settings: self.core_settings.clone(),
            destination,
            id,
        }))
    }

    fn make_udp_datagram_multiplexer(
        &mut self, id: log_utils::IdChain<u64>
    ) -> io::Result<(
        Arc<dyn forwarder::UdpDatagramPipeShared>,
        Box<dyn datagram_pipe::Source<Output = forwarder::UdpDatagramReadStatus>>,
        Box<dyn datagram_pipe::Sink<Input = downstream::UdpDatagram>>,
    )> {
        let (tx, rx) = mpsc::channel(1);
        let shared = Arc::new(DatagramTransceiverShared {
            core_settings: self.core_settings.clone(),
            associations: Default::default(),
            new_socket_tx: tx,
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
            Box::new(DatagramSink {
                shared,
            }),
        ))
    }

    fn make_icmp_datagram_multiplexer(&mut self, id: log_utils::IdChain<u64>)
        -> io::Result<(
            Box<dyn datagram_pipe::Source<Output = forwarder::IcmpDatagram>>,
            Box<dyn datagram_pipe::Sink<Input = downstream::IcmpDatagram>>,
        )>
    {
        self.icmp_forwarder.as_ref().unwrap().make_multiplexer(id)
    }
}

#[async_trait]
impl forwarder::TcpConnector for TcpConnector {
    async fn connect(self: Box<Self>) -> io::Result<(Box<dyn pipe::Source>, Box<dyn pipe::Sink>)> {
        let mut stream = TcpStream::connect(server_address(&self.core_settings)).await?;
        async_socks5::connect(&mut stream, self.destination, None).await
            .map_err(socks_to_io_error)?;
        Ok(TcpForwarder::pipe_from_stream(stream, self.id))
    }
}

impl DatagramSource {
    async fn read_pending_socket(&self, source: &SocketAddr) -> io::Result<Option<forwarder::UdpDatagramReadStatus>> {
        let socket = match self.shared.associations.lock().unwrap().get(source) {
            None => {
                log_id!(debug, self.shared.id, "UDP association not found: source={}", source);
                return Ok(None);
            }
            Some(x) => x.socket.clone(),
        };

        let mut buffer = BytesMut::new();
        buffer.resize(net_utils::MAX_DATAGRAM_SIZE, 0);
        let (n, peer) = socket.recv_from(buffer.as_mut()).await
            .map_err(socks_to_io_error)?;
        buffer.resize(n, 0);

        Ok(Some(forwarder::UdpDatagramReadStatus::Read(forwarder::UdpDatagram {
            meta: forwarder::UdpDatagramMeta {
                source: socks_to_socket_addr(&peer)?,
                destination: *source,
            },
            payload: buffer.freeze(),
        })))
    }

    fn on_socket_error(&mut self, source: &SocketAddr, error: io::Error) {
        if let Some(a) = self.shared.associations.lock().unwrap().remove(source) {
            self.pending_closures.extend(
                a.peers.into_iter()
                    .map(|peer| { (
                        forwarder::UdpDatagramMeta {
                            source: peer,
                            destination: *source,
                        },
                        io::Error::new(error.kind(), error.to_string()),
                    ) })
            );
        }
    }

    async fn poll_events(&mut self) -> io::Result<Option<SocketError>> {
        let futures = {
            type Future = Box<
                dyn futures::Future<Output = Result<SocketAddr, SocketError>>
                + Send
            >;

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
    source: SocketAddr, socket: Arc<UdpAssociationSocket>
) -> Result<SocketAddr, SocketError> {
    socket.get_ref().readable().await
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
                        log_id!(debug, self.shared.id, "Error reading UDP socket: source={} error={}",
                            source, e);
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

    async fn write(&mut self, datagram: downstream::UdpDatagram) -> io::Result<datagram_pipe::SendStatus> {
        let meta = forwarder::UdpDatagramMeta::from(&datagram.meta);
        let socket = self.shared.associations.lock().unwrap()
            .get(&meta.source)
            .map(|x| x.socket.clone())
            .ok_or_else(|| io::Error::from(ErrorKind::NotFound))?;

        socket.send_to(datagram.payload.as_ref(), meta.destination).await
            .map(|_| datagram_pipe::SendStatus::Sent)
            .map_err(socks_to_io_error)
    }
}

impl Into<async_socks5::AddrKind> for net_utils::TcpDestination {
    fn into(self) -> AddrKind {
        match self {
            TcpDestination::Address(x) => async_socks5::AddrKind::Ip(x),
            TcpDestination::HostName(x) => async_socks5::AddrKind::Domain(x.0, x.1),
        }
    }
}

fn server_address(settings: &Settings) -> &SocketAddr {
    match &settings.forward_protocol {
        ForwardProtocolSettings::Socks5(x) => &x.address,
        ForwardProtocolSettings::Direct(_) => unreachable!(),
    }
}

fn socks_to_socket_addr(x: &async_socks5::AddrKind) -> io::Result<SocketAddr> {
    match x {
        async_socks5::AddrKind::Domain(d, p) => Err(io::Error::new(
            ErrorKind::Other, format!("Unexpected bound address: {}:{}", d, p)
        )),
        async_socks5::AddrKind::Ip(addr) => Ok(*addr),
    }
}

fn socks_to_io_error(err: async_socks5::Error) -> io::Error {
    match err {
        async_socks5::Error::Io(e) => e,
        e => io::Error::new(ErrorKind::Other, e.to_string()),
    }
}
