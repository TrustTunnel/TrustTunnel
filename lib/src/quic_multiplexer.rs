use crate::http_codec::{RequestHeaders, ResponseHeaders};
use crate::settings::Settings;
use crate::tls_demultiplexer::TlsDemux;
use crate::utils::Either;
use crate::{log_id, log_utils, net_utils, tls_demultiplexer, utils};
use boring::ssl::{NameType, SelectCertError, SslContextBuilder, SslMethod, SslRef};
use bytes::{Buf, Bytes, BytesMut};
use http::header::InvalidHeaderName;
use lazy_static::lazy_static;
use quiche::h3;
use quiche::h3::NameValue;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::Instant;

const TOKEN_PREFIX_SIZE: usize = 16;
const MUX_ID_FMT: &str = "QMUX={}";
const SOCKET_ID_FMT: &str = "QSOCK={}";

const QUIC_CONNECTION_CLOSE_CODE: u64 = 0x42;

type QuicConnection = quiche::Connection;

pub(crate) struct QuicMultiplexer {
    core_settings: Arc<Settings>,
    socket: Arc<UdpSocket>,
    /// Receives messages from [`QuicSocket.mux_tx`]
    socket_rx: mpsc::Receiver<SocketMessage>,
    /// See [`QuicSocket.mux_tx`]
    mux_tx: Arc<std::sync::Mutex<mpsc::Sender<SocketMessage>>>,
    connections: HashMap<quiche::ConnectionId<'static>, Connection>,
    /// Active wire CIDs map to the stable connection/deadline key.
    connection_ids: HashMap<quiche::ConnectionId<'static>, quiche::ConnectionId<'static>>,
    /// Per connection, the client CIDs of paths the client migrated away from.
    migrations: HashMap<quiche::ConnectionId<'static>, PeerMigrations>,
    deadlines: HashMap<quiche::ConnectionId<'static>, Instant>,
    closest_deadline: Option<Instant>,
    tls_demux: Arc<std::sync::RwLock<TlsDemux>>,
    token_prefix: [u8; TOKEN_PREFIX_SIZE],
    id: log_utils::IdChain<u64>,
    next_socket_id: Arc<AtomicU64>,
}

/// See [`retire_left_path_dcid`].
#[derive(Default)]
struct PeerMigrations {
    /// The client CID the connection sends to, when last looked at.
    active: Option<quiche::ConnectionId<'static>>,
    /// Migrations to a different client CID seen so far; each left one path behind.
    migrated: u64,
    /// Sequence number of the next client CID to retire, which is also the number retired.
    next_to_retire: u64,
}

/// Retire a departed path's client CID before receiving a new path's first packet.
/// quiche keeps paths with a DCID and assigns replacement DCIDs to the first empty path,
/// so retiring earlier would refill the old path instead of freeing its slot (RFC 9000 §9.5).
fn retire_left_path_dcid(
    migrations: &mut PeerMigrations,
    conn: &mut QuicConnection,
    peer: &SocketAddr,
    id: &log_utils::IdChain<u64>,
) {
    if !conn.is_established() || conn.is_draining() || conn.is_closed() {
        return;
    }
    // A migration is a validated move to a different client CID; the same CID on a new address
    // pair is a rebinding that leaves no path behind.
    while let Some(event) = conn.path_event_next() {
        if matches!(event, quiche::PathEvent::PeerMigrated(..)) {
            let active = conn.destination_id().into_owned();
            if migrations
                .active
                .as_ref()
                .is_some_and(|previous| *previous != active)
            {
                migrations.migrated += 1;
            }
            migrations.active = Some(active);
        }
    }
    if migrations.active.is_none() {
        migrations.active = Some(conn.destination_id().into_owned());
    }
    // Only a path the client really left may lose its CID: retiring the CID of the path that is
    // still active would leave it unable to send.
    if migrations.next_to_retire >= migrations.migrated {
        return;
    }
    if conn.path_stats().any(|path| path.peer_addr == *peer) {
        return;
    }
    match conn.retire_dcid(migrations.next_to_retire) {
        Ok(()) => migrations.next_to_retire += 1,
        Err(e) => log_id!(
            debug,
            id,
            "Failed to retire the client CID of a left path: {}",
            e
        ),
    }
}

pub(crate) struct QuicSocket {
    conn_id: quiche::ConnectionId<'static>,
    /// Receives messages from [`EstablishedConnection.socket_tx`]
    conn_rx: tokio::sync::Mutex<mpsc::Receiver<MultiplexerMessage>>,
    /// Sends messages to [`QuicMultiplexer.socket_rx`]
    mux_tx: Arc<std::sync::Mutex<mpsc::Sender<SocketMessage>>>,
    peer: SocketAddr,
    udp_socket: Arc<UdpSocket>,
    quic_conn: Arc<std::sync::Mutex<QuicConnection>>,
    h3_conn: Arc<std::sync::Mutex<h3::Connection>>,
    waiting_writable_streams: std::sync::Mutex<HashSet<u64>>,
    id: log_utils::IdChain<u64>,
    tls_connection_meta: tls_demultiplexer::ConnectionMeta,
    /// TLS client_random extracted from QUIC handshake
    client_random: Vec<u8>,
}

pub(crate) enum QuicSocketEvent {
    Request(/* stream id */ u64, Box<RequestHeaders>),
    Readable(/* stream id */ u64),
    Writable(Vec</* stream id */ u64>),
    Close(/* stream id */ u64),
}

/// Messages sent by [`QuicMultiplexer`] to [`QuicSocket`]s
#[derive(Ord, PartialOrd, Eq, PartialEq)]
enum MultiplexerMessage {
    PollH3,
    Close,
}

/// Messages sent by [`QuicSocket`]s to [`QuicMultiplexer`]
enum SocketMessage {
    Close(quiche::ConnectionId<'static>),
    /// Tell the multiplexer to re-read [`quiche::Connection::timeout`] for this connection.
    RefreshDeadline(quiche::ConnectionId<'static>),
}

struct HandshakingConnection {
    quic_conn: Arc<std::sync::Mutex<QuicConnection>>,
    local_address: SocketAddr,
    tls_connection_meta: tls_demultiplexer::ConnectionMeta,
}

struct EstablishedConnection {
    /// Sends messages to [`QuicSocket.conn_rx`]
    socket_tx: mpsc::Sender<MultiplexerMessage>,
    quic_conn: Arc<std::sync::Mutex<QuicConnection>>,
}

enum Connection {
    Handshake(HandshakingConnection),
    Established(EstablishedConnection),
}

enum UnknownPacketStatus {
    Process,
    Skip,
}

enum HandshakeStatus {
    InProgress,
    Complete,
}

/// Background means that a connection is not yet established or already being tunneled
struct BackgroundConnection {
    quic: Arc<std::sync::Mutex<QuicConnection>>,
    message: Option<(mpsc::Sender<MultiplexerMessage>, MultiplexerMessage)>,
}

impl BackgroundConnection {
    fn with_conn(conn: Arc<std::sync::Mutex<QuicConnection>>) -> Self {
        Self {
            quic: conn,
            message: Default::default(),
        }
    }
}

impl QuicMultiplexer {
    pub fn new(
        core_settings: Arc<Settings>,
        socket: UdpSocket,
        tls_demux: Arc<std::sync::RwLock<TlsDemux>>,
        next_socket_id: Arc<AtomicU64>,
    ) -> io::Result<Self> {
        let queue_cap = core_settings
            .listen_protocols
            .quic
            .as_ref()
            .unwrap()
            .message_queue_capacity;
        let (tx, rx) = mpsc::channel(queue_cap);

        Ok(Self {
            core_settings,
            socket: Arc::new(socket),
            socket_rx: rx,
            mux_tx: Arc::new(std::sync::Mutex::new(tx)),
            connections: Default::default(),
            connection_ids: Default::default(),
            migrations: Default::default(),
            deadlines: Default::default(),
            closest_deadline: None,
            tls_demux,
            token_prefix: ring::rand::generate(&ring::rand::SystemRandom::new())
                .unwrap()
                .expose(),
            id: log_utils::IdChain::from(log_utils::IdItem::new(MUX_ID_FMT, 0)),
            next_socket_id,
        })
    }

    pub async fn listen(&mut self) -> io::Result<QuicSocket> {
        enum Event {
            UdpRead,
            UdpSend(SocketMessage),
        }

        loop {
            let event = {
                let wait_timeout =
                    tokio::time::sleep_until(self.closest_deadline.unwrap_or_else(Instant::now));
                tokio::pin!(wait_timeout);

                let wait_udp_send = self.socket_rx.recv();
                tokio::pin!(wait_udp_send);

                let wait_udp_read = self.socket.readable();
                tokio::pin!(wait_udp_read);

                tokio::select! {
                    r = wait_udp_read => match r {
                        Ok(_) => Some(Event::UdpRead),
                        Err(e) => return Err(e),
                    },
                    r = wait_udp_send => match r {
                        Some(m) => Some(Event::UdpSend(m)),
                        None => return Err(io::Error::other("Message receiving channel closed unexpectedly")),
                    },
                    // Arm this even for an expired deadline: tokio timers fire slightly
                    // after the deadline, so a `> now` guard disables the branch exactly
                    // when the timer fires and timeouts would only run on incoming packets.
                    _ = &mut wait_timeout, if self.closest_deadline.is_some() => None,
                }
            };

            self.process_timeouts();

            match event {
                None => (),
                Some(Event::UdpSend(m)) => self.on_socket_message(m)?,
                Some(Event::UdpRead) => {
                    if let Some(s) = self.read_udp_socket()? {
                        return Ok(s);
                    }
                }
            }

            self.process_pending_socket_messages()?;
            self.remove_closed_connections();
        }
    }

    fn read_udp_socket(&mut self) -> io::Result<Option<QuicSocket>> {
        struct Entry {
            conn: Arc<std::sync::Mutex<QuicConnection>>,
            socket_tx: Option<mpsc::Sender<MultiplexerMessage>>,
            messages: BTreeSet<MultiplexerMessage>,
        }

        const READ_BUDGET: usize = 1024;

        let mut socket = None;
        let mut pending = HashMap::with_capacity(READ_BUDGET / 2);
        let mut buffer = [0; net_utils::MAX_UDP_PAYLOAD_SIZE];
        for _ in 0..READ_BUDGET {
            match self.socket.try_recv_from(&mut buffer) {
                Ok((n, peer)) => {
                    let header =
                        match quiche::Header::from_slice(&mut buffer[..n], quiche::MAX_CONN_ID_LEN)
                        {
                            Ok(h) => {
                                log_id!(trace, self.id, "Received QUIC packet: {:?}", h);
                                h
                            }
                            Err(e) => {
                                log_id!(debug, self.id, "Parsing UDP packet header failed: {}", e);
                                continue;
                            }
                        };

                    let conn_id = self
                        .connection_ids
                        .get(&header.dcid)
                        .unwrap_or(&header.dcid)
                        .clone();
                    match self.on_quic_packet(&peer, &header, &mut buffer[..n]) {
                        Some(Either::Left(s)) => {
                            pending.entry(conn_id).or_insert_with(|| Entry {
                                conn: s.quic_conn.clone(),
                                socket_tx: Default::default(),
                                messages: Default::default(),
                            });
                            socket = Some(s);
                            break;
                        }
                        Some(Either::Right(x)) => {
                            let entry = pending.entry(conn_id).or_insert_with(|| Entry {
                                conn: x.quic,
                                socket_tx: None,
                                messages: Default::default(),
                            });
                            if let Some((tx, msg)) = x.message {
                                entry.socket_tx = Some(tx);
                                entry.messages.insert(msg);
                            }
                        }
                        None => continue,
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }

        for (conn_id, entry) in pending {
            let result = entry
                .socket_tx
                .iter()
                .cycle()
                .zip(entry.messages)
                .try_for_each(|(sender, msg)| match sender.try_send(msg) {
                    // `Full` is not considered as an error in this case, as the connection does not need
                    // multiple `poll` messages in the queue
                    Ok(_) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        Err(io::Error::other("Channel closed"))
                    }
                });

            let mut quic_conn = entry.conn.lock().unwrap();
            if let Err(e) = result {
                let _ =
                    quic_conn.close(false, QUIC_CONNECTION_CLOSE_CODE, e.to_string().as_bytes());
            }

            if let Err(e) = self.refresh_connection_ids(&conn_id, &mut quic_conn) {
                log_id!(debug, self.id, "Failed to update connection IDs: {}", e);
                let _ =
                    quic_conn.close(false, QUIC_CONNECTION_CLOSE_CODE, e.to_string().as_bytes());
            }

            if let Some(timeout) = quic_conn.timeout() {
                self.update_connection_deadline(conn_id, timeout);
            }

            if let Err(e) = flush_pending_data(&mut quic_conn, &self.socket, &self.id) {
                log_id!(debug, self.id, "Failed to flush QUIC connection: {}", e);
            }
        }

        Ok(socket)
    }

    fn on_quic_packet(
        &mut self,
        peer: &SocketAddr,
        header: &quiche::Header<'static>,
        packet: &mut [u8],
    ) -> Option<Either<QuicSocket, BackgroundConnection>> {
        let conn_id = self.connection_ids.get(&header.dcid).cloned();
        let (quic_conn, err) = match conn_id.as_ref().and_then(|id| self.connections.get(id)) {
            None => match self.on_unknown_quic_packet(peer, header) {
                Ok(UnknownPacketStatus::Process) => {
                    match self.on_new_connection(peer, header, packet) {
                        Ok(x) => return Some(x.map_right(BackgroundConnection::with_conn)),
                        Err((e, Some(quic))) => (quic, e),
                        Err((e, None)) => {
                            log_id!(
                                debug,
                                self.id,
                                "Failed to process QUIC packet: header={:?}, error={}",
                                header,
                                e
                            );
                            return None;
                        }
                    }
                }
                Ok(UnknownPacketStatus::Skip) => return None,
                Err(e) => {
                    log_id!(
                        debug,
                        self.id,
                        "Failed to process QUIC packet: header={:?}, error={}",
                        header,
                        e
                    );
                    return None;
                }
            },
            Some(Connection::Handshake(conn)) => {
                match conn.proceed_handshake(peer, packet, &self.id) {
                    Ok(HandshakeStatus::InProgress) => {
                        return Some(Either::with_right(BackgroundConnection::with_conn(
                            conn.quic_conn.clone(),
                        )))
                    }
                    Ok(HandshakeStatus::Complete) => {
                        self.deadlines.remove(&header.dcid);
                        self.connection_ids.remove(&header.dcid);
                        let conn = self
                            .connections
                            .remove(&header.dcid)
                            .map(|x| match x {
                                Connection::Handshake(x) => x,
                                Connection::Established(_) => unreachable!(),
                            })
                            .unwrap();
                        match self.finalize_established_connection(&header.dcid, conn, peer) {
                            Ok(sock) => return Some(Either::with_left(sock)),
                            Err((e, q)) => (q, e),
                        }
                    }
                    Err(e) => (conn.quic_conn.clone(), e),
                }
            }
            Some(Connection::Established(conn)) => {
                if let Some(id) = conn_id.as_ref() {
                    retire_left_path_dcid(
                        self.migrations.entry(id.clone()).or_default(),
                        &mut conn.quic_conn.lock().unwrap(),
                        peer,
                        &self.id,
                    );
                }
                match self.proceed_established_connection(conn, peer, packet) {
                    Ok(()) => {
                        return Some(Either::with_right(BackgroundConnection {
                            quic: conn.quic_conn.clone(),
                            message: Some((conn.socket_tx.clone(), MultiplexerMessage::PollH3)),
                        }))
                    }
                    Err(e) => (conn.quic_conn.clone(), e),
                }
            }
        };

        {
            let mut quic_conn = quic_conn.lock().unwrap();
            log_id!(
                debug,
                self.id,
                "Failed to process QUIC packet: header={:?}, error={}",
                header,
                err
            );
            let _ = quic_conn.close(
                false,
                QUIC_CONNECTION_CLOSE_CODE,
                err.to_string().as_bytes(),
            );
        }

        Some(Either::with_right(BackgroundConnection::with_conn(
            quic_conn,
        )))
    }

    fn on_unknown_quic_packet(
        &self,
        peer: &SocketAddr,
        header: &quiche::Header<'_>,
    ) -> io::Result<UnknownPacketStatus> {
        if !matches!(header.ty, quiche::Type::Initial) {
            return Err(io::Error::other(format!(
                "Unexpected packet type: {:?}",
                header
            )));
        }

        if !quiche::version_is_supported(header.version) {
            log_id!(trace, self.id, "Doing version negotiation: {:?}", header);
            let mut out = [0; net_utils::MAX_UDP_PAYLOAD_SIZE];
            let n = quiche::negotiate_version(&header.scid, &header.dcid, &mut out)
                .map_err(|e| io::Error::other(format!("Version negotiation failed: {}", e)))?;
            return self
                .socket
                .try_send_to(&out[..n], *peer)
                .map(|_| UnknownPacketStatus::Skip);
        }

        let quic_token = header
            .token
            .as_ref()
            .ok_or_else(|| io::Error::other("Invalid packet: initial packet must contain token"))?;

        let scid = generate_connection_id(&header.dcid);

        if quic_token.is_empty() {
            log_id!(trace, self.id, "Doing stateless retry: {:?}", header);
            let mut out = [0; net_utils::MAX_UDP_PAYLOAD_SIZE];
            let n = quiche::retry(
                &header.scid,
                &header.dcid,
                &scid,
                &mint_token(header, &self.token_prefix, peer),
                header.version,
                &mut out,
            )
            .map_err(|e| io::Error::other(format!("Retry failed: {}", e)))?;
            return self
                .socket
                .try_send_to(&out[..n], *peer)
                .map(|_| UnknownPacketStatus::Skip);
        }

        if scid.len() != header.dcid.len() {
            return Err(io::Error::other(
                "Invalid packet: unexpected destination connection ID",
            ));
        }

        Ok(UnknownPacketStatus::Process)
    }

    fn accept_quic_connection<'a>(
        &self,
        scid: &quiche::ConnectionId<'a>,
        odcid: Option<&quiche::ConnectionId<'a>>,
        peer: &SocketAddr,
        packet: &mut [u8],
    ) -> io::Result<QuicConnection> {
        let local_address = self.core_settings.listen_address;
        let mut quic_config =
            make_quic_config_with_domain_contexts(&self.core_settings, self.tls_demux.clone())?;
        let mut quic_conn = quiche::accept(scid, odcid, local_address, *peer, &mut quic_config)
            .map_err(|e| io::Error::other(format!("Failed to accept QUIC connection: {}", e)))?;

        quic_recv(
            &mut quic_conn,
            packet,
            &quiche::RecvInfo {
                from: *peer,
                to: local_address,
            },
            &self.id,
        )?;

        Ok(quic_conn)
    }

    fn finalize_established_connection(
        &mut self,
        conn_id: &quiche::ConnectionId<'_>,
        mut conn: HandshakingConnection,
        peer: &SocketAddr,
    ) -> Result<QuicSocket, (io::Error, Arc<std::sync::Mutex<QuicConnection>>)> {
        let quic_conn = conn.quic_conn;

        let sni = quic_conn
            .lock()
            .unwrap()
            .server_name()
            .unwrap_or("")
            .to_string();

        if !sni.is_empty() {
            if let Ok(meta) = self.tls_demux.read().unwrap().select(
                std::iter::once(tls_demultiplexer::Protocol::Http3.as_alpn().as_bytes()),
                sni,
            ) {
                conn.tls_connection_meta = meta;
            }
        } else {
            log_id!(
                debug,
                self.id,
                "SNI is empty in finalize_established_connection, using bootstrap meta"
            );
        }

        let h3_conn = {
            let mut quic = quic_conn.lock().unwrap();
            let h3_config = h3::Config::new().unwrap();
            let h3_conn = match h3::Connection::with_transport(&mut quic, &h3_config) {
                Ok(x) => x,
                Err(e) => {
                    drop(quic);
                    return Err((
                        io::Error::other(format!("Failed to open HTTP3 session: {}", e)),
                        quic_conn,
                    ));
                }
            };

            drop(quic);
            Arc::new(std::sync::Mutex::new(h3_conn))
        };

        // Extract client_random from QUIC after handshake is complete
        let extracted_client_random = {
            let mut quic = quic_conn.lock().unwrap();
            let ssl: &mut SslRef = quic.as_mut();
            let mut client_random = [0u8; 32];
            ssl.client_random(&mut client_random);
            client_random.to_vec()
        };

        let (tx, rx) = mpsc::channel(1);
        self.connection_ids
            .insert(conn_id.clone().into_owned(), conn_id.clone().into_owned());
        self.connections.insert(
            conn_id.clone().into_owned(),
            Connection::Established(EstablishedConnection {
                socket_tx: tx,
                quic_conn: quic_conn.clone(),
            }),
        );

        Ok(QuicSocket {
            conn_id: conn_id.clone().into_owned(),
            conn_rx: tokio::sync::Mutex::new(rx),
            mux_tx: self.mux_tx.clone(),
            peer: *peer,
            udp_socket: self.socket.clone(),
            quic_conn,
            h3_conn,
            waiting_writable_streams: Default::default(),
            id: self.id.extended(log_utils::IdItem::new(
                SOCKET_ID_FMT,
                self.next_socket_id.fetch_add(1, Ordering::Relaxed),
            )),
            tls_connection_meta: conn.tls_connection_meta,
            client_random: extracted_client_random,
        })
    }

    #[allow(clippy::type_complexity)]
    fn on_new_connection(
        &mut self,
        peer: &SocketAddr,
        header: &quiche::Header<'_>,
        packet: &mut [u8],
    ) -> Result<
        Either<QuicSocket, Arc<std::sync::Mutex<QuicConnection>>>,
        (io::Error, Option<Arc<std::sync::Mutex<QuicConnection>>>),
    > {
        let odcid = validate_token(&self.token_prefix, peer, header.token.as_ref().unwrap())
            .ok_or_else(|| (io::Error::other("Invalid packet: unexpected token"), None))?;

        log_id!(
            debug,
            self.id,
            "New connection: dcid={} scid={}",
            utils::hex_dump(&header.dcid),
            utils::hex_dump(&header.scid)
        );

        // Create QUIC connection - TLS callback will handle certificate selection automatically
        let quic_conn = self
            .accept_quic_connection(&header.dcid, Some(&odcid), peer, packet)
            .map_err(|e| (e, None))?;

        // Get connection metadata after handshake (SNI will be available)
        let tls_connection_meta = self
            .tls_demux
            .read()
            .unwrap()
            .get_quic_connection_bootstrap_meta();

        log_id!(debug, self.id, "Bootstrap meta: {:?}", tls_connection_meta);

        let is_established = quic_conn.is_established() || quic_conn.is_in_early_data();
        let quic_conn = Arc::new(std::sync::Mutex::new(quic_conn));
        let conn = HandshakingConnection {
            quic_conn: quic_conn.clone(),
            local_address: self.core_settings.listen_address,
            tls_connection_meta,
        };

        if is_established {
            return self
                .finalize_established_connection(&header.dcid, conn, peer)
                .map(Either::with_left)
                .map_err(|(e, q)| (e, Some(q)));
        }

        self.connection_ids.insert(
            header.dcid.clone().into_owned(),
            header.dcid.clone().into_owned(),
        );
        self.connections.insert(
            header.dcid.clone().into_owned(),
            Connection::Handshake(conn),
        );
        Ok(Either::with_right(quic_conn))
    }

    fn proceed_established_connection(
        &self,
        conn: &EstablishedConnection,
        peer: &SocketAddr,
        packet: &mut [u8],
    ) -> io::Result<()> {
        quic_recv(
            &mut conn.quic_conn.lock().unwrap(),
            packet,
            &quiche::RecvInfo {
                from: *peer,
                to: self.core_settings.listen_address,
            },
            &self.id,
        )
    }

    fn refresh_connection_ids(
        &mut self,
        conn_id: &quiche::ConnectionId<'static>,
        conn: &mut QuicConnection,
    ) -> io::Result<()> {
        while let Some(retired) = conn.retired_scid_next() {
            self.connection_ids.remove(&retired);
        }
        if !conn.is_established()
            || conn.is_draining()
            || conn.is_closed()
            || conn.local_error().is_some()
        {
            return Ok(());
        }

        let limit = conn.peer_transport_params().unwrap().active_conn_id_limit;
        while (conn.active_scids() as u64) < limit {
            let random: [u8; quiche::MAX_CONN_ID_LEN] =
                ring::rand::generate(&ring::rand::SystemRandom::new())
                    .map_err(|_| io::Error::other("Failed to generate connection ID"))?
                    .expose();
            let scid = generate_connection_id(&random);
            if self.connection_ids.contains_key(&scid) {
                continue;
            }
            let token: [u8; 16] = ring::rand::generate(&ring::rand::SystemRandom::new())
                .map_err(|_| io::Error::other("Failed to generate reset token"))?
                .expose();
            conn.new_scid(&scid, u128::from_be_bytes(token), false)
                .map_err(|e| io::Error::other(format!("Failed to issue connection ID: {}", e)))?;
            // Register before any sender can emit the NEW_CONNECTION_ID frame.
            self.connection_ids.insert(scid, conn_id.clone());
        }
        Ok(())
    }

    fn remove_connection(&mut self, conn_id: &quiche::ConnectionId<'static>) -> Option<Connection> {
        self.connection_ids.retain(|_, owner| owner != conn_id);
        self.migrations.remove(conn_id);
        self.deadlines.remove(conn_id);
        self.closest_deadline = self.deadlines.values().min().copied();
        self.connections.remove(conn_id)
    }

    fn update_connection_deadline(
        &mut self,
        conn_id: quiche::ConnectionId<'static>,
        duration: Duration,
    ) {
        let deadline = Instant::now() + duration;
        self.deadlines.insert(conn_id, deadline);
        self.closest_deadline = self.deadlines.values().min().copied();
    }

    fn process_timeouts(&mut self) {
        let now = Instant::now();

        let timedout: Vec<_> = self
            .deadlines
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(conn_id, _)| conn_id.clone())
            .collect();

        for conn_id in timedout {
            self.deadlines.remove(&conn_id);

            let quic_conn_arc = match self.connections.get(&conn_id) {
                None => {
                    log_id!(
                        debug,
                        self.id,
                        "Expired connection not found: {:?}",
                        conn_id
                    );
                    continue;
                }
                Some(Connection::Handshake(conn)) => conn.quic_conn.clone(),
                Some(Connection::Established(conn)) => conn.quic_conn.clone(),
            };

            let new_timeout = {
                let mut quic_conn = quic_conn_arc.lock().unwrap();
                quic_conn.on_timeout();
                if let Err(e) = flush_pending_data(&mut quic_conn, &self.socket, &self.id) {
                    log_id!(debug, self.id, "Failed to flush after on_timeout: {}", e);
                }
                quic_conn.timeout()
            };

            if let Some(timeout) = new_timeout {
                self.update_connection_deadline(conn_id, timeout);
            }
        }

        self.closest_deadline = self.deadlines.values().min().copied();
    }

    fn on_socket_message(&mut self, message: SocketMessage) -> io::Result<()> {
        match message {
            SocketMessage::Close(conn_id) => {
                self.remove_connection(&conn_id);
                Ok(())
            }
            SocketMessage::RefreshDeadline(conn_id) => {
                let quic_conn = match self.connections.get(&conn_id) {
                    Some(Connection::Handshake(c)) => c.quic_conn.clone(),
                    Some(Connection::Established(c)) => c.quic_conn.clone(),
                    None => return Ok(()),
                };
                let timeout = quic_conn.lock().unwrap().timeout();
                match timeout {
                    Some(d) => self.update_connection_deadline(conn_id, d),
                    None => {
                        if self.deadlines.remove(&conn_id).is_some() {
                            self.closest_deadline = self.deadlines.values().min().copied();
                        }
                    }
                }
                Ok(())
            }
        }
    }

    fn process_pending_socket_messages(&mut self) -> io::Result<()> {
        loop {
            match self.socket_rx.try_recv() {
                Ok(m) => self.on_socket_message(m)?,
                Err(mpsc::error::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return Err(io::Error::other(
                        "Message receive channel closed unexpectedly",
                    ))
                }
            }
        }
    }

    fn remove_closed_connections(&mut self) {
        let closed: Vec<_> = self
            .connections
            .iter()
            .filter(|(_, conn)| match conn {
                Connection::Handshake(c) => c.quic_conn.lock().unwrap().is_closed(),
                Connection::Established(c) => c.quic_conn.lock().unwrap().is_closed(),
            })
            .map(|(id, _)| id.clone())
            .collect();

        for conn_id in closed {
            if let Some(Connection::Established(c)) = self.remove_connection(&conn_id) {
                let _ = c.socket_tx.try_send(MultiplexerMessage::Close);
            }
        }
    }
}

impl QuicSocket {
    pub fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer)
    }

    pub fn tls_connection_meta(&self) -> &tls_demultiplexer::ConnectionMeta {
        &self.tls_connection_meta
    }

    pub fn client_random(&self) -> Vec<u8> {
        self.client_random.clone()
    }

    pub fn send_response(
        &self,
        stream_id: u64,
        response: &ResponseHeaders,
        fin: bool,
    ) -> io::Result<()> {
        let response: Vec<_> = std::iter::once(h3::HeaderRef::new(
            b":status",
            response.status.as_str().as_bytes(),
        ))
        .chain(
            response
                .headers
                .iter()
                .map(|(n, v)| h3::HeaderRef::new(n.as_ref(), v.as_ref())),
        )
        .collect();

        self.h3_conn
            .lock()
            .unwrap()
            .send_response(
                &mut self.quic_conn.lock().unwrap(),
                stream_id,
                response.as_slice(),
                fin,
            )
            .map_err(|e| {
                let kind = if matches!(e, h3::Error::StreamBlocked) {
                    ErrorKind::WouldBlock
                } else {
                    ErrorKind::Other
                };
                io::Error::new(kind, e.to_string())
            })?;

        self.flush_pending_data()
    }

    pub fn read(&self, stream_id: u64) -> io::Result<Option<Bytes>> {
        let chunk = {
            const READ_CHUNK_SIZE: usize = 64 * 1024;
            let mut bytes = BytesMut::zeroed(READ_CHUNK_SIZE);

            let mut read_offset = 0;
            loop {
                let buf = bytes.split_at_mut(read_offset).1;
                match self.h3_conn.lock().unwrap().recv_body(
                    &mut self.quic_conn.lock().unwrap(),
                    stream_id,
                    buf,
                ) {
                    Ok(n) => {
                        read_offset += n;
                        if read_offset == bytes.capacity() {
                            break Some(bytes.freeze());
                        }
                    }
                    Err(h3::Error::Done) if read_offset > 0 => {
                        bytes.truncate(read_offset);
                        break Some(bytes.freeze());
                    }
                    Err(h3::Error::Done) => break None,
                    Err(e) => return Err(io::Error::other(e.to_string())),
                };
            }
        };

        self.flush_pending_data()?;
        Ok(chunk)
    }

    pub fn write(&self, stream_id: u64, mut data: Bytes) -> io::Result<Bytes> {
        match self.h3_conn.lock().unwrap().send_body(
            &mut self.quic_conn.lock().unwrap(),
            stream_id,
            data.as_ref(),
            false,
        ) {
            Ok(n) => data.advance(n),
            Err(h3::Error::Done | h3::Error::StreamBlocked) => (),
            Err(e) => return Err(io::Error::other(e.to_string())),
        }

        self.flush_pending_data().map(|_| data)
    }

    pub fn stream_capacity(&self, stream_id: u64) -> io::Result<usize> {
        self.quic_conn
            .lock()
            .unwrap()
            .stream_capacity(stream_id)
            .map_err(|e| io::Error::other(e.to_string()))
    }

    pub fn stream_finished(&self, stream_id: u64) -> bool {
        self.quic_conn.lock().unwrap().stream_finished(stream_id)
    }

    pub fn notify_stream_waiting_writable(&self, stream_id: u64) {
        self.waiting_writable_streams
            .lock()
            .unwrap()
            .insert(stream_id);
    }

    pub fn shutdown_stream(&self, stream_id: u64, direction: quiche::Shutdown) {
        match direction {
            quiche::Shutdown::Write => {
                // `stream_shutdown(Write)` is dropping still unsent data
                let _ = self
                    .quic_conn
                    .lock()
                    .unwrap()
                    .stream_send(stream_id, &[], true);
            }
            quiche::Shutdown::Read => {
                let _ = self
                    .quic_conn
                    .lock()
                    .unwrap()
                    .stream_shutdown(stream_id, direction, 0);
            }
        }
        let _ = self.flush_pending_data();
    }

    pub fn graceful_shutdown(&self) -> io::Result<()> {
        {
            let mut quic_conn = self.quic_conn.lock().unwrap();
            let mut h3_conn = self.h3_conn.lock().unwrap();
            for stream_id in quic_conn.writable() {
                let _ = h3_conn.send_body(&mut quic_conn, stream_id, &[], true);
            }
        }
        let _ = self.flush_pending_data();

        self.quic_conn
            .lock()
            .unwrap()
            .close(true, 0, b"bye")
            .map_err(|e| io::Error::other(e.to_string()))?;
        self.flush_pending_data()
    }

    pub async fn listen(&self) -> io::Result<QuicSocketEvent> {
        loop {
            let event = loop {
                match self.process_pending_h3_events()? {
                    None => {
                        let writable_streams: Vec<_> = {
                            let mut quic_conn = self.quic_conn.lock().unwrap();
                            let mut waiting_streams = self.waiting_writable_streams.lock().unwrap();
                            // Avoid emitting Writable events for streams with negligible capacity.
                            let writable_streams: Vec<_> = waiting_streams
                                .iter()
                                .filter(|id| {
                                    quic_conn.stream_capacity(**id).map_or(true, |x| {
                                        x >= net_utils::MIN_USABLE_QUIC_STREAM_CAPACITY
                                    })
                                })
                                .copied()
                                .collect();
                            waiting_streams.retain(|id| !writable_streams.contains(id));
                            writable_streams
                        };

                        if !writable_streams.is_empty() {
                            break Some(QuicSocketEvent::Writable(writable_streams));
                        }
                    }
                    Some(event) => break Some(event),
                }

                match self
                    .conn_rx
                    .lock()
                    .await
                    .recv()
                    .await
                    .ok_or_else(|| io::Error::from(ErrorKind::UnexpectedEof))?
                {
                    MultiplexerMessage::PollH3 => (),
                    MultiplexerMessage::Close => break None,
                }
            };

            let quic_conn = self.quic_conn.lock().unwrap();
            if quic_conn.is_closed() {
                let _ = self
                    .mux_tx
                    .lock()
                    .unwrap()
                    .try_send(SocketMessage::Close(self.conn_id.clone()));
                return Err(io::Error::from(ErrorKind::UnexpectedEof));
            }

            match event {
                None => (),
                Some(ev) => return Ok(ev),
            }
        }
    }

    fn flush_pending_data(&self) -> io::Result<()> {
        let result = {
            let mut quic_conn = self.quic_conn.lock().unwrap();
            flush_pending_data(&mut quic_conn, &self.udp_socket, &self.id)
        };
        // Notify the multiplexer that the loss-detection timer may have changed
        let _ = self
            .mux_tx
            .lock()
            .unwrap()
            .try_send(SocketMessage::RefreshDeadline(self.conn_id.clone()));
        result
    }

    fn poll_h3_connection(&self) -> h3::Result<(u64, h3::Event)> {
        self.h3_conn
            .lock()
            .unwrap()
            .poll(&mut self.quic_conn.lock().unwrap())
    }

    fn process_pending_h3_events(&self) -> io::Result<Option<QuicSocketEvent>> {
        match self.poll_h3_connection() {
            Ok((stream_id, h3::Event::Headers { list, .. })) => {
                match self.on_request(stream_id, list) {
                    Ok(x) => Ok(Some(x)),
                    Err(e) => {
                        let response = http::Response::builder()
                            .status(http::StatusCode::BAD_REQUEST)
                            .body(())
                            .unwrap()
                            .into_parts()
                            .0;
                        let _ = self.send_response(stream_id, &response, true);
                        Err(e)
                    }
                }
            }
            Ok((stream_id, h3::Event::Data)) => Ok(Some(QuicSocketEvent::Readable(stream_id))),
            Ok((stream_id, h3::Event::Finished)) => Ok(Some(QuicSocketEvent::Close(stream_id))),
            Ok((stream_id, h3::Event::Reset(err))) => {
                log_id!(
                    trace,
                    self.id,
                    "Stream reset by client: id={}, err={}",
                    stream_id,
                    err
                );
                Ok(Some(QuicSocketEvent::Close(stream_id)))
            }
            Ok((_, h3::Event::PriorityUpdate)) => Ok(None),
            Ok((_, h3::Event::GoAway)) => {
                Err(io::Error::new(ErrorKind::UnexpectedEof, "Received GOAWAY"))
            }
            Err(h3::Error::Done) => Ok(None),
            Err(e) => Err(io::Error::other(e.to_string())),
        }
    }

    fn on_request(&self, stream_id: u64, headers: Vec<h3::Header>) -> io::Result<QuicSocketEvent> {
        let mut request_builder = http::request::Request::builder().version(http::Version::HTTP_3);

        let mut uri_builder = http::uri::Uri::builder();
        for h in headers {
            match h.name() {
                b":method" => request_builder = request_builder.method(h.value()),
                b":scheme" => uri_builder = uri_builder.scheme(h.value()),
                b":authority" => uri_builder = uri_builder.authority(h.value()),
                b":path" => uri_builder = uri_builder.path_and_query(h.value()),
                x => {
                    request_builder = match http::header::HeaderName::from_lowercase(x) {
                        Ok(name) => request_builder.header(name, h.value()),
                        Err(InvalidHeaderName { .. }) => {
                            return Err(io::Error::new(
                                ErrorKind::InvalidData,
                                format!("Unexpected header name: 0x{}", utils::hex_dump(h.name())),
                            ))
                        }
                    }
                }
            }
        }

        request_builder
            .uri(uri_builder.build().map_err(|e| {
                io::Error::new(ErrorKind::InvalidData, format!("Invalid URI: {}", e))
            })?)
            .body(())
            .map(|r| QuicSocketEvent::Request(stream_id, Box::new(r.into_parts().0)))
            .map_err(|e| io::Error::other(format!("Invalid request: {}", e)))
    }
}

impl HandshakingConnection {
    fn proceed_handshake(
        &self,
        peer: &SocketAddr,
        packet: &mut [u8],
        log_id: &log_utils::IdChain<u64>,
    ) -> io::Result<HandshakeStatus> {
        let mut quic_conn = self.quic_conn.lock().unwrap();
        quic_recv(
            &mut quic_conn,
            packet,
            &quiche::RecvInfo {
                from: *peer,
                to: self.local_address,
            },
            log_id,
        )?;

        if quic_conn.is_closed() {
            return Err(io::Error::other(format!(
                "[{}] Connection closed",
                quic_conn.trace_id()
            )));
        }

        if quic_conn.is_draining() {
            return Ok(HandshakeStatus::InProgress);
        }

        if !quic_conn.is_established() && !quic_conn.is_in_early_data() {
            return Ok(HandshakeStatus::InProgress);
        }

        Ok(HandshakeStatus::Complete)
    }
}

fn flush_pending_data(
    quic_conn: &mut quiche::Connection,
    udp_socket: &UdpSocket,
    id: &log_utils::IdChain<u64>,
) -> io::Result<()> {
    let mut out = [0; net_utils::MAX_UDP_PAYLOAD_SIZE];
    loop {
        match quic_conn.send(&mut out) {
            Ok((n, info)) => udp_socket_send_to(udp_socket, &out[..n], &info.to, id)?,
            Err(quiche::Error::Done) => break,
            Err(e) => return Err(io::Error::other(e.to_string())),
        }
    }

    Ok(())
}

fn udp_socket_send_to(
    socket: &UdpSocket,
    data: &[u8],
    peer: &SocketAddr,
    id: &log_utils::IdChain<u64>,
) -> io::Result<()> {
    match socket.try_send_to(data, *peer) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::WouldBlock || e.raw_os_error() == Some(libc::ENOBUFS) => {
            log_id!(
                debug,
                id,
                "Dropping {} bytes due to socket would block: peer={}",
                data.len(),
                peer
            );
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn quic_recv(
    conn: &mut quiche::Connection,
    packet: &mut [u8],
    info: &quiche::RecvInfo,
    id: &log_utils::IdChain<u64>,
) -> io::Result<()> {
    match conn.recv(packet, *info) {
        Ok(n) => {
            if n != packet.len() {
                log_id!(
                    debug,
                    id,
                    "Dropping {} bytes unaccepted during handshake: {}",
                    packet.len() - n,
                    conn.trace_id()
                );
            }
            Ok(())
        }
        Err(e) => Err(io::Error::other(format!("QUIC receive failure: {}", e))),
    }
}

fn make_quic_config_with_domain_contexts(
    core_settings: &Settings,
    tls_demux: Arc<std::sync::RwLock<TlsDemux>>,
) -> io::Result<quiche::Config> {
    let quic_settings = core_settings.listen_protocols.quic.as_ref().unwrap();

    // Create main SSL context with SNI callback that dynamically creates and switches contexts
    let mut main_ctx = SslContextBuilder::new(SslMethod::tls())?;

    // Clone tls_demux for use in callback
    let tls_demux_clone = tls_demux.clone();
    main_ctx.set_select_certificate_callback(move |mut client_hello| {
        let Some(sni) = client_hello.servername(NameType::HOST_NAME) else {
            return Ok(());
        };

        let meta = match tls_demux_clone.read().unwrap().select(
            std::iter::once(tls_demultiplexer::Protocol::Http3.as_alpn().as_bytes()),
            sni.to_string(),
        ) {
            Ok(m) => m,
            Err(_) => return Ok(()), // unknown SNI -> bootstrap cert
        };

        if meta.boring.chain.is_empty() {
            return Err(SelectCertError::ERROR);
        }

        let ssl = client_hello.ssl_mut();

        ssl.set_certificate(&meta.boring.chain[0])
            .map_err(|_| SelectCertError::ERROR)?;

        for cert in meta.boring.chain.iter().skip(1) {
            ssl.add_chain_cert(cert)
                .map_err(|_| SelectCertError::ERROR)?;
        }

        ssl.set_private_key(&meta.boring.key)
            .map_err(|_| SelectCertError::ERROR)?;

        Ok(())
    });

    // Load bootstrap certificate as default
    let bootstrap_meta = tls_demux
        .read()
        .unwrap()
        .get_quic_connection_bootstrap_meta();
    main_ctx.set_certificate_chain_file(&bootstrap_meta.cert_chain_path)?;
    main_ctx.set_private_key_file(&bootstrap_meta.key_path, boring::ssl::SslFiletype::PEM)?;

    let mut cfg = quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, main_ctx)
        .map_err(|e| io::Error::other(format!("Failed to create QUIC config: {}", e)))?;
    cfg.set_application_protos(h3::APPLICATION_PROTOCOL)
        .unwrap();
    cfg.set_max_idle_timeout(core_settings.client_listener_timeout.as_millis() as u64);
    cfg.set_max_recv_udp_payload_size(quic_settings.recv_udp_payload_size);
    cfg.set_max_send_udp_payload_size(quic_settings.send_udp_payload_size);
    cfg.set_initial_max_data(quic_settings.initial_max_data);
    cfg.set_initial_max_stream_data_bidi_local(quic_settings.initial_max_stream_data_bidi_local);
    cfg.set_initial_max_stream_data_bidi_remote(quic_settings.initial_max_stream_data_bidi_remote);
    cfg.set_initial_max_stream_data_uni(quic_settings.initial_max_stream_data_uni);
    cfg.set_initial_max_streams_bidi(quic_settings.initial_max_streams_bidi);
    cfg.set_initial_max_streams_uni(quic_settings.initial_max_streams_uni);
    cfg.set_max_connection_window(quic_settings.max_connection_window);
    cfg.set_max_stream_window(quic_settings.max_stream_window);
    cfg.set_disable_active_migration(quic_settings.disable_active_migration);
    if quic_settings.enable_early_data {
        cfg.enable_early_data();
    }
    Ok(cfg)
}

fn generate_connection_id(input: &[u8]) -> quiche::ConnectionId<'static> {
    lazy_static! {
        static ref CONN_ID_SEED: ring::hmac::Key = {
            let rng = ring::rand::SystemRandom::new();
            ring::hmac::Key::generate(ring::hmac::HMAC_SHA256, &rng).unwrap()
        };
    }
    let id = ring::hmac::sign(&CONN_ID_SEED, input);
    quiche::ConnectionId::from_ref(&id.as_ref()[..quiche::MAX_CONN_ID_LEN]).into_owned()
}

fn socket_addr_to_vec(addr: &SocketAddr) -> Vec<u8> {
    match addr.ip() {
        std::net::IpAddr::V4(a) => a
            .octets()
            .iter()
            .cloned()
            .chain(addr.port().to_be_bytes().iter().cloned())
            .collect(),
        std::net::IpAddr::V6(a) => a
            .octets()
            .iter()
            .cloned()
            .chain(addr.port().to_be_bytes().iter().cloned())
            .collect(),
    }
}

fn mint_token(
    header: &quiche::Header,
    prefix: &[u8; TOKEN_PREFIX_SIZE],
    peer: &SocketAddr,
) -> Vec<u8> {
    prefix
        .iter()
        .cloned()
        .chain(socket_addr_to_vec(peer).iter().cloned())
        .chain(header.dcid.iter().cloned())
        .collect()
}

fn validate_token<'a>(
    prefix: &[u8; TOKEN_PREFIX_SIZE],
    peer: &SocketAddr,
    token: &'a [u8],
) -> Option<quiche::ConnectionId<'a>> {
    token
        .strip_prefix(prefix)
        .and_then(|token| token.strip_prefix(socket_addr_to_vec(peer).as_slice()))
        .map(quiche::ConnectionId::from_ref)
}

#[cfg(test)]
mod tests {
    use super::*;
    use boring::asn1::Asn1Time;
    use boring::hash::MessageDigest;
    use boring::pkey::PKey;
    use boring::rsa::Rsa;
    use boring::x509::{X509NameBuilder, X509};
    use quiche::test_utils::Pipe;

    fn pipe() -> Pipe {
        let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "localhost").unwrap();
        let name = name.build();
        let mut cert = X509::builder().unwrap();
        cert.set_version(2).unwrap();
        cert.set_subject_name(&name).unwrap();
        cert.set_issuer_name(&name).unwrap();
        cert.set_pubkey(&key).unwrap();
        cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        cert.set_not_after(&Asn1Time::days_from_now(1).unwrap())
            .unwrap();
        cert.sign(&key, MessageDigest::sha256()).unwrap();
        let mut tls = SslContextBuilder::new(SslMethod::tls()).unwrap();
        tls.set_certificate(&cert.build()).unwrap();
        tls.set_private_key(&key).unwrap();
        let mut config =
            quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, tls).unwrap();
        config.set_application_protos(&[b"h3"]).unwrap();
        config.verify_peer(false);
        config.set_initial_max_data(1024);
        config.set_initial_max_stream_data_bidi_remote(1024);
        config.set_initial_max_streams_bidi(1);
        config.set_active_connection_id_limit(2);
        Pipe::with_config_and_scid_lengths(
            &mut config,
            quiche::MAX_CONN_ID_LEN,
            quiche::MAX_CONN_ID_LEN,
        )
        .unwrap()
    }

    async fn multiplexer() -> QuicMultiplexer {
        let settings = Arc::new(Settings::default());
        let tls = TlsDemux::new(&settings, &Default::default()).unwrap();
        QuicMultiplexer::new(
            settings,
            UdpSocket::bind("127.0.0.1:0").await.unwrap(),
            Arc::new(std::sync::RwLock::new(tls)),
            Arc::new(AtomicU64::new(0)),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn connection_ids_are_issued_replenished_and_removed() {
        let mut mux = multiplexer().await;
        let mut pipe = pipe();
        let conn_id = pipe.server.source_id().into_owned();
        mux.connection_ids.insert(conn_id.clone(), conn_id.clone());
        mux.refresh_connection_ids(&conn_id, &mut pipe.server)
            .unwrap();
        assert_eq!(mux.connection_ids.len(), 1);
        pipe.handshake().unwrap();
        pipe.advance().unwrap();
        mux.refresh_connection_ids(&conn_id, &mut pipe.server)
            .unwrap();
        assert_eq!(pipe.server.active_scids(), 2);
        assert_eq!(mux.connection_ids.len(), 2);
        assert!(mux.connection_ids.values().all(|owner| *owner == conn_id));
        pipe.advance().unwrap();

        // Retire the initial wire CID; the connection/deadline key must survive.
        pipe.client.retire_dcid(0).unwrap();
        pipe.advance().unwrap();
        mux.refresh_connection_ids(&conn_id, &mut pipe.server)
            .unwrap();
        assert_eq!(pipe.server.active_scids(), 2);
        assert_eq!(mux.connection_ids.len(), 2);
        assert!(!mux.connection_ids.contains_key(&conn_id));
        assert!(mux.connection_ids.values().all(|owner| *owner == conn_id));
        pipe.advance().unwrap();
        assert_eq!(pipe.client.available_dcids(), 1);

        let other_id = quiche::ConnectionId::from_vec(vec![42; quiche::MAX_CONN_ID_LEN]);
        mux.connection_ids
            .insert(other_id.clone(), other_id.clone());
        mux.update_connection_deadline(other_id.clone(), Duration::from_secs(20));
        mux.update_connection_deadline(conn_id.clone(), Duration::from_secs(10));
        mux.migrations
            .insert(conn_id.clone(), PeerMigrations::default());
        let (socket_tx, _socket_rx) = mpsc::channel(1);
        mux.connections.insert(
            conn_id.clone(),
            Connection::Established(EstablishedConnection {
                socket_tx,
                quic_conn: Arc::new(std::sync::Mutex::new(pipe.server)),
            }),
        );
        mux.on_socket_message(SocketMessage::Close(conn_id.clone()))
            .unwrap();
        assert!(!mux.connections.contains_key(&conn_id));
        assert!(!mux.migrations.contains_key(&conn_id));
        assert!(!mux.deadlines.contains_key(&conn_id));
        assert_eq!(mux.connection_ids.len(), 1);
        assert_eq!(mux.connection_ids.get(&other_id), Some(&other_id));
        assert_eq!(mux.closest_deadline, mux.deadlines.get(&other_id).copied());
    }

    #[tokio::test]
    async fn closing_connection_does_not_issue_connection_ids() {
        let mut mux = multiplexer().await;
        let mut pipe = pipe();
        pipe.handshake().unwrap();
        pipe.advance().unwrap();
        let conn_id = pipe.server.source_id().into_owned();
        pipe.server.close(false, 0, b"").unwrap();
        mux.refresh_connection_ids(&conn_id, &mut pipe.server)
            .unwrap();
        assert_eq!(pipe.server.active_scids(), 1);
        assert!(mux.connection_ids.is_empty());
    }

    #[tokio::test]
    async fn departed_path_cid_is_retired_only_before_a_new_path() {
        let mut mux = multiplexer().await;
        let mut pipe = pipe();
        pipe.handshake().unwrap();
        pipe.advance().unwrap();
        let conn_id = pipe.server.source_id().into_owned();
        mux.refresh_connection_ids(&conn_id, &mut pipe.server)
            .unwrap();
        pipe.client
            .new_scid(&generate_connection_id(b"client-1"), 1, false)
            .unwrap();
        pipe.advance().unwrap();
        let mut migrations = PeerMigrations::default();
        let peer = SocketAddr::from(([127, 0, 0, 1], 1235));
        let next_peer = SocketAddr::from(([127, 0, 0, 1], 1236));
        retire_left_path_dcid(&mut migrations, &mut pipe.server, &peer, &mux.id);
        assert_eq!(migrations.next_to_retire, 0);

        pipe.client.probe_path(peer, Pipe::server_addr()).unwrap();
        pipe.advance().unwrap();
        assert_eq!(
            pipe.client.is_path_validated(peer, Pipe::server_addr()),
            Ok(true)
        );
        retire_left_path_dcid(&mut migrations, &mut pipe.server, &next_peer, &mux.id);
        assert_eq!(migrations.migrated, 0, "probing is not migration");
        assert_eq!(migrations.next_to_retire, 0);

        pipe.client.migrate(peer, Pipe::server_addr()).unwrap();
        pipe.client.stream_send(0, b"data", false).unwrap();
        pipe.advance().unwrap();
        retire_left_path_dcid(&mut migrations, &mut pipe.server, &peer, &mux.id);
        assert_eq!(migrations.migrated, 1);
        assert_eq!(migrations.next_to_retire, 0, "known paths keep their CID");
        assert!(pipe
            .server
            .path_stats()
            .any(|path| path.active && path.peer_addr == peer));
        let active = pipe.server.destination_id().into_owned();
        let original = pipe
            .client
            .source_ids()
            .find(|cid| **cid != active)
            .unwrap()
            .clone()
            .into_owned();
        retire_left_path_dcid(&mut migrations, &mut pipe.server, &next_peer, &mux.id);
        assert_eq!(migrations.next_to_retire, 1);
        assert_eq!(pipe.server.destination_id(), active);
        retire_left_path_dcid(&mut migrations, &mut pipe.server, &next_peer, &mux.id);
        assert_eq!(
            migrations.next_to_retire, 1,
            "do not retire the active path CID"
        );
        pipe.advance().unwrap();
        assert_eq!(pipe.client.retired_scid_next(), Some(original));
        let mut data = [0; 4];
        assert_eq!(pipe.server.stream_recv(0, &mut data).unwrap(), (4, false));
        assert_eq!(&data, b"data");
    }
}
