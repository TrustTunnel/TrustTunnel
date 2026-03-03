use crate::forwarder::UdpMultiplexer;
use crate::metrics::OutboundUdpSocketCounter;
use crate::{core, datagram_pipe, downstream, forwarder, log_id, log_utils, net_utils};
use async_trait::async_trait;
use bytes::Bytes;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, LinkedList};
use std::io;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::net::UdpSocket;
use tokio::sync;

struct Connection {
    socket: Arc<UdpSocket>,
    being_listened: bool,
    _metrics_guard: OutboundUdpSocketCounter,
}

type Connections = HashMap<forwarder::UdpDatagramMeta, Connection>;

struct MultiplexerShared {
    connections: Mutex<Connections>,
    context: Arc<core::Context>,
}

struct MultiplexerSource {
    shared: Arc<MultiplexerShared>,
    wake_rx: sync::mpsc::Receiver<()>,
    pending_closures: LinkedList<(forwarder::UdpDatagramMeta, io::Error)>,
    parent_id_chain: log_utils::IdChain<u64>,
}

struct MultiplexerSink {
    shared: Arc<MultiplexerShared>,
    wake_tx: sync::mpsc::Sender<()>,
}

struct SocketError {
    meta: forwarder::UdpDatagramMeta,
    io: io::Error,
}

enum PollStatus {
    PendingRead(forwarder::UdpDatagramMeta),
    SocketError(SocketError),
}

pub(crate) fn make_multiplexer(
    context: Arc<core::Context>,
    id: log_utils::IdChain<u64>,
) -> io::Result<UdpMultiplexer> {
    let shared = Arc::new(MultiplexerShared {
        connections: Mutex::new(Default::default()),
        context,
    });
    let (wake_tx, wake_rx) = sync::mpsc::channel(1);

    Ok((
        shared.clone(),
        Box::new(MultiplexerSource {
            shared: shared.clone(),
            wake_rx,
            pending_closures: Default::default(),
            parent_id_chain: id,
        }),
        Box::new(MultiplexerSink { shared, wake_tx }),
    ))
}

async fn listen_socket_read(
    meta: forwarder::UdpDatagramMeta,
    socket: Arc<UdpSocket>,
) -> Result<forwarder::UdpDatagramMeta, SocketError> {
    socket
        .readable()
        .await
        .map(|_| meta)
        .map_err(|io| SocketError { meta, io })
}

impl MultiplexerSource {
    fn on_socket_error(&mut self, meta: &forwarder::UdpDatagramMeta, error: io::Error) {
        if self
            .shared
            .connections
            .lock()
            .unwrap()
            .remove(meta)
            .is_some()
        {
            self.pending_closures.push_back((*meta, error));
        }
    }

    fn read_pending_socket(
        &mut self,
        meta: &forwarder::UdpDatagramMeta,
    ) -> Option<forwarder::UdpDatagramReadStatus> {
        let socket = self
            .shared
            .connections
            .lock()
            .unwrap()
            .get(meta)
            .map(|conn| conn.socket.clone())?;

        let mut buffer = Vec::with_capacity(net_utils::MAX_UDP_PAYLOAD_SIZE);
        match socket.try_recv_buf(&mut buffer) {
            Ok(_) => Some(forwarder::UdpDatagramReadStatus::Read(
                forwarder::UdpDatagram {
                    meta: meta.reversed(),
                    payload: Bytes::from(buffer),
                },
            )),
            Err(e) if e.kind() == ErrorKind::WouldBlock => None,
            Err(e) => {
                self.on_socket_error(meta, e);
                None
            }
        }
    }

    async fn poll_events(&mut self) -> io::Result<Option<PollStatus>> {
        let futures = {
            type Future = Box<
                dyn futures::Future<Output = Result<forwarder::UdpDatagramMeta, SocketError>>
                    + Send,
            >;

            let connections = self.shared.connections.lock().unwrap();
            let mut futures: Vec<Pin<Future>> = Vec::with_capacity(1 + connections.len());
            // add always pending future to avoid a busy loop in case of connection absence
            futures.push(Box::pin(futures::future::pending()));
            for (meta, conn) in connections.deref() {
                futures.push(Box::pin(listen_socket_read(*meta, conn.socket.clone())));
            }
            futures
        };

        let wait_reads = futures::future::select_all(futures);
        tokio::pin!(wait_reads);

        let wait_waker = self.wake_rx.recv();
        tokio::pin!(wait_waker);

        tokio::select! {
            reads = wait_reads => match reads.0 {
                Ok(ready) => Ok(Some(PollStatus::PendingRead(ready))),
                Err(e) => {
                    log_id!(debug, self.parent_id_chain, "Error waiting for UDP read: meta={:?} error={}",
                        e.meta, e.io);
                    Ok(Some(PollStatus::SocketError(e)))
                }
            },
            r = wait_waker => match r {
                Some(_) => Ok(None),
                None => {
                    log_id!(debug, self.parent_id_chain, "Wake sender dropped");
                    Err(io::Error::from(ErrorKind::UnexpectedEof))
                }
            }
        }
    }
}

#[async_trait]
impl forwarder::UdpDatagramPipeShared for MultiplexerShared {
    async fn on_new_udp_connection(&self, meta: &downstream::UdpDatagramMeta) -> io::Result<()> {
        let dest_ip = meta.destination.ip();
        if !self.context.settings.allow_private_network_connections
            && !net_utils::is_global_ip(&dest_ip)
        {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "UDP destination is in a non-routable network",
            ));
        }

        match self
            .connections
            .lock()
            .unwrap()
            .entry(forwarder::UdpDatagramMeta::from(meta))
        {
            Entry::Occupied(_) => Err(io::Error::new(ErrorKind::Other, "Already present")),
            Entry::Vacant(e) => {
                let metrics_guard = self.context.metrics.clone().outbound_udp_socket_counter();
                e.insert(Connection {
                    socket: Arc::new(make_udp_socket(&meta.destination)?),
                    being_listened: false,
                    _metrics_guard: metrics_guard,
                });
                Ok(())
            }
        }
    }

    fn on_connection_closed(&self, meta: &forwarder::UdpDatagramMeta) {
        self.connections.lock().unwrap().remove(&meta.reversed());
    }
}

#[async_trait]
impl datagram_pipe::Source for MultiplexerSource {
    type Output = forwarder::UdpDatagramReadStatus;

    fn id(&self) -> log_utils::IdChain<u64> {
        self.parent_id_chain.clone()
    }

    async fn read(&mut self) -> io::Result<forwarder::UdpDatagramReadStatus> {
        loop {
            if let Some((meta, error)) = self.pending_closures.pop_front() {
                return Ok(forwarder::UdpDatagramReadStatus::UdpClose(meta, error));
            }

            match self.poll_events().await? {
                None => (),
                Some(PollStatus::PendingRead(meta)) => {
                    if let Some(x) = self.read_pending_socket(&meta) {
                        return Ok(x);
                    }
                }
                Some(PollStatus::SocketError(SocketError { meta, io })) => {
                    self.on_socket_error(&meta, io)
                }
            }
        }
    }
}

#[async_trait]
impl datagram_pipe::Sink for MultiplexerSink {
    type Input = downstream::UdpDatagram;

    async fn write(
        &mut self,
        datagram: downstream::UdpDatagram,
    ) -> io::Result<datagram_pipe::SendStatus> {
        let meta = forwarder::UdpDatagramMeta::from(&datagram.meta);
        let socket = self
            .shared
            .connections
            .lock()
            .unwrap()
            .get(&meta)
            .map(|c| c.socket.clone())
            .ok_or_else(|| io::Error::from(ErrorKind::NotFound))?;

        socket.send(datagram.payload.as_ref()).await?;

        if let Some(conn) = self.shared.connections.lock().unwrap().get_mut(&meta) {
            if !conn.being_listened {
                match self.wake_tx.try_send(()) {
                    Ok(_) | Err(sync::mpsc::error::TrySendError::Full(_)) => {
                        conn.being_listened = true;
                    }
                    Err(e) => {
                        return Err(io::Error::new(
                            ErrorKind::Other,
                            format!("Failed to wake up UDP listener task: {}", e),
                        ))
                    }
                }
            }
        }

        Ok(datagram_pipe::SendStatus::Sent)
    }
}

fn make_udp_socket(peer: &SocketAddr) -> io::Result<UdpSocket> {
    let socket = net_utils::make_udp_socket(peer.is_ipv4())?;
    socket.connect(peer)?;
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket)
}
