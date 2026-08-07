use crate::http1_codec::Http1Codec;
use crate::http_codec::HttpCodec;
use crate::tls_demultiplexer::Protocol;
use crate::traffic_limiter::{TrafficDirection, TrafficLimiter};
use crate::{core, http_codec, log_id, log_utils};
use bytes::Bytes;
use prometheus::Encoder;
use serde::Serialize;
use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};

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
    traffic_limiter: Option<Arc<TrafficLimiter>>,
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

#[derive(Serialize, Debug, PartialEq, Eq)]
pub(crate) struct ClientSummary {
    username: String,
    ip: Option<String>,
    sessions: u64,
    inbound: u64,
    outbound: u64,
    total: u64,
    limit: Option<u64>,
    quota_exceeded: bool,
}

pub(crate) struct ClientSessionsCounter {
    metrics: Arc<Metrics>,
    _protocol: Protocol,
}

pub(crate) struct OutboundTcpSocketCounter {
    metrics: Arc<Metrics>,
}

pub(crate) struct OutboundUdpSocketCounter {
    metrics: Arc<Metrics>,
}

impl Metrics {
    pub fn new(traffic_limiter: Option<Arc<TrafficLimiter>>) -> io::Result<Arc<Self>> {
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
            traffic_limiter,
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
            let label = username_label(c.username.as_deref());
            if let Some(proto) = c.protocol_label {
                for _ in 0..c.sessions {
                    self.client_sessions
                        .with_label_values(&[proto.as_str(), label])
                        .dec();
                }
            }
        }
    }

    pub fn transfer_session_username(
        &self,
        protocol: Protocol,
        conn_id: &str,
        username: Option<String>,
    ) {
        let mut clients = self.clients.lock().unwrap();
        let old_label = username_label(
            clients
                .get(conn_id)
                .and_then(|c| c.username.as_deref()),
        );
        let new_label = username_label(username.as_deref());

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

    pub fn connection_username(&self, conn_id: &str) -> Option<String> {
        self.clients
            .lock()
            .unwrap()
            .get(conn_id)
            .and_then(|c| c.username.clone())
    }

    pub fn add_inbound_bytes(&self, conn_id: log_utils::IdChain<u64>, n: usize) {
        if n == 0 {
            return;
        }

        let key = conn_id.to_string();
        let username = {
            let mut clients = self.clients.lock().unwrap();
            let username = clients
                .get_mut(&key)
                .and_then(|c| c.username.clone())
                .unwrap_or_default();
            if let Some(c) = clients.get_mut(&key) {
                c.inbound = c.inbound.saturating_add(n as u64);
            }
            username
        };

        let label = username_label(Some(username.as_str()).filter(|s| !s.is_empty()));
        self.inbound_traffic.with_label_values(&[label]).inc_by(n as u64);

        if !username.is_empty() {
            if let Some(limiter) = self.traffic_limiter.as_ref() {
                limiter.record(&username, TrafficDirection::Inbound, n);
            }
        }
    }

    pub fn add_outbound_bytes(&self, conn_id: log_utils::IdChain<u64>, n: usize) {
        if n == 0 {
            return;
        }

        let key = conn_id.to_string();
        let username = {
            let mut clients = self.clients.lock().unwrap();
            let username = clients
                .get_mut(&key)
                .and_then(|c| c.username.clone())
                .unwrap_or_default();
            if let Some(c) = clients.get_mut(&key) {
                c.outbound = c.outbound.saturating_add(n as u64);
            }
            username
        };

        let label = username_label(Some(username.as_str()).filter(|s| !s.is_empty()));
        self.outbound_traffic
            .with_label_values(&[label])
            .inc_by(n as u64);

        if !username.is_empty() {
            if let Some(limiter) = self.traffic_limiter.as_ref() {
                limiter.record(&username, TrafficDirection::Outbound, n);
            }
        }
    }

    pub fn build_client_summaries(
        &self,
        configured_usernames: impl IntoIterator<Item = String>,
    ) -> Vec<ClientSummary> {
        let mut agg: HashMap<String, ClientSummary> = HashMap::new();

        for username in configured_usernames {
            agg.entry(username.clone()).or_insert_with(|| ClientSummary {
                username,
                ip: None,
                sessions: 0,
                inbound: 0,
                outbound: 0,
                total: 0,
                limit: None,
                quota_exceeded: false,
            });
        }

        {
            let clients_map = self.clients.lock().unwrap();
            for info in clients_map.values() {
                let uname = info.username.clone().unwrap_or_default();
                let entry = agg.entry(uname.clone()).or_insert_with(|| ClientSummary {
                    username: uname.clone(),
                    ip: info.ip.map(|x| x.to_string()),
                    sessions: 0,
                    inbound: 0,
                    outbound: 0,
                    total: 0,
                    limit: None,
                    quota_exceeded: false,
                });
                entry.sessions = entry.sessions.saturating_add(info.sessions);
                entry.inbound = entry.inbound.saturating_add(info.inbound);
                entry.outbound = entry.outbound.saturating_add(info.outbound);
                if entry.ip.is_none() {
                    entry.ip = info.ip.map(|x| x.to_string());
                }
            }
        }

        if let Some(limiter) = self.traffic_limiter.as_ref() {
            for summary in agg.values_mut() {
                let persisted = limiter.summary(&summary.username);
                summary.inbound = summary.inbound.max(persisted.inbound);
                summary.outbound = summary.outbound.max(persisted.outbound);
                summary.limit = persisted.limit;
                summary.quota_exceeded = persisted.quota_exceeded;
            }
        }

        let mut summaries: Vec<_> = agg.into_values().collect();
        for summary in &mut summaries {
            summary.total = summary.inbound.saturating_add(summary.outbound);
        }
        summaries.sort_by(|a, b| a.username.cmp(&b.username));
        summaries
    }

    fn collect(&self) -> (String, Bytes) {
        let encoder = prometheus::TextEncoder::new();

        let mut metric_families = self._registry.gather();
        metric_families.extend(prometheus::gather());
        let mut buffer = vec![];
        encoder.encode(&metric_families, &mut buffer).unwrap();

        (encoder.format_type().to_string(), Bytes::from(buffer))
    }
}

impl ClientSessionsCounter {
    fn new(
        metrics: Arc<Metrics>,
        protocol: Protocol,
        conn_id: String,
        username: Option<String>,
    ) -> Self {
        let label = username_label(username.as_deref());
        metrics
            .client_sessions
            .with_label_values(&[protocol.as_str(), label])
            .inc();

        {
            let mut clients = metrics.clients.lock().unwrap();
            let entry = clients.entry(conn_id).or_default();
            if let Some(u) = username {
                entry.username = Some(u);
            }
            entry.sessions = entry.sessions.saturating_add(1);
            entry.protocol_label = Some(protocol.as_str().to_string());
        }

        Self {
            metrics,
            _protocol: protocol,
        }
    }
}

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
                Err(e) => Err(io::Error::other(format!("{}", e))),
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
            CLIENTS_PATH => handle_clients_collect(context.clone(), stream).await,
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
    context: Arc<core::Context>,
    stream: Box<dyn http_codec::Stream>,
) -> io::Result<()> {
    let mut summaries = context.metrics.build_client_summaries(
        context
            .settings
            .clients
            .iter()
            .map(|c| c.username.clone()),
    );

    for summary in &mut summaries {
        if summary.limit.is_none() {
            summary.limit = context
                .settings
                .clients
                .iter()
                .find(|c| c.username == summary.username)
                .and_then(|c| c.max_traffic_bytes)
                .or(context.settings.default_max_traffic_bytes_per_client);
        }
        if let Some(limit) = summary.limit {
            summary.quota_exceeded = summary.total >= limit;
        }
    }

    let content_vec = serde_json::to_vec(&summaries).unwrap_or_else(|_| b"[]".to_vec());
    let mut content = Bytes::from(content_vec);
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

    while !content.is_empty() {
        content = sink.write(content)?;
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

fn username_label(username: Option<&str>) -> &str {
    match username {
        Some(name) if !name.is_empty() => name,
        _ => "",
    }
}

fn prometheus_to_io_error(e: prometheus::Error) -> io::Error {
    match e {
        prometheus::Error::Io(e) => e,
        e => io::Error::other(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Settings, ValidationError};
    use crate::traffic_limiter::TrafficLimiter;
    use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
    use base64::Engine;
    use std::net::{IpAddr, Ipv4Addr};

    fn make_metrics() -> Arc<Metrics> {
        Metrics::new(None).unwrap()
    }

    #[test]
    fn transfer_session_username_rebalances_gauge() {
        let metrics = make_metrics();
        let conn_id = "conn-1".to_string();
        metrics.register_connection(conn_id.clone(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        let _guard = metrics.clone().client_sessions_counter(
            Protocol::Http2,
            conn_id.clone(),
            None,
        );

        assert_eq!(
            metrics
                .client_sessions
                .with_label_values(&[Protocol::Http2.as_str(), ""])
                .get(),
            1
        );

        metrics.transfer_session_username(Protocol::Http2, &conn_id, Some("alice".into()));

        assert_eq!(
            metrics
                .client_sessions
                .with_label_values(&[Protocol::Http2.as_str(), ""])
                .get(),
            0
        );
        assert_eq!(
            metrics
                .client_sessions
                .with_label_values(&[Protocol::Http2.as_str(), "alice"])
                .get(),
            1
        );
    }

    #[test]
    fn unregister_connection_decrements_session_gauge() {
        let metrics = make_metrics();
        let conn_id = "conn-2".to_string();
        metrics.register_connection(conn_id.clone(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        let _guard = metrics.clone().client_sessions_counter(
            Protocol::Http1,
            conn_id.clone(),
            Some("bob".into()),
        );

        metrics.unregister_connection(&conn_id);

        assert_eq!(
            metrics
                .client_sessions
                .with_label_values(&[Protocol::Http1.as_str(), "bob"])
                .get(),
            0
        );
    }

    #[test]
    fn build_client_summaries_merges_runtime_and_configured_clients() {
        let limiter = TrafficLimiter::new(
            &[crate::authentication::registry_based::Client {
                username: "alice".into(),
                password: "pass".into(),
                max_http2_conns: None,
                max_http3_conns: None,
                max_traffic_bytes: Some(1000),
            }],
            None,
            None,
        );
        let metrics = Metrics::new(Some(limiter)).unwrap();
        let conn_id: log_utils::IdChain<u64> = log_utils::IdItem::new("TEST={}", 1).into();
        let conn_key = conn_id.to_string();
        metrics.register_connection(conn_key.clone(), IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)));
        metrics.transfer_session_username(Protocol::Http2, &conn_key, Some("alice".into()));
        metrics.add_inbound_bytes(conn_id.clone(), 100);
        metrics.add_outbound_bytes(conn_id.clone(), 200);

        let summaries = metrics.build_client_summaries(["alice".into(), "bob".into()]);
        assert_eq!(summaries.len(), 2);

        let alice = summaries.iter().find(|x| x.username == "alice").unwrap();
        assert_eq!(alice.inbound, 100);
        assert_eq!(alice.outbound, 200);
        assert_eq!(alice.total, 300);
        assert_eq!(alice.limit, Some(1000));
        assert_eq!(alice.ip.as_deref(), Some("1.2.3.4"));

        let bob = summaries.iter().find(|x| x.username == "bob").unwrap();
        assert_eq!(bob.sessions, 0);
        assert_eq!(bob.inbound, 0);
        assert_eq!(bob.outbound, 0);
    }

    #[test]
    fn traffic_is_attributed_after_username_transfer() {
        let metrics = make_metrics();
        let conn_id: log_utils::IdChain<u64> = log_utils::IdItem::new("TEST={}", 42).into();
        metrics.register_connection(conn_id.to_string(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        metrics.transfer_session_username(Protocol::Http2, &conn_id.to_string(), Some("carol".into()));

        metrics.add_inbound_bytes(conn_id.clone(), 50);
        metrics.add_outbound_bytes(conn_id.clone(), 70);

        assert_eq!(
            metrics
                .inbound_traffic
                .with_label_values(&["carol"])
                .get(),
            50
        );
        assert_eq!(
            metrics
                .outbound_traffic
                .with_label_values(&["carol"])
                .get(),
            70
        );
    }

    #[test]
    fn traffic_usage_file_required_when_quotas_configured() {
        let err = Settings::builder()
            .clients(vec![authentication::registry_based::Client {
                username: "alice".into(),
                password: "pass".into(),
                max_http2_conns: None,
                max_http3_conns: None,
                max_traffic_bytes: Some(1000),
            }])
            .build()
            .unwrap_err();
        assert!(matches!(err, ValidationError::TrafficUsageFileRequired));
    }
}
