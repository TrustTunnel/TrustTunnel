use crate::forwarder::TcpConnector;
use crate::metrics::OutboundTcpSocketCounter;
use crate::net_utils::TcpDestination;
use crate::{core, forwarder, log_id, log_utils, net_utils, pipe, tunnel};
use async_trait::async_trait;
use bytes::{Buf, Bytes};
use std::io;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

pub(crate) struct TcpForwarder {
    context: Arc<core::Context>,
}

struct StreamRx {
    rx: OwnedReadHalf,
    id: log_utils::IdChain<u64>,
    _metrics_guard: OutboundTcpSocketCounter,
}

struct StreamTx {
    tx: OwnedWriteHalf,
    /// This is a workaround for the fact that half-closed connections are often not
    /// supported in the wild. For example,
    /// nginx https://mailman.nginx.org/pipermail/nginx/2008-September/007388.html, or
    /// golang https://github.com/golang/go/issues/18527.
    eof_pending: bool,
    id: log_utils::IdChain<u64>,
}

impl TcpForwarder {
    pub fn new(context: Arc<core::Context>) -> Self {
        Self { context }
    }

    pub(crate) fn pipe_from_stream(
        stream: TcpStream,
        id: log_utils::IdChain<u64>,
        metrics_guard: OutboundTcpSocketCounter,
    ) -> (Box<dyn pipe::Source>, Box<dyn pipe::Sink>) {
        let (rx, tx) = stream.into_split();
        (
            Box::new(StreamRx {
                rx,
                id: id.clone(),
                _metrics_guard: metrics_guard,
            }),
            Box::new(StreamTx {
                tx,
                eof_pending: false,
                id,
            }),
        )
    }
}

#[async_trait]
impl TcpConnector for TcpForwarder {
    async fn connect(
        self: Box<Self>,
        id: log_utils::IdChain<u64>,
        meta: forwarder::TcpConnectionMeta,
    ) -> Result<(Box<dyn pipe::Source>, Box<dyn pipe::Sink>), tunnel::ConnectionError> {
        let peer = match meta.destination {
            TcpDestination::Address(peer) => {
                let peer_ip = peer.ip();
                if !self.context.settings.allow_private_network_connections
                    && !net_utils::is_global_ip(&peer_ip)
                {
                    if peer_ip.is_loopback() {
                        return Err(tunnel::ConnectionError::DnsLoopback);
                    }
                    return Err(tunnel::ConnectionError::DnsNonroutable);
                }

                peer
            }
            TcpDestination::HostName(peer) => {
                log_id!(trace, id, "Resolving peer: {:?}", peer);

                let resolved = tokio::net::lookup_host(format!("{}:{}", peer.0, peer.1))
                    .await
                    .map_err(io_to_connection_error)?;

                enum SelectionStatus {
                    Loopback,
                    NonRoutable,
                    Suitable(SocketAddr),
                }

                let mut status = None;
                for a in resolved {
                    let ip = a.ip();
                    if ip.is_ipv6() && !self.context.settings.ipv6_available {
                        continue;
                    }

                    if net_utils::is_global_ip(&ip)
                        || self.context.settings.allow_private_network_connections
                    {
                        status = Some(SelectionStatus::Suitable(a));
                        break;
                    }

                    if status.is_none() && ip.is_loopback() {
                        status = Some(SelectionStatus::Loopback);
                        continue;
                    }

                    status = Some(SelectionStatus::NonRoutable);
                }

                match status {
                    None => {
                        return Err(io_to_connection_error(io::Error::new(
                            ErrorKind::Other,
                            "Resolved to empty list",
                        )))
                    }
                    Some(SelectionStatus::Loopback) => {
                        return Err(tunnel::ConnectionError::DnsLoopback)
                    }
                    Some(SelectionStatus::NonRoutable) => {
                        return Err(tunnel::ConnectionError::DnsNonroutable)
                    }
                    Some(SelectionStatus::Suitable(x)) => {
                        log_id!(trace, id, "Selected address: {}", x);
                        x
                    }
                }
            }
        };

        log_id!(trace, id, "Connecting to peer: {}", peer);
        let metrics_guard = self.context.metrics.clone().outbound_tcp_socket_counter();
        TcpStream::connect(peer)
            .await
            .and_then(|s| {
                s.set_nodelay(true)?;
                Ok(s)
            })
            .map(|s| {
                if let Ok(local_addr) = s.local_addr() {
                    log_id!(
                        trace,
                        id,
                        "Connection established, local port: {}",
                        local_addr.port()
                    );
                }
                TcpForwarder::pipe_from_stream(s, id, metrics_guard)
            })
            .map_err(io_to_connection_error)
    }
}

#[async_trait]
impl pipe::Source for StreamRx {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }

    async fn read(&mut self) -> io::Result<pipe::Data> {
        const READ_CHUNK_SIZE: usize = 64 * 1024;
        let mut buffer = Vec::with_capacity(READ_CHUNK_SIZE);

        loop {
            match self.rx.read_buf(&mut buffer).await {
                Ok(0) => break Ok(pipe::Data::Eof),
                Ok(_) => break Ok(pipe::Data::Chunk(Bytes::from(buffer))),
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue,
                Err(e) => break Err(e),
            }
        }
    }

    fn consume(&mut self, _size: usize) -> io::Result<()> {
        // do nothing
        Ok(())
    }
}

#[async_trait]
impl pipe::Sink for StreamTx {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }

    fn write(&mut self, mut data: Bytes) -> io::Result<Bytes> {
        if self.eof_pending {
            return Err(io::Error::new(
                ErrorKind::Other,
                "Already shut down".to_string(),
            ));
        }

        while !data.is_empty() {
            match self.tx.try_write(data.as_ref()) {
                Ok(n) => data.advance(n),
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }

        Ok(data)
    }

    fn eof(&mut self) -> io::Result<()> {
        // Mark eof as pending but do not close the connection yet, it will
        // be done in flush.
        self.eof_pending = true;
        Ok(())
    }

    async fn wait_writable(&mut self) -> io::Result<()> {
        if self.eof_pending {
            return Err(io::Error::new(
                ErrorKind::Other,
                "Already shut down".to_string(),
            ));
        }

        self.tx.writable().await
    }

    async fn flush(&mut self) -> io::Result<()> {
        self.tx.flush().await?;
        if self.eof_pending {
            self.tx.shutdown().await?;
        }
        Ok(())
    }
}

fn io_to_connection_error(error: io::Error) -> tunnel::ConnectionError {
    // for now, corresponding ErrorKind's are not stable
    if error.raw_os_error() == Some(libc::ENETUNREACH)
        || error.raw_os_error() == Some(libc::EHOSTUNREACH)
    {
        return tunnel::ConnectionError::HostUnreachable;
    }

    if error.kind() == ErrorKind::TimedOut {
        return tunnel::ConnectionError::Timeout;
    }

    tunnel::ConnectionError::Io(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};

    fn make_test_context_disallow_private_network() -> Arc<core::Context> {
        let mut ctx = core::Context::default();
        Arc::get_mut(&mut ctx.settings)
            .unwrap()
            .allow_private_network_connections = false;
        Arc::new(ctx)
    }

    #[tokio::test]
    async fn test_connect_denies_loopback_address_when_private_network_disallowed() {
        let context = make_test_context_disallow_private_network();
        let connector: Box<dyn TcpConnector> = Box::new(TcpForwarder::new(context));

        let meta = forwarder::TcpConnectionMeta {
            client_address: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
            destination: TcpDestination::Address(SocketAddr::from((Ipv4Addr::LOCALHOST, 22))),
            auth: None,
            tls_domain: String::new(),
            user_agent: None,
        };

        let err = match connector.connect(log_utils::IdChain::empty(), meta).await {
            Ok(_) => panic!("Expected connection to be denied"),
            Err(e) => e,
        };

        assert!(matches!(err, tunnel::ConnectionError::DnsLoopback));
    }

    #[tokio::test]
    async fn test_connect_denies_private_address_when_private_network_disallowed() {
        let context = make_test_context_disallow_private_network();
        let connector: Box<dyn TcpConnector> = Box::new(TcpForwarder::new(context));

        let meta = forwarder::TcpConnectionMeta {
            client_address: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
            destination: TcpDestination::Address(SocketAddr::from((
                Ipv4Addr::new(192, 168, 0, 1),
                80,
            ))),
            auth: None,
            tls_domain: String::new(),
            user_agent: None,
        };

        let err = match connector.connect(log_utils::IdChain::empty(), meta).await {
            Ok(_) => panic!("Expected connection to be denied"),
            Err(e) => e,
        };

        assert!(matches!(err, tunnel::ConnectionError::DnsNonroutable));
    }

    #[tokio::test]
    async fn test_connect_denies_ipv4_mapped_private_address() {
        let context = make_test_context_disallow_private_network();
        let connector: Box<dyn TcpConnector> = Box::new(TcpForwarder::new(context));

        // ::ffff:192.168.0.1 — IPv4-mapped private address
        let mapped_private = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc0a8, 0x0001);
        let meta = forwarder::TcpConnectionMeta {
            client_address: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
            destination: TcpDestination::Address(SocketAddr::V6(SocketAddrV6::new(
                mapped_private,
                80,
                0,
                0,
            ))),
            auth: None,
            tls_domain: String::new(),
            user_agent: None,
        };

        let err = match connector.connect(log_utils::IdChain::empty(), meta).await {
            Ok(_) => panic!("Expected connection to be denied"),
            Err(e) => e,
        };

        assert!(matches!(err, tunnel::ConnectionError::DnsNonroutable));
    }

    #[tokio::test]
    async fn test_connect_denies_ipv4_mapped_loopback() {
        let context = make_test_context_disallow_private_network();
        let connector: Box<dyn TcpConnector> = Box::new(TcpForwarder::new(context));

        // ::ffff:127.0.0.1 — IPv4-mapped loopback
        let mapped_loopback = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001);
        let meta = forwarder::TcpConnectionMeta {
            client_address: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
            destination: TcpDestination::Address(SocketAddr::V6(SocketAddrV6::new(
                mapped_loopback,
                22,
                0,
                0,
            ))),
            auth: None,
            tls_domain: String::new(),
            user_agent: None,
        };

        let err = match connector.connect(log_utils::IdChain::empty(), meta).await {
            Ok(_) => panic!("Expected connection to be denied"),
            Err(e) => e,
        };

        // IPv4-mapped loopback is blocked as non-routable (the V6 wrapper means
        // IpAddr::is_loopback() returns false, so it hits DnsNonroutable rather
        // than DnsLoopback — but the connection is denied either way)
        assert!(matches!(err, tunnel::ConnectionError::DnsNonroutable));
    }
}
