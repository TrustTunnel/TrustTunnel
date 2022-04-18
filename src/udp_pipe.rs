use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use async_trait::async_trait;
use futures::future;
use futures::future::Either;
use tokio::time::Instant;
use crate::{datagram_pipe, downstream, forwarder, log_id, log_utils, net_utils};


pub(crate) struct DuplexPipe {
    left_pipe: LeftPipe,
    right_pipe: RightPipe,
    timeout: Duration,
}

/// Forwards UDP packets from a client to a target host
struct LeftPipe {
    source: Box<dyn datagram_pipe::Source<Output = downstream::UdpDatagram>>,
    sink: Box<dyn datagram_pipe::Sink<Input = downstream::UdpDatagram>>,
    shared: Arc<UdpPipeShared>,
    next_connection_id: std::ops::RangeFrom<u64>,
}

/// Forwards UDP packets from a target host to a client
struct RightPipe {
    source: Box<dyn datagram_pipe::Source<Output = forwarder::UdpDatagramReadStatus>>,
    sink: Box<dyn datagram_pipe::Sink<Input = forwarder::UdpDatagram>>,
    shared: Arc<UdpPipeShared>,
}

struct UdpPipeShared {
    udp_connections: Mutex<HashMap<forwarder::UdpDatagramMeta, UdpConnection>>,
    forwarder_shared: Arc<dyn forwarder::UdpDatagramPipeShared>,
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


impl LeftPipe {
    async fn exchange(&mut self) -> io::Result<()> {
        loop {
            let datagram = self.source.read().await?;
            log_id!(trace, self.source.id(), "--> Datagram: {:?}", datagram);

            if let Err(e) = self.on_udp_packet(&datagram.meta).await {
                log_id!(debug, self.source.id(),
                    "--> Dropping UDP packet due to error: datagram={:?}, error={}",
                    datagram, e
                );
                continue;
            }

            match self.sink.write(datagram).await? {
                datagram_pipe::SendStatus::Sent => log_id!(debug, self.source.id(), "--> Datagram sent"),
                datagram_pipe::SendStatus::Dropped => log_id!(debug, self.source.id(), "--> Datagram dropped"),
            }
        }
    }

    async fn on_udp_packet(&mut self, meta: &downstream::UdpDatagramMeta) -> io::Result<()> {
        if let Some(conn) = self.shared.udp_connections.lock().unwrap()
            .get_mut(&forwarder::UdpDatagramMeta::from(meta))
        {
            conn.register_outgoing_packet();
            return Ok(());
        }

        let is_plain_dns = meta.destination.port() == net_utils::PLAIN_DNS_PORT_NUMBER;
        self.shared.udp_connections.lock().unwrap().insert(
            forwarder::UdpDatagramMeta::from(meta),
            UdpConnection {
                last_activity: Instant::now(),
                plain_dns_info: is_plain_dns.then(|| PlainDnsInfo {
                    pending_queries: 0,
                }),
                log_id: self.source.id().extended(log_utils::IdItem::new(
                    log_utils::CONNECTION_ID_FMT, self.next_connection_id.next().unwrap()
                )),
            }
        );

        self.shared.forwarder_shared.on_new_udp_connection(meta).await?;

        self.shared.udp_connections.lock().unwrap()
            .get_mut(&forwarder::UdpDatagramMeta::from(meta))
            .map(|c| c.register_outgoing_packet());
        Ok(())
    }
}

impl RightPipe {
    async fn exchange(&mut self) -> io::Result<()> {
        loop {
            let datagram = match { let x = self.source.read().await?; x } {
                forwarder::UdpDatagramReadStatus::Read(x) => x,
                forwarder::UdpDatagramReadStatus::UdpClose(meta, e) => {
                    if let Some(c) = self.shared.udp_connections.lock().unwrap().remove(&meta) {
                        log_id!(debug, c.log_id, "Connection closed: meta={:?} error={}", meta, e);
                    }
                    continue;
                }
            };
            log_id!(trace, self.source.id(), "<-- Datagram: {:?}", datagram);

            let meta = datagram.meta;
            match self.sink.write(datagram).await? {
                datagram_pipe::SendStatus::Sent => (),
                datagram_pipe::SendStatus::Dropped => log_id!(debug, self.source.id(), "<-- Datagram dropped"),
            }

            let reversed = meta.reversed();
            let x = self.on_udp_packet(&reversed);
            match x {
                UdpConnectionStatus::Continue => (),
                UdpConnectionStatus::Done => {
                    if let Some(c) = self.shared.udp_connections.lock().unwrap().remove(&reversed) {
                        log_id!(debug, c.log_id, "All UDP queries are completed");
                    }
                    self.shared.forwarder_shared.on_connection_closed(&meta);
                }
            }
        }
    }

    fn on_udp_packet(&mut self, meta: &forwarder::UdpDatagramMeta) -> UdpConnectionStatus {
        match self.shared.udp_connections.lock().unwrap().get_mut(meta) {
            None => UdpConnectionStatus::Continue,
            Some(conn) => conn.register_incoming_packet(),
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
        self.plain_dns_info.as_mut()
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

impl DuplexPipe {
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
        timeout: Duration,
    ) -> Self {
        let shared = Arc::new(UdpPipeShared {
            udp_connections: Mutex::new(Default::default()),
            forwarder_shared: shared2,
        });

        Self {
            left_pipe: LeftPipe {
                source: source1,
                sink: sink2,
                shared: shared.clone(),
                next_connection_id: 0..,
            },
            right_pipe: RightPipe {
                source: source2,
                sink: sink1,
                shared,
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

        let mut connections = self.left_pipe.shared.udp_connections.lock().unwrap();
        let expired: Vec<_> = connections.iter()
            .filter(|(_, conn)| conn.last_activity < last_unexpired_timestamp)
            .map(|(meta, c)| (*meta, c.log_id.clone()))
            .collect();

        for (meta, id) in expired {
            connections.remove(&meta);
            self.right_pipe.shared.forwarder_shared.on_connection_closed(&meta);
            log_id!(debug, id, "Connection expired: {:?}", meta);
        }
    }
}

#[async_trait]
impl datagram_pipe::DuplexPipe for DuplexPipe {
    async fn exchange(&mut self) -> io::Result<()> {
        loop {
            match tokio::time::timeout(self.timeout / 4, self.exchange_once()).await {
                Ok(x) => return x,
                Err(_) => self.on_timer_tick(),
            }
        }
    }
}
