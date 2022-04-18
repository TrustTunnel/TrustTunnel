use std::fmt::{Debug, Formatter};
use std::io;
use std::net::{IpAddr, SocketAddr};
use async_trait::async_trait;
use bytes::Bytes;
use crate::{authorization, datagram_pipe, forwarder, icmp_utils, log_utils, pipe};
use crate::net_utils::TcpDestination;


#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) struct UdpDatagramMeta {
    pub source: SocketAddr,
    pub destination: SocketAddr,
    pub app_name: Option<String>,
}

pub(crate) struct UdpDatagram {
    pub meta: UdpDatagramMeta,
    pub payload: Bytes,
}

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) struct IcmpDatagramMeta {
    pub peer: IpAddr,
}

#[derive(Debug)]
pub(crate) struct IcmpDatagram {
    pub meta: IcmpDatagramMeta,
    pub message: icmp_utils::Message,
    pub ttl: u8,
}

pub(crate) trait StreamId {
    /// Get the request ID for logging
    fn id(&self) -> log_utils::IdChain<u64>;
}

/// An abstract interface for an authorization request implementation
pub(crate) trait AuthorizationRequest: StreamId + Send {
    /// Get the authorization info
    fn auth_info(&self) -> io::Result<authorization::Source>;

    /// Proceed the successfully authorized request
    fn succeed_request(self: Box<Self>) -> io::Result<Option<AuthorizedRequest>>;

    /// Reject the request failed authorization
    fn fail_request(self: Box<Self>);
}

pub(crate) enum AuthorizedRequest {
    TcpConnect(Box<dyn PendingTcpConnectRequest>),
    DatagramMultiplexer(Box<dyn PendingDatagramMultiplexerRequest>),
}

/// An abstract interface for a TCP connection request implementation
pub(crate) trait PendingTcpConnectRequest: StreamId + Send {
    /// Get the target host
    fn destination(&self) -> io::Result<TcpDestination>;

    /// Notify a client of the successfully tunneled connection
    fn succeed_request(self: Box<Self>) -> io::Result<(Box<dyn pipe::Source>, Box<dyn pipe::Sink>)>;

    /// Notify a client of a connection failure
    fn fail_request(self: Box<Self>, error: io::Error) -> io::Result<()>;
}

pub(crate) enum DatagramPipeHalves {
    Udp(Box<dyn datagram_pipe::Source<Output = UdpDatagram>>, Box<dyn datagram_pipe::Sink<Input = forwarder::UdpDatagram>>),
    Icmp(Box<dyn datagram_pipe::Source<Output = IcmpDatagram>>, Box<dyn datagram_pipe::Sink<Input = forwarder::IcmpDatagram>>),
}

/// An abstract interface for a datagram multiplexer open request implementation
pub(crate) trait PendingDatagramMultiplexerRequest: StreamId + Send {
    /// Notify a client of the successfully opened multiplexer
    fn succeed_request(self: Box<Self>) -> io::Result<DatagramPipeHalves>;

    /// Notify a client of a multiplexer open failure
    fn fail_request(self: Box<Self>, error: io::Error) -> io::Result<()>;
}

/// An abstract interface for a downstream implementation which communicates with a client
#[async_trait]
pub(crate) trait Downstream: Send {
    /// Listen to events on the client-side.
    /// Returns `None` in case the listening finished gracefully and should not be continued,
    /// `Some` in case the downstream encountered the new authorization request which should be
    /// processed and listening should be continued.
    async fn listen(&mut self) -> io::Result<Option<Box<dyn AuthorizationRequest>>>;
}

impl Debug for UdpDatagram {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "meta={:?}, payload={}B", self.meta, self.payload.len())
    }
}
