use std::fmt::{Debug, Formatter};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use async_trait::async_trait;
use bytes::Bytes;
use crate::{datagram_pipe, downstream, icmp_utils, log_utils, pipe};
use crate::net_utils::TcpDestination;


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

/// An abstract interface for a TCP connector implementation
#[async_trait]
pub(crate) trait TcpConnector: Send {
    /// Establish TCP connection to the peer
    async fn connect(self: Box<Self>) -> io::Result<(Box<dyn pipe::Source>, Box<dyn pipe::Sink>)>;
}

/// Encapsulates a shared state of the pipe's source and sink.
/// The default implementation does nothing.
#[async_trait]
pub(crate) trait UdpDatagramPipeShared: Send + Sync {
    /// Notify the pipe of a new UDP "connection"
    async fn on_new_udp_connection(&self, meta: &downstream::UdpDatagramMeta) -> io::Result<()>;

    /// Notify the pipe of a UDP "connection" close
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

/// An abstract interface for a traffic forwarder implementation
pub(crate) trait Forwarder: Send {
    /// Create a TCP connector object
    fn tcp_connector(
        &mut self, id: log_utils::IdChain<u64>, destination: TcpDestination
    ) -> io::Result<Box<dyn TcpConnector>>;

    /// Create a UDP datagram multiplexer
    fn make_udp_datagram_multiplexer(
        &mut self, id: log_utils::IdChain<u64>
    ) -> io::Result<(
        Arc<dyn UdpDatagramPipeShared>,
        Box<dyn datagram_pipe::Source<Output = UdpDatagramReadStatus>>,
        Box<dyn datagram_pipe::Sink<Input = downstream::UdpDatagram>>,
    )>;

    /// Create an ICMP datagram multiplexer
    fn make_icmp_datagram_multiplexer(
        &mut self, id: log_utils::IdChain<u64>
    ) -> io::Result<(
        Box<dyn datagram_pipe::Source<Output = IcmpDatagram>>,
        Box<dyn datagram_pipe::Sink<Input = downstream::IcmpDatagram>>,
    )>;
}

impl UdpDatagramMeta {
    pub fn reversed(&self) -> Self {
        Self{
            source: self.destination,
            destination: self.source,
        }
    }
}

impl From<&downstream::UdpDatagramMeta> for UdpDatagramMeta {
    fn from(x: &downstream::UdpDatagramMeta) -> Self {
        Self{
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
