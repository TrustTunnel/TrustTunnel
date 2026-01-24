use crate::{datagram_pipe, downstream, forwarder, log_id, log_utils, net_utils, pipe};
use async_trait::async_trait;
use futures::future;
use futures::future::Either;
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::time::Instant;

pub(crate) struct DuplexPipe<F: Send + Sync> {
    left_pipe: LeftPipe<F>,
    right_pipe: RightPipe<F>,
    timeout: Duration,
}

/// Forwards UDP packets from a client to a target host
struct LeftPipe<F: Send + Sync> {
    source: Box<dyn datagram_pipe::Source<Output = downstream::UdpDatagram>>,
    sink: Box<dyn datagram_pipe::Sink<Input = downstream::UdpDatagram>>,
    shared: Arc<UdpPipeShared<F>>,
    direction: pipe::SimplexDirection,
    next_connection_id: std::ops::RangeFrom<u64>,
}

/// Forwards UDP packets from a target host to a client
struct RightPipe<F: Send + Sync> {
    source: Box<dyn datagram_pipe::Source<Output = forwarder::UdpDatagramReadStatus>>,
    sink: Box<dyn datagram_pipe::Sink<Input = forwarder::UdpDatagram>>,
    shared: Arc<UdpPipeShared<F>>,
    direction: pipe::SimplexDirection,
}

struct UdpPipeShared<F: Send + Sync> {
    udp_connections: RwLock<HashMap<forwarder::UdpDatagramMeta, Mutex<UdpConnection>>>,
    forwarder_shared: Arc<dyn forwarder::UdpDatagramPipeShared>,
    update_metrics: F,
}

struct UdpConnection {
    last_activity: Instant,
    plain_dns_info: Option<PlainDnsInfo>,
    log_id: log_utils::IdChain<u64>,
}

struct PlainDnsInfo {
    pending_queries: usize,
}

#[derive(Eq, PartialEq)]
enum UdpConnectionStatus {
    Continue,
    Done,
}

impl<F: Fn(pipe::SimplexDirection, usize) + Send + Sync> LeftPipe<F> {
    async fn exchange(&mut self) -> io::Result<()> {
        loop {
            if std::env::var("TRUSTTUNNEL_GRO_DEBUG").unwrap_or_default() == "true" {
                log::info!("LeftPipe: Waiting for packet from client...");
            }
            let datagram = self.source.read().await?;
            if std::env::var("TRUSTTUNNEL_GRO_DEBUG").unwrap_or_default() == "true" {
                log::info!("LeftPipe: Processing packet: {:?}", datagram.meta);
            }

            if let Err(e) = self.on_udp_packet(&datagram.meta).await {
                log_id!(
                    debug,
                    self.source.id(),
                    "--> Dropping UDP packet due to error: datagram={:?}, error={}",
                    datagram,
                    e
                );
                continue;
            }

            let datagram_len = datagram.payload.len();
            match self.sink.write(datagram).await? {
                datagram_pipe::SendStatus::Sent => {
                    (self.shared.update_metrics)(self.direction, datagram_len);
                }
                datagram_pipe::SendStatus::Dropped => {
                    log_id!(trace, self.source.id(), "--> Datagram dropped")
                }
            }
        }
    }

    async fn on_udp_packet(&mut self, meta: &downstream::UdpDatagramMeta) -> io::Result<()> {
        let key = forwarder::UdpDatagramMeta::from(meta);
        
        // Fast path: try to find existing connection with read lock
        if let Some(conn) = self.shared.udp_connections.read().unwrap().get(&key) {
            conn.lock().unwrap().register_outgoing_packet();
            return Ok(());
        }

        // Slow path: insert new connection with write lock
        {
            let mut connections = self.shared.udp_connections.write().unwrap();
            // Check again in case another thread inserted it
            if let Some(conn) = connections.get(&key) {
                conn.lock().unwrap().register_outgoing_packet();
                return Ok(());
            }

            let is_plain_dns = meta.destination.port() == net_utils::PLAIN_DNS_PORT_NUMBER;
            connections.insert(
                key,
                Mutex::new(UdpConnection {
                    last_activity: Instant::now(),
                    plain_dns_info: is_plain_dns.then_some(PlainDnsInfo { pending_queries: 0 }),
                    log_id: self.source.id().extended(log_utils::IdItem::new(
                        log_utils::CONNECTION_ID_FMT,
                        self.next_connection_id.next().unwrap(),
                    )),
                }),
            );
        } // Lock is explicitly dropped here before await

        self.shared
            .forwarder_shared
            .on_new_udp_connection(meta)
            .await?;

        // Re-acquire read lock to update timestamp just in case
        if let Some(c) = self
            .shared
            .udp_connections
            .read()
            .unwrap()
            .get(&key)
        {
            c.lock().unwrap().register_outgoing_packet()
        }
        Ok(())
    }
}

impl<F: Fn(pipe::SimplexDirection, usize) + Send + Sync> RightPipe<F> {
    async fn exchange(&mut self) -> io::Result<()> {
        loop {
            let datagram = match self.source.read().await? {
                forwarder::UdpDatagramReadStatus::Read(x) => {
                    if std::env::var("TRUSTTUNNEL_GRO_DEBUG").unwrap_or_default() == "true" {
                        log::info!("RightPipe: Read packet from dispatcher: len={}", x.payload.len());
                    }
                    x
                }
                forwarder::UdpDatagramReadStatus::UdpClose(meta, e) => {
                    if let Some(c) = self.shared.udp_connections.write().unwrap().remove(&meta) {
                        log_id!(
                            debug,
                            c.lock().unwrap().log_id,
                            "Connection closed: meta={:?} error={}",
                            meta,
                            e
                        );
                        // Ensure forwarder also cleans up its task/socket
                        self.shared.forwarder_shared.on_connection_closed(&meta);
                    }
                    continue;
                }
            };
            log_id!(trace, self.source.id(), "<-- Datagram: {:?}", datagram);

            let meta = datagram.meta;
            let mut datagram = datagram;
            let datagram_len = datagram.payload.len();
            match self.sink.write(datagram).await? {
                datagram_pipe::SendStatus::Sent => {
                    log::info!("RightPipe: Wrote packet to downstream sink: len={}", datagram_len);
                    (self.shared.update_metrics)(self.direction, datagram_len);
                }
                datagram_pipe::SendStatus::Dropped => {
                    log::warn!("RightPipe: Packet DROPPED by downstream sink: len={}", datagram_len);
                }
            }

            let x = self.on_udp_packet(&meta.reversed());
            match x {
                UdpConnectionStatus::Continue => (),
                UdpConnectionStatus::Done => {
                    let key = meta.reversed();
                    if let Some(c) = self
                        .shared
                        .udp_connections
                        .write()
                        .unwrap()
                        .remove(&key)
                    {
                        log_id!(debug, c.lock().unwrap().log_id, "All UDP queries are completed");
                    }
                    self.shared.forwarder_shared.on_connection_closed(&key);
                }
            }
        }
    }

    fn on_udp_packet(&mut self, meta: &forwarder::UdpDatagramMeta) -> UdpConnectionStatus {
        match self.shared.udp_connections.read().unwrap().get(meta) {
            None => UdpConnectionStatus::Continue,
            Some(conn) => conn.lock().unwrap().register_incoming_packet(),
        }
    }
}

impl UdpConnection {
    fn register_outgoing_packet(&mut self) {
        self.last_activity = Instant::now();
        if let Some(info) = self.plain_dns_info.as_mut() {
            info.pending_queries += 1;
        }
    }

    fn register_incoming_packet(&mut self) -> UdpConnectionStatus {
        self.last_activity = Instant::now();
        self.plain_dns_info
            .as_mut()
            .map_or(UdpConnectionStatus::Continue, |info| {
                info.pending_queries = info.pending_queries.saturating_sub(1);
                if info.pending_queries == 0 {
                    UdpConnectionStatus::Done
                } else {
                    UdpConnectionStatus::Continue
                }
            })
    }
}

impl<F: Fn(pipe::SimplexDirection, usize) + Send + Sync> DuplexPipe<F> {
    #[allow(clippy::type_complexity)]
    pub fn new(
        (source1, sink1): (
            Box<dyn datagram_pipe::Source<Output = downstream::UdpDatagram>>,
            Box<dyn datagram_pipe::Sink<Input = forwarder::UdpDatagram>>,
        ),
        (shared2, source2, sink2): (
            Arc<dyn forwarder::UdpDatagramPipeShared>,
            Box<dyn datagram_pipe::Source<Output = forwarder::UdpDatagramReadStatus>>,
            Box<dyn datagram_pipe::Sink<Input = downstream::UdpDatagram>>,
        ),
        update_metrics: F,
        timeout: Duration,
    ) -> Self {
        let shared = Arc::new(UdpPipeShared {
            udp_connections: RwLock::new(Default::default()),
            forwarder_shared: shared2,
            update_metrics,
        });

        Self {
            left_pipe: LeftPipe {
                source: source1,
                sink: sink2,
                shared: shared.clone(),
                direction: pipe::SimplexDirection::Outgoing,
                next_connection_id: 0..,
            },
            right_pipe: RightPipe {
                source: source2,
                sink: sink1,
                shared,
                direction: pipe::SimplexDirection::Incoming,
            },
            timeout,
        }
    }

    async fn exchange_once(&mut self) -> io::Result<()> {
        let left = self.left_pipe.exchange();
        futures::pin_mut!(left);
        let right = self.right_pipe.exchange();
        futures::pin_mut!(right);
        match future::try_select(left, right).await {
            Ok(_) => Ok(()),
            Err(Either::Left((e, _))) | Err(Either::Right((e, _))) => Err(e),
        }
    }

    fn on_timer_tick(&mut self) {
        let last_unexpired_timestamp = Instant::now() - self.timeout;

        let mut connections = self.left_pipe.shared.udp_connections.write().unwrap();
        // Since we need to remove, we need write lock. 
        // We can't use `retain` easily if we want to log the removed ones with access to internal id
        // Actually we can, but let's stick to simple logic.
        // We need to iterate and identify expired keys.
        let expired: Vec<_> = connections
            .iter()
            .filter(|(_, conn)| conn.lock().unwrap().last_activity < last_unexpired_timestamp)
            .map(|(meta, c)| (*meta, c.lock().unwrap().log_id.clone()))
            .collect();

        for (meta, id) in expired {
            connections.remove(&meta);
            self.right_pipe
                .shared
                .forwarder_shared
                .on_connection_closed(&meta);
            log_id!(debug, id, "Connection expired: {:?}", meta);
        }
    }
}

#[async_trait]
impl<F: Fn(pipe::SimplexDirection, usize) + Send + Sync> datagram_pipe::DuplexPipe
    for DuplexPipe<F>
{
    async fn exchange(&mut self) -> io::Result<()> {
        let res = loop {
            match tokio::time::timeout(self.timeout / 4, self.exchange_once()).await {
                Ok(x) => break x,
                Err(_) => self.on_timer_tick(),
            }
        };

        // Explicitly clean up all remaining connections on exit
        let mut connections = self.left_pipe.shared.udp_connections.write().unwrap();
        let remaining: Vec<_> = connections.keys().cloned().collect();
        for meta in remaining {
            if let Some(c) = connections.remove(&meta) {
                log_id!(debug, c.lock().unwrap().log_id, "Cleaning up connection on pipe exit: {:?}", meta);
            }
            self.right_pipe.shared.forwarder_shared.on_connection_closed(&meta);
        }

        res
    }
}
