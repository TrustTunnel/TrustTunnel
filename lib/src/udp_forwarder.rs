use crate::forwarder::UdpMultiplexer;
use crate::metrics::OutboundUdpSocketCounter;
use crate::{core, datagram_pipe, downstream, forwarder, log_utils, net_utils};
use async_trait::async_trait;
use bytes::{Buf, Bytes, BytesMut};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;



struct Connection {
    socket: Arc<UdpSocket>,
    _task: JoinHandle<()>,
    #[allow(dead_code)]
    being_listened: bool,
    _metrics_guard: OutboundUdpSocketCounter,
}

type Connections = HashMap<forwarder::UdpDatagramMeta, Connection>;

struct MultiplexerShared {
    connections: RwLock<Connections>,
    context: Arc<core::Context>,
    packet_tx: mpsc::Sender<InternalEvent>,
}

struct MultiplexerSource {
    packet_rx: mpsc::Receiver<InternalEvent>,
    pending_closures: VecDeque<(forwarder::UdpDatagramMeta, io::Error)>,
    parent_id_chain: log_utils::IdChain<u64>,
}

struct MultiplexerSink {
    shared: Arc<MultiplexerShared>,
    wake_tx: mpsc::Sender<()>,
}

enum InternalEvent {
    Payload(forwarder::UdpDatagramMeta, Bytes),
    Error(forwarder::UdpDatagramMeta, io::Error),
}

pub(crate) fn make_multiplexer(
    context: Arc<core::Context>,
    id: log_utils::IdChain<u64>,
) -> io::Result<UdpMultiplexer> {
    let (packet_tx, packet_rx) = mpsc::channel(32768);
    let (wake_tx, _wake_rx) = mpsc::channel(1);

    let shared = Arc::new(MultiplexerShared {
        connections: RwLock::new(Default::default()),
        context,
        packet_tx,
    });

    Ok((
        shared.clone(),
        Box::new(MultiplexerSource {
            packet_rx,
            pending_closures: Default::default(),
            parent_id_chain: id,
        }),
        Box::new(MultiplexerSink { shared, wake_tx }),
    ))
}

impl MultiplexerSource {
    fn handle_event(&mut self, event: InternalEvent) -> Option<forwarder::UdpDatagramReadStatus> {
        match event {
            InternalEvent::Payload(meta, payload) => {
                if std::env::var("TRUSTTUNNEL_GRO_DEBUG").unwrap_or_default() == "true" {
                    log::info!("Dispatcher: Received payload: len={} meta={:?}", payload.len(), meta);
                }
                Some(forwarder::UdpDatagramReadStatus::Read(
                    forwarder::UdpDatagram {
                        meta: meta.reversed(),
                        payload,
                    },
                ))
            }
            InternalEvent::Error(meta, error) => {
                self.pending_closures.push_back((meta, error));
                None
            }
        }
    }
}

#[async_trait]
impl forwarder::UdpDatagramPipeShared for MultiplexerShared {
    async fn on_new_udp_connection(&self, meta: &downstream::UdpDatagramMeta) -> io::Result<()> {
        let ip = meta.destination.ip();
        if ip.is_ipv6() && !self.context.settings.ipv6_available {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "IPv6 connections are disabled",
            ));
        }

        if !net_utils::is_global_ip(&ip) && !self.context.settings.allow_private_network_connections {
            // Special case: allow benchmarking range (198.18.0.0/15) as it is often used for internal DNS proxies
            let octets = match ip {
                std::net::IpAddr::V4(v4) => v4.octets(),
                _ => [0, 0, 0, 0],
            };
            if !(octets[0] == 198 && (octets[1] & 0xfe) == 18) {
                log::warn!("Dropping UDP connection to non-global IP: {} (allow_private_network_connections is false)", ip);
                return Err(io::Error::new(
                    ErrorKind::PermissionDenied,
                    "Connections to private network are disabled",
                ));
            }
        }

        let mut connections = self.connections.write().unwrap();
        let key = forwarder::UdpDatagramMeta::from(meta);
        
        match connections.entry(key) {
            Entry::Occupied(_) => Ok(()), // Connection already exists, that's fine - reuse it
            Entry::Vacant(e) => {
                let metrics_guard = self.context.metrics.clone().outbound_udp_socket_counter();

                let socket = Arc::new(make_udp_socket(&meta.destination)?);
                let socket_clone = socket.clone();
                let packet_tx = self.packet_tx.clone();
                let meta_copy = key;

                let task = tokio::spawn(async move {
                    // Standard buffer size for UDP forwarding
                    const RECV_BUFFER_SIZE: usize = 65536;
                    let mut buffer = BytesMut::with_capacity(RECV_BUFFER_SIZE);
                    
                    loop {
                        if buffer.capacity() < RECV_BUFFER_SIZE {
                            buffer.reserve(RECV_BUFFER_SIZE);
                        }
                        
                        match socket_clone.recv_buf(&mut buffer).await {
                            Ok(_) => {
                                let payload = buffer.split().freeze();
                                if packet_tx.send(InternalEvent::Payload(meta_copy, payload)).await.is_err() {
                                    break; // Channel closed
                                }
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::ConnectionReset => {
                                // ConnectionReset is common for UDP on Windows (ICMP Port Unreachable).
                                // We should just ignore it and continue listening.
                                log::debug!("UDP socket received ConnectionReset (ICMP Port Unreachable), ignoring.");
                                continue;
                            }
                            Err(e) => {
                                let _ = packet_tx.send(InternalEvent::Error(meta_copy, e)).await;
                                break;
                            }
                        }
                    }
                });
                e.insert(Connection {
                    socket,
                    _task: task,
                    being_listened: false,
                    _metrics_guard: metrics_guard,
                });
                Ok(())
            }
        }
    }

    fn on_connection_closed(&self, meta: &forwarder::UdpDatagramMeta) {
        if let Some(conn) = self.connections.write().unwrap().remove(meta) {
            conn._task.abort();
        }
    }
}

#[async_trait]
impl datagram_pipe::Source for MultiplexerSource {
    type Output = forwarder::UdpDatagramReadStatus;

    fn id(&self) -> log_utils::IdChain<u64> {
        self.parent_id_chain.clone()
    }

    async fn read(&mut self) -> io::Result<forwarder::UdpDatagramReadStatus> {
        if std::env::var("TRUSTTUNNEL_GRO_DEBUG").unwrap_or_default() == "true" {
            log::info!("MultiplexerSource::read() ENTERED - waiting for packet from channel");
        }
        loop {
            if let Some((meta, error)) = self.pending_closures.pop_front() {
                return Ok(forwarder::UdpDatagramReadStatus::UdpClose(meta, error));
            }

            match self.packet_rx.recv().await {
                Some(event) => {
                   if let Some(status) = self.handle_event(event) {
                       return Ok(status);
                   }
                }
                None => {
                    return Err(io::Error::from(ErrorKind::UnexpectedEof));
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
        let socket = {
            let connections = self.shared.connections.read().unwrap();
            connections.get(&meta).map(|c| c.socket.clone())
        };

        if let Some(socket) = socket {
            socket.send(datagram.payload.as_ref()).await?;
            Ok(datagram_pipe::SendStatus::Sent)
        } else {
            Err(io::Error::from(ErrorKind::NotFound))
        }
    }
}

fn make_udp_socket(peer: &SocketAddr) -> io::Result<UdpSocket> {
    let socket = net_utils::make_udp_socket(peer.is_ipv4())?;
    socket.connect(peer)?;
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket)
}
