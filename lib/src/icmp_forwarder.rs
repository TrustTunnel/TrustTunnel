extern "C" {
    fn set_icmp_filter(fd: libc::c_int) -> libc::c_int;
    fn set_icmpv6_filter(fd: libc::c_int) -> libc::c_int;
}

use crate::forwarder::IcmpMultiplexer;
use crate::settings::Settings;
use crate::{datagram_pipe, downstream, forwarder, icmp_utils, log_utils, net_utils, utils};
use async_trait::async_trait;
use bytes::Bytes;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap, LinkedList};
use std::io;
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr};
use std::ops::Bound;
use std::os::unix::io::AsRawFd;
use std::sync::{Arc, Mutex};
use tokio::io::unix::AsyncFd;
use tokio::sync::{mpsc, RwLock};
use tokio::time::Instant;

pub(crate) struct IcmpForwarder {
    shared: Arc<ForwarderShared>,
    /// Waiting for notification from [`ForwarderShared::deadline_waker_tx`]
    deadline_waker_rx: Arc<tokio::sync::Notify>,
}

#[derive(Clone)]
struct ReplyWaiter {
    original_peer: IpAddr,
    waker_tx: mpsc::Sender<(IpAddr, icmp_utils::Message)>,
}

#[derive(Default)]
struct Listeners {
    reply_waiters: HashMap<icmp_utils::Echo, ReplyWaiter>,
    deadlines: BTreeMap<Instant, LinkedList<icmp_utils::Echo>>,
}

#[derive(Default)]
struct Sockets {
    v4: Option<RawPacketStream>,
    v6: Option<RawPacketStream>,
}

struct ForwarderShared {
    core_settings: Arc<Settings>,
    sockets: RwLock<Sockets>,
    listeners: Mutex<Listeners>,
    /// Wakes [`IcmpForwarder::deadline_waker_rx`]
    deadline_waker_tx: Arc<tokio::sync::Notify>,
}

struct PipeShared {
    forwarder_shared: Arc<ForwarderShared>,
}

struct IcmpSource {
    rx: mpsc::Receiver<(IpAddr, icmp_utils::Message)>,
    id: log_utils::IdChain<u64>,
}

struct IcmpSink {
    shared: Arc<PipeShared>,
    tx: mpsc::Sender<(IpAddr, icmp_utils::Message)>,
}

impl IcmpForwarder {
    pub fn new(core_settings: Arc<Settings>) -> Self {
        let deadline_waker = Arc::new(tokio::sync::Notify::new());
        Self {
            shared: Arc::new(ForwarderShared {
                core_settings,
                sockets: Default::default(),
                listeners: Default::default(),
                deadline_waker_tx: deadline_waker.clone(),
            }),
            deadline_waker_rx: deadline_waker,
        }
    }

    pub fn make_multiplexer(&self, id: log_utils::IdChain<u64>) -> io::Result<IcmpMultiplexer> {
        let (tx, rx) = mpsc::channel(
            self.shared
                .core_settings
                .icmp
                .as_ref()
                .unwrap()
                .recv_message_queue_capacity,
        );
        let shared = Arc::new(PipeShared {
            forwarder_shared: self.shared.clone(),
        });
        Ok((
            Box::new(IcmpSource { rx, id }),
            Box::new(IcmpSink { shared, tx }),
        ))
    }

    pub async fn listen(&self) -> io::Result<()> {
        self.init_sockets().await?;

        loop {
            let maintenance = self.maintain_listeners();
            tokio::pin!(maintenance);

            let wait_v4 = self.listen_v4();
            tokio::pin!(wait_v4);

            let wait_v6 = self.listen_v6();
            tokio::pin!(wait_v6);

            let (peer, reply) = tokio::select! {
                r = maintenance => match r {
                    Ok(_) => unreachable!(),
                    Err(e) => return Err(e),
                },
                r = wait_v4 => r?,
                r = wait_v6 => r?,
            };

            trace!("Received message: peer={} message={:?}", peer, &reply);
            let request = match reply.responded_echo_request() {
                None => {
                    debug!(
                        "Failed to extract echo request, dropping message: peer={}, message={:?}",
                        peer, reply
                    );
                    continue;
                }
                Some(x) => x,
            };

            let mut listeners = self.shared.listeners.lock().unwrap();
            match listeners.reply_waiters.get(&request) {
                None => {
                    debug!("Reply waiter not found: peer={}, reply={:?}", peer, reply);
                    continue;
                }
                Some(ReplyWaiter { waker_tx, .. }) => match waker_tx.try_send((peer, reply)) {
                    Ok(_) => (),
                    Err(mpsc::error::TrySendError::Closed((peer, message))) => {
                        debug!(
                            "Listener closed: peer={} request={:?} reply={:?}",
                            peer, request, message
                        );
                        listeners.reply_waiters.remove(&request);
                    }
                    Err(mpsc::error::TrySendError::Full((peer, message))) => {
                        debug!("Dropping message due to queue overflow: peer={} request={:?} reply={:?}",
                                peer, request, message);
                        listeners.reply_waiters.remove(&request);
                    }
                },
            }
        }
    }

    async fn init_sockets(&self) -> io::Result<()> {
        let mut sockets = self.shared.sockets.write().await;
        sockets.v4 = Some(RawPacketStream::new(
            libc::IPPROTO_ICMP,
            &self
                .shared
                .core_settings
                .icmp
                .as_ref()
                .unwrap()
                .interface_name,
        )?);

        sockets.v6 = if self.shared.core_settings.ipv6_available {
            Some(RawPacketStream::new(
                libc::IPPROTO_ICMPV6,
                &self
                    .shared
                    .core_settings
                    .icmp
                    .as_ref()
                    .unwrap()
                    .interface_name,
            )?)
        } else {
            None
        };

        Ok(())
    }

    async fn maintain_listeners(&self) -> io::Result<()> {
        loop {
            let closest_deadline = self
                .shared
                .listeners
                .lock()
                .unwrap()
                .deadlines
                .keys()
                .next()
                .cloned();
            match closest_deadline {
                None => self.deadline_waker_rx.notified().await,
                Some(x) => tokio::time::sleep_until(x).await,
            }

            let mut listeners = self.shared.listeners.lock().unwrap();
            let now = Instant::now();
            let expired: Vec<_> = listeners
                .deadlines
                .range((Bound::Unbounded, Bound::Included(now)))
                .map(|(k, _)| *k)
                .collect();
            for deadline in expired {
                if let Some(requests) = listeners.deadlines.remove(&deadline) {
                    for request in requests {
                        if let Some(waiter) = listeners.reply_waiters.remove(&request) {
                            debug!(
                                "Request expired: peer={} request={:?}",
                                waiter.original_peer, request
                            );
                        }
                    }
                }
            }
        }
    }

    async fn listen_v4(&self) -> io::Result<(IpAddr, icmp_utils::Message)> {
        loop {
            let (peer, packet) =
                Self::listen_socket(self.shared.sockets.read().await.v4.as_ref().unwrap()).await?;

            match icmp_utils::v4::Message::deserialize(packet.clone()) {
                Ok(x) => break Ok((peer, icmp_utils::Message::from(x))),
                Err(e) => {
                    debug!(
                        "Dropping malformed ICMPv4 message: {:?}, {}",
                        e,
                        utils::hex_dump(&packet)
                    );
                    continue;
                }
            }
        }
    }

    async fn listen_v6(&self) -> io::Result<(IpAddr, icmp_utils::Message)> {
        if let Some(socket) = self.shared.sockets.read().await.v6.as_ref() {
            loop {
                let (peer, packet) = Self::listen_socket(socket).await?;
                match icmp_utils::v6::Message::deserialize(packet) {
                    Ok(x) => break Ok((peer, icmp_utils::Message::from(x))),
                    Err(e) => {
                        debug!("Dropping malformed ICMPv6 message: {:?}", e);
                        continue;
                    }
                }
            }
        } else {
            futures::future::pending().await
        }
    }

    async fn listen_socket(sock: &RawPacketStream) -> io::Result<(IpAddr, Bytes)> {
        loop {
            let mut guard = sock.inner.readable().await?;
            let (peer, packet) = match guard.try_io(|x| net_utils::recv_from(x.as_raw_fd(), None)) {
                Ok(x) => x?,
                Err(_would_block) => continue,
            };
            trace!(
                "Received ICMP: peer={} bytes={:?}",
                peer,
                utils::hex_dump(&packet)
            );

            if peer.is_ipv4() {
                match net_utils::skip_ipv4_header(packet) {
                    None => debug!("Dropping ICMPv4 packet with invalid IP header"),
                    Some((proto, payload)) if proto == libc::IPPROTO_ICMP => {
                        return Ok((peer, payload))
                    }
                    Some((proto, payload)) => debug!(
                        "Dropping non-ICMP packet: proto={}, payload={}",
                        proto,
                        utils::hex_dump(&payload)
                    ),
                }
            } else {
                return Ok((peer, packet));
            }
        }
    }
}

#[async_trait]
impl datagram_pipe::Source for IcmpSource {
    type Output = forwarder::IcmpDatagram;

    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }

    async fn read(&mut self) -> io::Result<forwarder::IcmpDatagram> {
        let (peer, message) = self
            .rx
            .recv()
            .await
            .ok_or_else(|| io::Error::from(ErrorKind::UnexpectedEof))?;

        Ok(forwarder::IcmpDatagram {
            meta: forwarder::IcmpDatagramMeta { peer },
            message,
        })
    }
}

#[async_trait]
impl datagram_pipe::Sink for IcmpSink {
    type Input = downstream::IcmpDatagram;

    async fn write(
        &mut self,
        datagram: downstream::IcmpDatagram,
    ) -> io::Result<datagram_pipe::SendStatus> {
        let echo = match datagram.message.to_echo() {
            None => {
                debug!("Only echo request messages can be sent to peer");
                return Ok(datagram_pipe::SendStatus::Dropped);
            }
            Some(x) => x,
        };

        let forwarder_shared = self.shared.forwarder_shared.clone();
        let sockets = forwarder_shared.sockets.read().await;
        let socket = {
            let socket = if datagram.meta.peer.is_ipv4() {
                sockets.v4.as_ref()
            } else {
                sockets.v6.as_ref()
            };

            match socket {
                None => return Ok(datagram_pipe::SendStatus::Dropped),
                Some(x) => x,
            }
        };

        let serialized = datagram.message.serialize();
        socket
            .send_to(datagram.meta.peer, datagram.ttl, &serialized)
            .await?;

        let deadline = Instant::now()
            + forwarder_shared
                .core_settings
                .icmp
                .as_ref()
                .unwrap()
                .request_timeout;
        let mut listeners = forwarder_shared.listeners.lock().unwrap();
        listeners.reply_waiters.insert(
            echo.clone(),
            ReplyWaiter {
                original_peer: datagram.meta.peer,
                waker_tx: self.tx.clone(),
            },
        );

        match listeners.deadlines.entry(deadline) {
            Entry::Vacant(e) => {
                e.insert(LinkedList::from([echo.clone()]));
                if listeners.deadlines.len() == 1 {
                    forwarder_shared.deadline_waker_tx.notify_one();
                }
            }
            Entry::Occupied(mut e) => {
                e.get_mut().push_back(echo.clone());
            }
        }

        Ok(datagram_pipe::SendStatus::Sent)
    }
}

struct RawPacketStream {
    inner: AsyncFd<libc::c_int>,
}

impl RawPacketStream {
    pub fn new(protocol: libc::c_int, if_name: &str) -> io::Result<Self> {
        let family = match protocol {
            libc::IPPROTO_ICMP => libc::AF_INET,
            libc::IPPROTO_ICMPV6 => libc::AF_INET6,
            _ => unreachable!(),
        };

        unsafe {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            let fd = libc::socket(family, libc::SOCK_RAW, protocol);
            #[cfg(target_os = "macos")]
            let fd = libc::socket(family, libc::SOCK_DGRAM, protocol);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }

            net_utils::bind_to_interface(fd, family, if_name)?;

            if family == libc::AF_INET && 0 != set_icmp_filter(fd) {
                libc::close(fd);
                return Err(io::Error::last_os_error());
            }
            if family == libc::AF_INET6 && 0 != set_icmpv6_filter(fd) {
                libc::close(fd);
                return Err(io::Error::last_os_error());
            }

            let socket = AsyncFd::new(fd).inspect_err(|_| {
                libc::close(fd);
            })?;

            Ok(Self { inner: socket })
        }
    }

    pub async fn send_to(&self, dst: IpAddr, ttl: u8, packet: &Bytes) -> io::Result<()> {
        let guard = self.inner.writable().await?;
        net_utils::set_socket_ttl(*guard.get_inner(), dst.is_ipv4(), ttl)?;

        let (sockaddr, sockaddr_len) = net_utils::socket_addr_to_libc(&SocketAddr::from((dst, 0)));
        unsafe {
            let r = libc::sendto(
                *guard.get_inner(),
                (*packet).as_ptr() as *const libc::c_void,
                packet.len(),
                0,
                &sockaddr as *const _ as *const libc::sockaddr,
                sockaddr_len,
            );
            if r < 0 {
                return Err(io::Error::last_os_error());
            }
        }

        Ok(())
    }
}

impl Drop for RawPacketStream {
    fn drop(&mut self) {
        let fd = self.inner.get_ref();
        unsafe {
            if 0 != libc::close(*fd) {
                debug!("Failed to close socket: {}", io::Error::last_os_error());
            }
        }
    }
}
