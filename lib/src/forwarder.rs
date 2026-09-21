use crate::net_utils::TcpDestination;
use crate::{authentication, datagram_pipe, downstream, icmp_utils, log_utils, pipe, tunnel};
use async_trait::async_trait;
use bytes::Bytes;
use std::fmt::{Debug, Formatter};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

#[derive(Debug, Hash, Eq, PartialEq, Copy, Clone)]
pub(crate) struct UdpDatagramMeta {
    pub source: SocketAddr,
    pub destination: SocketAddr,
}

pub(crate) struct UdpDatagram {
    pub meta: UdpDatagramMeta,
    pub payload: Bytes,
}

#[derive(Debug, Hash, Eq, PartialEq, Copy, Clone)]
pub(crate) struct IcmpDatagramMeta {
    pub peer: IpAddr,
}

#[derive(Debug)]
pub(crate) struct IcmpDatagram {
    pub meta: IcmpDatagramMeta,
    pub message: icmp_utils::Message,
}

#[derive(Debug, Clone)]
pub(crate) struct TcpConnectionMeta {
    /// Address of a VPN client made the connection request
    pub client_address: IpAddr,
    /// Destination address of the connection
    pub destination: TcpDestination,
    /// Authentication request source
    pub auth: Option<authentication::Source<'static>>,
    /// The domain name used for TLS session (SNI)
    pub tls_domain: String,
    /// May contain a platform name of the VPN client and name of the application
    /// initiated the request
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct UdpMultiplexerMeta {
    /// An address of the VPN client establishing the UDP tunnel
    pub client_address: IpAddr,
    /// Authentication request source
    pub auth: Option<authentication::Source<'static>>,
    /// The domain name used for TLS session (SNI)
    pub tls_domain: String,
    /// May contain a platform name of the VPN client
    pub user_agent: Option<String>,
}

/// An abstract interface for a TCP connector implementation
#[async_trait]
pub(crate) trait TcpConnector: Send {
    /// Establish TCP connection to the peer
    async fn connect(
        self: Box<Self>,
        id: log_utils::IdChain<u64>,
        meta: TcpConnectionMeta,
    ) -> Result<(Box<dyn pipe::Source>, Box<dyn pipe::Sink>), tunnel::ConnectionError>;
}

/// An abstract interface for a datagram multiplexer authenticator
#[async_trait]
pub(crate) trait DatagramMultiplexerAuthenticator: Send {
    /// Perform an authentication procedure
    async fn check_auth(
        self: Box<Self>,
        client_address: IpAddr,
        tls_domain: &'_ str,
        auth: authentication::Source<'_>,
        user_agent: Option<&'_ str>,
    ) -> Result<(), tunnel::ConnectionError>;
}

/// Encapsulates a shared state of the pipe's source and sink.
/// The default implementation does nothing.
#[async_trait]
pub(crate) trait UdpDatagramPipeShared: Send + Sync {
    /// Notify the pipe of a new UDP "connection"
    ///
    /// `meta` is forward-oriented (`source` = the client, `destination` = the
    /// target). Implementations store the connection under this orientation, so
    /// it must match what `on_connection_closed` derives the forward key from
    async fn on_new_udp_connection(&self, meta: &downstream::UdpDatagramMeta) -> io::Result<()>;

    /// Notify the pipe of a UDP "connection" close
    ///
    /// `meta` MUST be reverse-oriented (i.e. `source` = the target a datagram
    /// was received from, `destination` = the client it is forwarded to).
    /// Implementations look up their internally stored connections by the
    /// forward-oriented key, so passing a forward-oriented `meta` would miss
    /// the entry and leak the underlying socket
    fn on_connection_closed(&self, meta: &UdpDatagramMeta);
}

/// The status of successful [`DatagramSource.read`]
#[derive(Debug)]
pub(crate) enum UdpDatagramReadStatus {
    /// The datagram received from a peer
    Read(UdpDatagram),
    /// UDP "connection" closed for some reason
    UdpClose(UdpDatagramMeta, io::Error),
}

pub(crate) type UdpMultiplexer = (
    Arc<dyn UdpDatagramPipeShared>,
    Box<dyn datagram_pipe::Source<Output = UdpDatagramReadStatus>>,
    Box<dyn datagram_pipe::Sink<Input = downstream::UdpDatagram>>,
);

pub(crate) type IcmpMultiplexer = (
    Box<dyn datagram_pipe::Source<Output = IcmpDatagram>>,
    Box<dyn datagram_pipe::Sink<Input = downstream::IcmpDatagram>>,
);

/// An abstract interface for a traffic forwarder implementation
pub(crate) trait Forwarder: Send {
    /// Create a TCP connector object
    fn tcp_connector(&self) -> Box<dyn TcpConnector>;

    /// Create a datagram multiplexer authenticator
    fn datagram_mux_authenticator(&self) -> Box<dyn DatagramMultiplexerAuthenticator>;

    /// Create a UDP datagram multiplexer
    fn make_udp_datagram_multiplexer(
        &self,
        id: log_utils::IdChain<u64>,
        meta: UdpMultiplexerMeta,
    ) -> io::Result<UdpMultiplexer>;

    /// Create an ICMP datagram multiplexer
    fn make_icmp_datagram_multiplexer(
        &self,
        id: log_utils::IdChain<u64>,
    ) -> io::Result<Option<IcmpMultiplexer>>;
}

impl UdpDatagramMeta {
    pub fn reversed(&self) -> Self {
        Self {
            source: self.destination,
            destination: self.source,
        }
    }
}

impl From<&downstream::UdpDatagramMeta> for UdpDatagramMeta {
    fn from(x: &downstream::UdpDatagramMeta) -> Self {
        Self {
            source: x.source,
            destination: x.destination,
        }
    }
}

impl Debug for UdpDatagram {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "meta={:?}, payload={}B", self.meta, self.payload.len())
    }
}

impl datagram_pipe::Datagram for IcmpDatagram {
    fn len(&self) -> usize {
        self.message.len()
    }
}
