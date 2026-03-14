use crate::http1_codec::Http1Codec;
use crate::http_codec::HttpCodec;
use crate::tls_demultiplexer::Protocol;
use crate::{core, http_codec, log_id, log_utils};
use bytes::Bytes;
use once_cell::sync::Lazy;
use prometheus::Encoder;
use std::io;
use std::io::ErrorKind;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};

const LOG_FMT: &str = "METRICS={}";
const HEALTH_CHECK_PATH: &str = "/health-check";
const METRICS_PATH: &str = "/metrics";

pub(crate) struct Metrics {
    _registry: prometheus::Registry,
    client_sessions: prometheus::IntGaugeVec,
    active_connections: prometheus::IntGaugeVec,
    inbound_traffic: prometheus::IntCounterVec,
    outbound_traffic: prometheus::IntCounterVec,
    traffic_bytes: prometheus::IntCounterVec,
    handshake_duration_seconds: prometheus::HistogramVec,
    request_latency_seconds: prometheus::HistogramVec,
    outbound_tcp_sockets: prometheus::IntGauge,
    outbound_udp_sockets: prometheus::IntGauge,
    session_guard_active_sessions_total: prometheus::IntGauge,
    session_guard_rejections_total: prometheus::IntCounter,
    session_guard_stale_reaped_total: prometheus::IntCounter,
    session_guard_users_at_limit: prometheus::IntGauge,
    session_guard_registry_size: prometheus::IntGauge,
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

pub(crate) struct ActiveConnectionCounter {
    metrics: Arc<Metrics>,
    connection_type: &'static str,
}

pub(crate) struct RequestLatencyObserver {
    metrics: Arc<Metrics>,
    tunnel_type: &'static str,
    started: Instant,
}

static AUTH_BASIC_SUCCESS_TOTAL: Lazy<prometheus::IntCounter> = Lazy::new(|| {
    let counter = prometheus::IntCounter::new(
        "auth_basic_success_total",
        "Total number of successful Basic authentication attempts",
    )
    .unwrap();
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .ok();
    counter
});

static AUTH_BASIC_FAILURE_TOTAL: Lazy<prometheus::IntCounter> = Lazy::new(|| {
    let counter = prometheus::IntCounter::new(
        "auth_basic_failure_total",
        "Total number of failed Basic authentication attempts",
    )
    .unwrap();
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .ok();
    counter
});

static AUTH_JWT_SUCCESS_TOTAL: Lazy<prometheus::IntCounter> = Lazy::new(|| {
    let counter = prometheus::IntCounter::new(
        "auth_jwt_success_total",
        "Total number of successful JWT authentication attempts",
    )
    .unwrap();
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .ok();
    counter
});

static AUTH_JWT_FAILURE_TOTAL: Lazy<prometheus::IntCounter> = Lazy::new(|| {
    let counter = prometheus::IntCounter::new(
        "auth_jwt_failure_total",
        "Total number of failed JWT authentication attempts",
    )
    .unwrap();
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .ok();
    counter
});

static JWT_VALIDATION_ERRORS_TOTAL: Lazy<prometheus::IntCounterVec> = Lazy::new(|| {
    let mut labels = std::collections::HashMap::new();
    labels.insert("instance".to_string(), default_node_label());

    let counter = prometheus::Opts::new(
        "vpn_jwt_validation_errors_total",
        "Total number of failed JWT validation attempts",
    )
    .const_labels(labels);

    let metric = prometheus::IntCounterVec::new(counter, &["reason"]).unwrap();
    prometheus::default_registry()
        .register(Box::new(metric.clone()))
        .ok();
    metric
});

impl Metrics {
    pub fn new() -> io::Result<Arc<Self>> {
        let mut labels = std::collections::HashMap::new();
        labels.insert("instance".to_string(), default_node_label());
        let registry =
            prometheus::Registry::new_custom(None, Some(labels)).map_err(prometheus_to_io_error)?;
        Ok(Arc::new(Self {
            client_sessions: prometheus::register_int_gauge_vec_with_registry!(
                "client_sessions",
                "Number of active client sessions",
                &["protocol_type"],
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            active_connections: prometheus::register_int_gauge_vec_with_registry!(
                "vpn_active_connections",
                "Current number of active tunnels",
                &["connection_type"],
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            inbound_traffic: prometheus::register_int_counter_vec_with_registry!(
                "inbound_traffic_bytes",
                "Total number of bytes uploaded by clients",
                &["protocol_type"],
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            outbound_traffic: prometheus::register_int_counter_vec_with_registry!(
                "outbound_traffic_bytes",
                "Total number of bytes downloaded by clients",
                &["protocol_type"],
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            traffic_bytes: prometheus::register_int_counter_vec_with_registry!(
                "vpn_traffic_bytes_total",
                "Total number of tunneled bytes split by direction",
                &["protocol_type", "direction"],
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            handshake_duration_seconds: prometheus::register_histogram_vec_with_registry!(
                "vpn_handshake_duration_seconds",
                "Duration of TLS handshake/channel establishment",
                &["protocol"],
                vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0],
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            request_latency_seconds: prometheus::register_histogram_vec_with_registry!(
                "vpn_request_latency_seconds",
                "Tunnel request processing latency",
                &["tunnel_type"],
                vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0],
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
            session_guard_active_sessions_total: prometheus::register_int_gauge_with_registry!(
                "session_guard_active_sessions_total",
                "Current total active client sessions tracked by session guard",
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            session_guard_rejections_total: prometheus::register_int_counter_with_registry!(
                "session_guard_rejections_total",
                "Total number of rejected session acquisitions due to max per-user limit",
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            session_guard_stale_reaped_total: prometheus::register_int_counter_with_registry!(
                "session_guard_stale_reaped_total",
                "Total number of stale sessions reaped by session guard cleaner",
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            session_guard_users_at_limit: prometheus::register_int_gauge_with_registry!(
                "session_guard_users_at_limit",
                "Current number of users with active sessions equal to or above limit",
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            session_guard_registry_size: prometheus::register_int_gauge_with_registry!(
                "session_guard_registry_size",
                "Current number of users in session guard registry",
                registry,
            )
            .map_err(prometheus_to_io_error)?,
            _registry: registry,
        }))
    }

    pub fn client_sessions_counter(self: Arc<Self>, protocol: Protocol) -> ClientSessionsCounter {
        ClientSessionsCounter::new(self, protocol)
    }

    pub fn outbound_tcp_socket_counter(self: Arc<Self>) -> OutboundTcpSocketCounter {
        OutboundTcpSocketCounter::new(self)
    }

    pub fn outbound_udp_socket_counter(self: Arc<Self>) -> OutboundUdpSocketCounter {
        OutboundUdpSocketCounter::new(self)
    }

    pub fn add_inbound_bytes(&self, protocol: Protocol, n: usize) {
        self.inbound_traffic
            .with_label_values(&[protocol.as_str()])
            .inc_by(n as u64);
        self.traffic_bytes
            .with_label_values(&[protocol.as_str(), "in"])
            .inc_by(n as u64);
    }

    pub fn add_outbound_bytes(&self, protocol: Protocol, n: usize) {
        self.outbound_traffic
            .with_label_values(&[protocol.as_str()])
            .inc_by(n as u64);
        self.traffic_bytes
            .with_label_values(&[protocol.as_str(), "out"])
            .inc_by(n as u64);
    }

    pub fn active_connection_counter(
        self: Arc<Self>,
        connection_type: &'static str,
    ) -> ActiveConnectionCounter {
        ActiveConnectionCounter::new(self, connection_type)
    }

    pub fn request_latency_observer(
        self: Arc<Self>,
        tunnel_type: &'static str,
    ) -> RequestLatencyObserver {
        RequestLatencyObserver::new(self, tunnel_type)
    }

    pub fn set_session_guard_active_sessions_total(&self, value: i64) {
        self.session_guard_active_sessions_total.set(value);
    }

    pub fn inc_session_guard_rejections_total(&self) {
        self.session_guard_rejections_total.inc();
    }

    pub fn inc_session_guard_stale_reaped_total_by(&self, value: u64) {
        self.session_guard_stale_reaped_total.inc_by(value);
    }

    pub fn set_session_guard_users_at_limit(&self, value: i64) {
        self.session_guard_users_at_limit.set(value);
    }

    pub fn set_session_guard_registry_size(&self, value: i64) {
        self.session_guard_registry_size.set(value);
    }

    pub fn observe_handshake_duration(&self, protocol: Protocol, duration: Duration) {
        self.handshake_duration_seconds
            .with_label_values(&[protocol.as_str()])
            .observe(duration.as_secs_f64());
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
    fn new(metrics: Arc<Metrics>, protocol: Protocol) -> Self {
        metrics
            .client_sessions
            .with_label_values(&[protocol.as_str()])
            .inc();

        Self { metrics, protocol }
    }
}

impl Drop for ClientSessionsCounter {
    fn drop(&mut self) {
        self.metrics
            .client_sessions
            .with_label_values(&[self.protocol.as_str()])
            .dec();
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

impl ActiveConnectionCounter {
    fn new(metrics: Arc<Metrics>, connection_type: &'static str) -> Self {
        metrics
            .active_connections
            .with_label_values(&[connection_type])
            .inc();

        Self {
            metrics,
            connection_type,
        }
    }
}

impl Drop for ActiveConnectionCounter {
    fn drop(&mut self) {
        self.metrics
            .active_connections
            .with_label_values(&[self.connection_type])
            .dec();
    }
}

impl RequestLatencyObserver {
    fn new(metrics: Arc<Metrics>, tunnel_type: &'static str) -> Self {
        Self {
            metrics,
            tunnel_type,
            started: Instant::now(),
        }
    }
}

impl Drop for RequestLatencyObserver {
    fn drop(&mut self) {
        self.metrics
            .request_latency_seconds
            .with_label_values(&[self.tunnel_type])
            .observe(self.started.elapsed().as_secs_f64());
    }
}

pub(crate) fn add_jwt_validation_error(reason: &str) {
    JWT_VALIDATION_ERRORS_TOTAL
        .with_label_values(&[reason])
        .inc();
}

pub(crate) fn add_auth_basic_success() {
    AUTH_BASIC_SUCCESS_TOTAL.inc();
}

pub(crate) fn add_auth_basic_failure() {
    AUTH_BASIC_FAILURE_TOTAL.inc();
}

pub(crate) fn add_auth_jwt_success() {
    AUTH_JWT_SUCCESS_TOTAL.inc();
}

pub(crate) fn add_auth_jwt_failure() {
    AUTH_JWT_FAILURE_TOTAL.inc();
}

fn default_node_label() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string())
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
    if !settings.is_some_and(|x| x.enabled) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_connections_gauge_changes() {
        let metrics = Metrics::new().unwrap();
        {
            let _guard = metrics.clone().active_connection_counter("tcp");
            assert_eq!(
                metrics.active_connections.with_label_values(&["tcp"]).get(),
                1
            );
        }

        assert_eq!(
            metrics.active_connections.with_label_values(&["tcp"]).get(),
            0
        );
    }

    #[test]
    fn jwt_error_counter_increments() {
        let before = JWT_VALIDATION_ERRORS_TOTAL
            .with_label_values(&["invalid_token"])
            .get();
        add_jwt_validation_error("invalid_token");
        let after = JWT_VALIDATION_ERRORS_TOTAL
            .with_label_values(&["invalid_token"])
            .get();
        assert_eq!(after, before + 1);
    }

    #[test]
    fn auth_method_counters_increment() {
        let before_basic_success = AUTH_BASIC_SUCCESS_TOTAL.get();
        let before_basic_failure = AUTH_BASIC_FAILURE_TOTAL.get();
        let before_jwt_success = AUTH_JWT_SUCCESS_TOTAL.get();
        let before_jwt_failure = AUTH_JWT_FAILURE_TOTAL.get();

        add_auth_basic_success();
        add_auth_basic_failure();
        add_auth_jwt_success();
        add_auth_jwt_failure();

        assert_eq!(AUTH_BASIC_SUCCESS_TOTAL.get(), before_basic_success + 1);
        assert_eq!(AUTH_BASIC_FAILURE_TOTAL.get(), before_basic_failure + 1);
        assert_eq!(AUTH_JWT_SUCCESS_TOTAL.get(), before_jwt_success + 1);
        assert_eq!(AUTH_JWT_FAILURE_TOTAL.get(), before_jwt_failure + 1);
    }
}
