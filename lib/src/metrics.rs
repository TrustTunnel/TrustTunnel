use crate::http1_codec::Http1Codec;
use crate::http_codec::HttpCodec;
use crate::tls_demultiplexer::Protocol;
use crate::{core, http_codec, log_id, log_utils};
use bytes::Bytes;
use prometheus::Encoder;
use serde::Serialize;
use std::collections::HashMap;
use std::io;
use std::io::ErrorKind;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};
use serde_json;

const LOG_FMT: &str = "METRICS={}";
const HEALTH_CHECK_PATH: &str = "/health-check";
const METRICS_PATH: &str = "/metrics";
const CLIENTS_PATH: &str = "/clients";

pub(crate) struct Metrics {
    _registry: prometheus::Registry,
    client_sessions: prometheus::IntGaugeVec,
    inbound_traffic: prometheus::IntCounterVec,
    outbound_traffic: prometheus::IntCounterVec,
    outbound_tcp_sockets: prometheus::IntGauge,
    outbound_udp_sockets: prometheus::IntGauge,
    clients: Mutex<HashMap<String, ClientInfo>>,
}

#[derive(Debug, Default)]
struct ClientInfo {
    username: Option<String>,
    ip: Option<IpAddr>,
    sessions: u64,
    protocol_label: Option<String>,
    inbound: u64,
    outbound: u64,
}

#[derive(Serialize)]
struct ClientSummary {
    username: String,
    ip: Option<String>,
    sessions: u64,
    inbound: u64,
    outbound: u64,
}

pub(crate) struct ClientSessionsCounter {
    metrics: Arc<Metrics>,
    protocol: Protocol,
}

pub(crate) struct OutboundTcpSocketCounter {
    metrics: Arc<Metrics>,
}

pub(crate) struct OutboundUdpSocketCounter {
    metrics: Arc<Metrics>,
}

impl Metrics {
    pub fn new() -> io::Result<Arc<Self>> {
        let registry = prometheus::Registry::new();
        Ok(Arc::new(Self {
            client_sessions: prometheus::register_int_gauge_vec_with_registry!(
                "client_sessions",
                "Number of active client sessions",
                &["protocol_type", "username"],
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            inbound_traffic: prometheus::register_int_counter_vec_with_registry!(
                "inbound_traffic_bytes",
                "Total number of bytes uploaded by clients",
                &["username"],
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            outbound_traffic: prometheus::register_int_counter_vec_with_registry!(
                "outbound_traffic_bytes",
                "Total number of bytes downloaded by clients",
                &["username"],
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            outbound_tcp_sockets: prometheus::register_int_gauge_with_registry!(
                "outbound_tcp_sockets",
                "Number of active outbound TCP connections",
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            outbound_udp_sockets: prometheus::register_int_gauge_with_registry!(
                "outbound_udp_sockets",
                "Number of active outbound UDP sockets",
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            _registry: registry,
            clients: Mutex::new(HashMap::new()),
        }))
    }

    pub fn client_sessions_counter(
        self: Arc<Self>,
        protocol: Protocol,
        conn_id: String,
        username: Option<String>,
    ) -> ClientSessionsCounter {
        ClientSessionsCounter::new(self, protocol, conn_id, username)
    }

    pub fn outbound_tcp_socket_counter(self: Arc<Self>) -> OutboundTcpSocketCounter {
        OutboundTcpSocketCounter::new(self)
    }

    pub fn outbound_udp_socket_counter(self: Arc<Self>) -> OutboundUdpSocketCounter {
        OutboundUdpSocketCounter::new(self)
    }

    pub fn add_inbound_bytes(&self, _protocol: Protocol, conn_id: log_utils::IdChain<u64>, n: usize) {
        let key = conn_id.to_string();
        let mut clients = self.clients.lock().unwrap();
        let username = clients
            .get_mut(&key)
            .and_then(|c| c.username.clone())
            .unwrap_or_default();
        let label = if username.is_empty() { "" } else { &username };
        self.inbound_traffic
            .with_label_values(&[label])
            .inc_by(n as u64);
        if let Some(c) = clients.get_mut(&key) {
            c.inbound = c.inbound.saturating_add(n as u64);
        }
    }

    pub fn add_outbound_bytes(&self, _protocol: Protocol, conn_id: log_utils::IdChain<u64>, n: usize) {
        let key = conn_id.to_string();
        let mut clients = self.clients.lock().unwrap();
        let username = clients
            .get_mut(&key)
            .and_then(|c| c.username.clone())
            .unwrap_or_default();
        let label = if username.is_empty() { "" } else { &username };
        self.outbound_traffic
            .with_label_values(&[label])
            .inc_by(n as u64);
        if let Some(c) = clients.get_mut(&key) {
            c.outbound = c.outbound.saturating_add(n as u64);
        }
    }

    fn collect(&self) -> (String, Bytes) {
        let encoder = prometheus::TextEncoder::new();

        let mut metric_families = self._registry.gather();
        metric_families.extend(prometheus::gather());
        let mut buffer = vec![];
        encoder.encode(&metric_families, &mut buffer).unwrap();

        (encoder.format_type().to_string(), Bytes::from(buffer))
    }

    pub fn register_connection(&self, conn_id: String, ip: IpAddr) {
        let mut clients = self.clients.lock().unwrap();
        clients.entry(conn_id).or_insert_with(|| ClientInfo {
            username: None,
            ip: Some(ip),
            sessions: 0,
            protocol_label: None,
            inbound: 0,
            outbound: 0,
        });
    }

    pub fn unregister_connection(&self, conn_id: &str) {
        let mut clients = self.clients.lock().unwrap();
        if let Some(c) = clients.remove(conn_id) {
            let uname = c.username.unwrap_or_default();
            let label = if uname.is_empty() { "" } else { &uname };
            if let Some(proto) = c.protocol_label {
                for _ in 0..c.sessions {
                    self.client_sessions
                        .with_label_values(&[proto.as_str(), label])
                        .dec();
                }
            }
        }
    }

    pub fn set_connection_username(&self, conn_id: &str, username: Option<String>) {
        let mut clients = self.clients.lock().unwrap();
        if let Some(c) = clients.get_mut(conn_id) {
            c.username = username;
        }
    }

    pub fn transfer_session_username(&self, protocol: Protocol, conn_id: &str, username: Option<String>) {
        let mut clients = self.clients.lock().unwrap();
        let old = clients
            .get(conn_id)
            .and_then(|c| c.username.clone())
            .unwrap_or_default();
        let old_label = if old.is_empty() { "" } else { &old };
        let new_label = username.as_deref().unwrap_or("");
        // adjust prometheus gauges
        self.client_sessions
            .with_label_values(&[protocol.as_str(), old_label])
            .dec();
        self.client_sessions
            .with_label_values(&[protocol.as_str(), new_label])
            .inc();

        if let Some(c) = clients.get_mut(conn_id) {
            c.username = username;
        }
    }

    fn clients_json(&self) -> Bytes {
        let clients = self.clients.lock().unwrap();
        // aggregate by username
        let mut agg: HashMap<String, ClientSummary> = HashMap::new();
        for (_id, info) in clients.iter() {
            let uname = info.username.clone().unwrap_or_default();
            let entry = agg.entry(uname.clone()).or_insert(ClientSummary {
                username: uname.clone(),
                ip: info.ip.map(|x| x.to_string()),
                sessions: 0,
                inbound: 0,
                outbound: 0,
            });
            entry.sessions = entry.sessions.saturating_add(info.sessions);
            entry.inbound = entry.inbound.saturating_add(info.inbound);
            entry.outbound = entry.outbound.saturating_add(info.outbound);
            if entry.ip.is_none() {
                entry.ip = info.ip.map(|x| x.to_string());
            }
        }

        let vec: Vec<ClientSummary> = agg.into_values().collect();
        let js = serde_json::to_vec(&vec).unwrap_or_else(|_| b"[]".to_vec());
        Bytes::from(js)
    }
}

impl ClientSessionsCounter {
    fn new(
        metrics: Arc<Metrics>,
        protocol: Protocol,
        conn_id: String,
        username: Option<String>,
    ) -> Self {
        let label = username.as_deref().unwrap_or("");
        metrics
            .client_sessions
            .with_label_values(&[protocol.as_str(), label])
            .inc();

        // Ensure client entry exists and update
        let mut clients = metrics.clients.lock().unwrap();
        let entry = clients.entry(conn_id.clone()).or_default();
        if let Some(u) = username.clone() {
            entry.username = Some(u);
        }
        entry.sessions = entry.sessions.saturating_add(1);
        entry.protocol_label = Some(protocol.as_str().to_string());

        Self { metrics, protocol }
    }
}

// Session gauge lifecycle is managed via explicit register/unregister of connections.

impl OutboundTcpSocketCounter {
    fn new(metrics: Arc<Metrics>) -> Self {
        metrics.outbound_tcp_sockets.inc();
        Self { metrics }
    }
}

impl Drop for OutboundTcpSocketCounter {
    fn drop(&mut self) {
        self.metrics.outbound_tcp_sockets.dec();
    }
}

impl OutboundUdpSocketCounter {
    fn new(metrics: Arc<Metrics>) -> Self {
        metrics.outbound_udp_sockets.inc();
        Self { metrics }
    }
}

impl Drop for OutboundUdpSocketCounter {
    fn drop(&mut self) {
        self.metrics.outbound_udp_sockets.dec();
    }
}

pub(crate) async fn listen(
    context: Arc<core::Context>,
    log_chain: log_utils::IdChain<u64>,
) -> io::Result<()> {
    let (mut shutdown_notification, _shutdown_completion) = {
        let shutdown = context.shutdown.lock().unwrap();
        (shutdown.notification_handler(), shutdown.completion_guard())
    };

    tokio::select! {
        x = shutdown_notification.wait() => {
            match x {
                Ok(_) => Ok(()),
                Err(e) => Err(io::Error::new(ErrorKind::Other, format!("{}", e))),
            }
        }
        x = listen_inner(context, log_chain) => x,
    }
}

async fn listen_inner(
    context: Arc<core::Context>,
    log_chain: log_utils::IdChain<u64>,
) -> io::Result<()> {
    let settings = context.settings.metrics.as_ref();
    if settings.is_none() {
        return Ok(());
    }

    let next_id = AtomicU64::default();
    let listener = TcpListener::bind(settings.unwrap().address).await?;

    loop {
        let (stream, peer) = listener.accept().await?;
        let log_id = log_chain.extended(log_utils::IdItem::new(
            LOG_FMT,
            next_id.fetch_add(1, Ordering::Relaxed),
        ));
        log_id!(trace, log_id, "New connection from {}", peer);
        let context = context.clone();
        tokio::spawn(async move { handle_request(context, stream, log_id).await });
    }
}

async fn handle_request(
    context: Arc<core::Context>,
    io: TcpStream,
    log_id: log_utils::IdChain<u64>,
) {
    let mut codec = Http1Codec::new(context.settings.clone(), io, log_id.clone());
    let timeout = context.settings.metrics.as_ref().unwrap().request_timeout;
    let stream = match tokio::time::timeout(timeout, codec.listen()).await {
        Ok(Ok(Some(x))) => {
            log_id!(trace, log_id, "Got request: {:?}", x.request().request());
            x
        }
        Ok(Ok(None)) => {
            log_id!(debug, log_id, "Connection closed immediately");
            return;
        }
        Ok(Err(e)) => {
            log_id!(debug, log_id, "Listen failed: {}", e);
            return;
        }
        Err(_elapsed) => {
            log_id!(
                debug,
                log_id,
                "Didn't receive any request during configured period"
            );
            return;
        }
    };

    let dispatch = async {
        match codec.listen().await {
            Ok(Some(x)) => log_id!(
                debug,
                log_id,
                "Got unexpected request while processing previous: {:?}",
                x.request().request(),
            ),
            Ok(None) => (),
            Err(e) => log_id!(debug, log_id, "IO error during processing: {}", e),
        }
    };

    let handle = async {
        let path = stream.request().request().uri.path();
        let result = match path {
            HEALTH_CHECK_PATH => handle_health_check(stream),
            METRICS_PATH => handle_metrics_collect(&context.metrics, stream).await,
            CLIENTS_PATH => handle_clients_collect(&context.metrics, stream).await,
            x => {
                log_id!(debug, log_id, "Unexpected path: {}", x);
                let respond = stream.split().1;
                if let Err(e) =
                    respond.send_bad_response(http::status::StatusCode::BAD_REQUEST, vec![])
                {
                    log_id!(debug, log_id, "Failed to send response: {}", e);
                }
                return;
            }
        };

        if let Err(e) = result {
            log_id!(debug, log_id, "Failed to handle request: {}", e);
        }
    };

    tokio::select! {
        _ = dispatch => (),
        _ = handle => (),
    }

    if let Err(e) = codec.graceful_shutdown().await {
        log_id!(debug, log_id, "Failed to shutdown HTTP session: {}", e);
    }
}

fn handle_health_check(stream: Box<dyn http_codec::Stream>) -> io::Result<()> {
    stream.split().1.send_ok_response(true).map(|_| ())
}

async fn handle_clients_collect(
    metrics: &Metrics,
    stream: Box<dyn http_codec::Stream>,
) -> io::Result<()> {
    let content = metrics.clients_json();
    let response = http::Response::builder()
        .version(stream.request().request().version)
        .status(http::status::StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(http::header::CONTENT_LENGTH, content.len())
        .body(())
        .unwrap()
        .into_parts()
        .0;

    let mut sink = stream
        .split()
        .1
        .send_response(response, false)?
        .into_pipe_sink();

    let mut buf = content;
    while !buf.is_empty() {
        buf = sink.write(buf)?;
        sink.wait_writable().await?;
    }

    sink.eof()
}

async fn handle_metrics_collect(
    metrics: &Metrics,
    stream: Box<dyn http_codec::Stream>,
) -> io::Result<()> {
    let (content_type, mut content) = metrics.collect();
    let response = http::Response::builder()
        .version(stream.request().request().version)
        .status(http::status::StatusCode::OK)
        .header(http::header::CONTENT_TYPE, content_type)
        .header(http::header::CONTENT_LENGTH, content.len())
        .body(())
        .unwrap()
        .into_parts()
        .0;

    let mut sink = stream
        .split()
        .1
        .send_response(response, false)?
        .into_pipe_sink();

    while !content.is_empty() {
        content = sink.write(content)?;
        sink.wait_writable().await?;
    }

    sink.eof()
}

fn prometheus_to_io_error(e: prometheus::Error) -> io::Error {
    match e {
        prometheus::Error::Io(e) => e,
        e => io::Error::new(ErrorKind::Other, e.to_string()),
    }
}
