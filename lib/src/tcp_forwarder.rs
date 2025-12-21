use crate::forwarder::TcpConnector;
use crate::net_utils::TcpDestination;
use crate::settings::Settings;
use crate::{forwarder, log_id, log_utils, net_utils, pipe, tunnel};
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
    core_settings: Arc<Settings>,
}

struct StreamRx {
    rx: OwnedReadHalf,
    id: log_utils::IdChain<u64>,
}

struct StreamTx {
    tx: OwnedWriteHalf,
    /// This is a workaround for the fact that half-closed connections are often not
    /// supported in the wild. For example,
    /// nginx https://mailman.nginx.org/pipermail/nginx/2008-September/007388.html, or
    /// golang https://github.com/golang/go/issues/18527.
    is_shut_down: bool,
    id: log_utils::IdChain<u64>,
}

impl TcpForwarder {
    pub fn new(core_settings: Arc<Settings>) -> Self {
        Self { core_settings }
    }

    pub fn pipe_from_stream(
        stream: TcpStream,
        id: log_utils::IdChain<u64>,
    ) -> (Box<dyn pipe::Source>, Box<dyn pipe::Sink>) {
        let (rx, tx) = stream.into_split();
        (
            Box::new(StreamRx { rx, id: id.clone() }),
            Box::new(StreamTx {
                tx,
                is_shut_down: false,
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
            TcpDestination::Address(peer) => peer,
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
                    if ip.is_ipv6() && !self.core_settings.ipv6_available {
                        continue;
                    }

                    if net_utils::is_global_ip(&ip)
                        || self.core_settings.allow_private_network_connections
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
        TcpStream::connect(peer)
            .await
            .and_then(|s| {
                s.set_nodelay(true)?;
                Ok(s)
            })
            .map(|s| TcpForwarder::pipe_from_stream(s, id))
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
        if self.is_shut_down {
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
        self.is_shut_down = true;
        Ok(())
    }

    async fn wait_writable(&mut self) -> io::Result<()> {
        if self.is_shut_down {
            return Err(io::Error::new(
                ErrorKind::Other,
                "Already shut down".to_string(),
            ));
        }

        self.tx.writable().await
    }

    async fn flush(&mut self) -> io::Result<()> {
        self.tx.flush().await
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
