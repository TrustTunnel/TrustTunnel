use anyhow::{Context, Result};
use axum::extract::State as AxumState;
use axum::http::StatusCode as HttpStatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use chrono::Utc;
use log::{error, info, warn};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

#[derive(Clone)]
struct Config {
    lk_internal_base_url: String,
    internal_agent_token: String,
    node_id: String,
    sync_interval_seconds: u64,
    heartbeat_interval_seconds: u64,
    credentials_path: PathBuf,
    trusttunnel_reload_mode: String,
    trusttunnel_pid: i32,
    agent_port: u16,
    trusttunnel_tcp_addr: String,
}

const HEARTBEAT_MAX_RETRIES: usize = 3;
const HEARTBEAT_INITIAL_BACKOFF_SECONDS: u64 = 1;
const LK_REQUEST_TIMEOUT_SECONDS: u64 = 5;
const LK_CONNECT_TIMEOUT_SECONDS: u64 = 2;
const SYNC_MAX_RETRIES: usize = 3;
const SYNC_INITIAL_BACKOFF_SECONDS: u64 = 1;
const SYNC_CYCLE_BUDGET_SECONDS: u64 = 30;
// Sync cycle SLA formula:
// total <= ((SYNC_MAX_RETRIES + 1) * LK_REQUEST_TIMEOUT_SECONDS)
//        + sum(retry_backoff_sequence(SYNC_MAX_RETRIES, SYNC_INITIAL_BACKOFF_SECONDS))
//        + SYNC_REPORT_TIMEOUT_SECONDS
// With defaults: (4 * 5s) + (1s + 2s + 4s) + 3s = 30s.
const SYNC_REPORT_TIMEOUT_SECONDS: u64 = 3;
const TCP_REACHABILITY_TIMEOUT_SECONDS: u64 = 2;

fn retry_backoff_sequence(
    max_retries: usize,
    initial_backoff: Duration,
    max_backoff: Duration,
) -> Vec<Duration> {
    let mut backoff = initial_backoff;
    (0..max_retries)
        .map(|_| {
            let current = backoff;
            backoff = (backoff * 2).min(max_backoff);
            current
        })
        .collect()
}

fn sync_cycle_worst_case_duration(
    max_retries: usize,
    initial_backoff: Duration,
    fetch_timeout: Duration,
    report_timeout: Duration,
) -> Duration {
    let fetch_attempts = max_retries.saturating_add(1) as u32;
    let fetch_total = fetch_timeout.saturating_mul(fetch_attempts);
    let backoff_total: Duration =
        retry_backoff_sequence(max_retries, initial_backoff, Duration::from_secs(30))
            .into_iter()
            .sum();
    fetch_total + backoff_total + report_timeout
}

impl Config {
    fn from_env() -> Result<Self> {
        Ok(Self {
            lk_internal_base_url: std::env::var("LK_INTERNAL_BASE_URL")?,
            internal_agent_token: std::env::var("INTERNAL_AGENT_TOKEN")?,
            node_id: std::env::var("NODE_ID")?,
            sync_interval_seconds: std::env::var("SYNC_INTERVAL_SECONDS")
                .unwrap_or_else(|_| "30".to_string())
                .parse()?,
            heartbeat_interval_seconds: std::env::var("HEARTBEAT_INTERVAL_SECONDS")
                .unwrap_or_else(|_| "10".to_string())
                .parse()?,
            credentials_path: PathBuf::from(
                std::env::var("CREDENTIALS_PATH")
                    .unwrap_or_else(|_| "/runtime/accounts.toml".to_string()),
            ),
            trusttunnel_reload_mode: std::env::var("TRUSTTUNNEL_RELOAD_MODE")
                .unwrap_or_else(|_| "signal".to_string()),
            trusttunnel_pid: std::env::var("TRUSTTUNNEL_PID")
                .unwrap_or_else(|_| "1".to_string())
                .parse()?,
            agent_port: std::env::var("AGENT_PORT")
                .unwrap_or_else(|_| "9105".to_string())
                .parse()?,
            trusttunnel_tcp_addr: std::env::var("TRUSTTUNNEL_TCP_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:8443".to_string()),
        })
    }
}

#[derive(Deserialize)]
struct SnapshotResponse {
    version: String,
    accounts: Vec<Account>,
    checksum: String,
}

#[derive(Clone, Deserialize, Serialize)]
struct Account {
    username: String,
    password: String,
    enabled: bool,
}

#[derive(Serialize)]
struct SyncReport {
    node_id: String,
    version: String,
    applied_count: usize,
    status: String,
    error: Option<String>,
    timestamp: String,
}

#[derive(Clone)]
struct SyncOutcome {
    version: String,
    status: String,
    applied_count: usize,
    error: Option<String>,
}

#[derive(Serialize)]
struct HeartbeatPayload<'a> {
    node_id: &'a str,
    status: &'a str,
    metrics_json: HeartbeatMetrics,
}

#[derive(Serialize)]
struct HeartbeatMetrics {
    active_connections: u64,
    cpu_percent: f64,
    mem_percent: f64,
    rx_mbps: f64,
    tx_mbps: f64,
}

#[derive(Serialize)]
struct CredentialsFile {
    client: Vec<AccountCredential>,
}

#[derive(Serialize)]
struct AccountCredential {
    username: String,
    password: String,
}

#[derive(Default)]
struct SyncDurationState {
    last_seconds: f64,
    sum_seconds: f64,
    count: u64,
}

#[derive(Default)]
struct AgentState {
    last_version: Option<String>,
    last_checksum: Option<String>,
    last_network_total: Option<u64>,
    last_sync_successful: bool,
    last_sync_timestamp: Option<i64>,
    sync_duration: SyncDurationState,
    sync_success_total: u64,
    sync_failure_total: u64,
    accounts_current: usize,
    heartbeat_success_total: u64,
    heartbeat_failure_total: u64,
    lk_reachable: bool,
    lk_timeout_count: u64,
    lk_error_count: u64,
    trusttunnel_tcp_reachable: bool,
}

type SharedAgentState = Arc<Mutex<AgentState>>;

#[derive(Clone, Copy)]
enum LkErrorKind {
    Timeout,
    Other,
}

const LK_TIMEOUT_ERROR_TAG: &str = "lk_timeout";
const LK_OTHER_ERROR_TAG: &str = "lk_error";

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let cfg = Config::from_env()?;
    let state: SharedAgentState = Arc::new(Mutex::new(AgentState::default()));

    let http_state = state.clone();
    let http_port = cfg.agent_port;
    tokio::spawn(async move {
        if let Err(err) = run_http_server(http_port, http_state).await {
            error!("http server failed: {err:#}");
        }
    });

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(LK_CONNECT_TIMEOUT_SECONDS))
        .timeout(Duration::from_secs(LK_REQUEST_TIMEOUT_SECONDS))
        .build()?;

    let mut sync_interval = tokio::time::interval(Duration::from_secs(cfg.sync_interval_seconds));
    let mut heartbeat_interval =
        tokio::time::interval(Duration::from_secs(cfg.heartbeat_interval_seconds.max(1)));
    let heartbeat_in_flight = Arc::new(AtomicBool::new(false));

    loop {
        tokio::select! {
            _ = sync_interval.tick() => {
                if let Err(e) = sync_once(&cfg, &client, state.clone()).await {
                    error!("credentials sync failed: {e:#}");
                }
            }
            _ = heartbeat_interval.tick() => {
                if heartbeat_in_flight
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
                {
                    warn!("heartbeat push skipped: previous attempt still in progress");
                    continue;
                }

                let cfg = cfg.clone();
                let client = client.clone();
                let state = state.clone();
                let heartbeat_in_flight = heartbeat_in_flight.clone();

                tokio::spawn(async move {
                    let result = push_metrics_with_retry(
                        &cfg,
                        &client,
                        state,
                        HEARTBEAT_MAX_RETRIES,
                        Duration::from_secs(HEARTBEAT_INITIAL_BACKOFF_SECONDS),
                    )
                    .await;

                    if let Err(e) = result {
                        warn!("metrics push failed: {}", sanitize_error_message(&format!("{e:#}")));
                    }

                    heartbeat_in_flight.store(false, Ordering::SeqCst);
                });
            }
            _ = tokio::signal::ctrl_c() => {
                info!("sidecar agent interrupted");
                return Ok(());
            }
        }
    }
}

async fn run_http_server(port: u16, state: SharedAgentState) -> Result<()> {
    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/healthz", get(healthz_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    info!("agent HTTP server listening on 0.0.0.0:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn metrics_handler(AxumState(state): AxumState<SharedAgentState>) -> impl IntoResponse {
    let state = state.lock().await;
    let last_sync_timestamp_seconds = state.last_sync_timestamp.unwrap_or(0);

    let body = format!(
        "# TYPE agent_last_sync_timestamp_seconds gauge\nagent_last_sync_timestamp_seconds {}\n# TYPE agent_last_sync_timestamp gauge\nagent_last_sync_timestamp {}\n# TYPE agent_sync_duration_seconds gauge\nagent_sync_duration_seconds {}\n# TYPE agent_sync_duration_seconds_sum counter\nagent_sync_duration_seconds_sum {}\n# TYPE agent_sync_duration_seconds_count counter\nagent_sync_duration_seconds_count {}\n# TYPE agent_sync_success_total counter\nagent_sync_success_total {}\n# TYPE agent_sync_failure_total counter\nagent_sync_failure_total {}\n# TYPE agent_accounts_current gauge\nagent_accounts_current {}\n# TYPE agent_heartbeat_success_total counter\nagent_heartbeat_success_total {}\n# TYPE agent_heartbeat_failure_total counter\nagent_heartbeat_failure_total {}\n# TYPE agent_lk_timeout_total counter\nagent_lk_timeout_total {}\n# TYPE agent_lk_timeout_count counter\nagent_lk_timeout_count {}\n# TYPE agent_lk_error_total counter\nagent_lk_error_total {}\n# TYPE agent_lk_error_count counter\nagent_lk_error_count {}\n",
        last_sync_timestamp_seconds,
        last_sync_timestamp_seconds,
        state.sync_duration.last_seconds,
        state.sync_duration.sum_seconds,
        state.sync_duration.count,
        state.sync_success_total,
        state.sync_failure_total,
        state.accounts_current,
        state.heartbeat_success_total,
        state.heartbeat_failure_total,
        state.lk_timeout_count,
        state.lk_timeout_count,
        state.lk_error_count,
        state.lk_error_count,
    );

    ([("content-type", "text/plain; version=0.0.4")], body).into_response()
}

async fn healthz_handler(AxumState(state): AxumState<SharedAgentState>) -> impl IntoResponse {
    let state = state.lock().await;
    let healthy =
        state.last_sync_successful && state.lk_reachable && state.trusttunnel_tcp_reachable;
    if healthy {
        (HttpStatusCode::OK, "ok").into_response()
    } else {
        (HttpStatusCode::SERVICE_UNAVAILABLE, "unhealthy").into_response()
    }
}

async fn sync_once(cfg: &Config, client: &reqwest::Client, state: SharedAgentState) -> Result<()> {
    sync_once_with_retry(
        cfg,
        client,
        state,
        SYNC_MAX_RETRIES,
        Duration::from_secs(SYNC_INITIAL_BACKOFF_SECONDS),
    )
    .await
}

async fn sync_once_with_retry(
    cfg: &Config,
    client: &reqwest::Client,
    state: SharedAgentState,
    max_retries: usize,
    initial_backoff: Duration,
) -> Result<()> {
    sync_once_with_retry_budgeted(
        cfg,
        client,
        state,
        max_retries,
        initial_backoff,
        Duration::from_secs(SYNC_CYCLE_BUDGET_SECONDS),
        Duration::from_secs(LK_REQUEST_TIMEOUT_SECONDS),
        Duration::from_secs(SYNC_REPORT_TIMEOUT_SECONDS),
    )
    .await
}

async fn sync_once_with_retry_budgeted(
    cfg: &Config,
    client: &reqwest::Client,
    state: SharedAgentState,
    max_retries: usize,
    initial_backoff: Duration,
    cycle_budget: Duration,
    fetch_timeout: Duration,
    report_timeout: Duration,
) -> Result<()> {
    let sync_cycle_started_at = Instant::now();
    let sync_deadline = sync_cycle_started_at + cycle_budget;
    let (outcome, sync_result) = match fetch_snapshot_with_retry(
        cfg,
        client,
        max_retries,
        initial_backoff,
        sync_deadline,
        fetch_timeout,
    )
    .await
    {
        Ok(snapshot) => {
            let mut outcome = SyncOutcome {
                version: snapshot.version.clone(),
                status: "success".to_string(),
                applied_count: snapshot
                    .accounts
                    .iter()
                    .filter(|account| account.enabled)
                    .count(),
                error: None,
            };

            let is_new = {
                let state_lock = state.lock().await;
                state_lock.last_version.as_deref() != Some(&snapshot.version)
                    || state_lock.last_checksum.as_deref() != Some(&snapshot.checksum)
            };

            let sync_result = (|| -> Result<()> {
                if is_new {
                    validate_checksum(&snapshot).context("validate_checksum")?;
                    write_credentials_atomically(&cfg.credentials_path, &snapshot.accounts)
                        .context("write_credentials_atomically")?;
                    trigger_trusttunnel_reload(&cfg.trusttunnel_reload_mode, cfg.trusttunnel_pid)
                        .context("trigger_trusttunnel_reload")?;
                    info!("applied credentials snapshot version={}", snapshot.version);
                }
                Ok(())
            })();

            if sync_result.is_err() {
                outcome.status = "failed".to_string();
                outcome.error = sync_result
                    .as_ref()
                    .err()
                    .map(|e| format!("sync failed: {e}"));
            } else {
                let mut state_lock = state.lock().await;
                state_lock.last_version = Some(snapshot.version);
                state_lock.last_checksum = Some(snapshot.checksum);
                state_lock.accounts_current = outcome.applied_count;
            }

            (outcome, sync_result)
        }
        Err(err) => {
            let outcome = SyncOutcome {
                version: "unknown".to_string(),
                status: "failed".to_string(),
                applied_count: 0,
                error: Some(format!("fetch_snapshot failed: {err}")),
            };
            (outcome, Err(err))
        }
    };

    {
        let mut state_lock = state.lock().await;
        if sync_result.is_ok() {
            state_lock.last_sync_successful = true;
            state_lock.sync_success_total = state_lock.sync_success_total.saturating_add(1);
            state_lock.last_sync_timestamp = Some(Utc::now().timestamp());
            state_lock.lk_reachable = true;
        } else {
            state_lock.last_sync_successful = false;
            state_lock.sync_failure_total = state_lock.sync_failure_total.saturating_add(1);
            state_lock.lk_reachable = false;
            if let Err(err) = &sync_result {
                increment_lk_error_counter(&mut state_lock, err);
            }
        }
    }

    if let Some(report_budget_left) = remaining_budget(sync_deadline) {
        let effective_report_timeout = report_timeout.min(report_budget_left);
        if let Err(err) = post_sync_report(cfg, client, outcome, effective_report_timeout).await {
            warn!("sync report failed (best effort): {err:#}");
            let mut state_lock = state.lock().await;
            increment_lk_error_counter(&mut state_lock, &err);
        }
    } else {
        warn!("sync report skipped: sync cycle budget exhausted");
    }

    let sync_duration_seconds = sync_cycle_started_at.elapsed().as_secs_f64();
    let mut state_lock = state.lock().await;
    state_lock.sync_duration.last_seconds = sync_duration_seconds;
    state_lock.sync_duration.sum_seconds += sync_duration_seconds;
    state_lock.sync_duration.count = state_lock.sync_duration.count.saturating_add(1);

    sync_result
}

async fn fetch_snapshot_with_retry(
    cfg: &Config,
    client: &reqwest::Client,
    max_retries: usize,
    initial_backoff: Duration,
    sync_deadline: Instant,
    fetch_timeout: Duration,
) -> Result<SnapshotResponse> {
    fetch_snapshot_with_retry_and_sleep(
        cfg,
        client,
        max_retries,
        initial_backoff,
        sync_deadline,
        fetch_timeout,
        |duration| async move {
            tokio::time::sleep(duration).await;
        },
    )
    .await
}

async fn fetch_snapshot_with_retry_and_sleep<S, Fut>(
    cfg: &Config,
    client: &reqwest::Client,
    max_retries: usize,
    initial_backoff: Duration,
    sync_deadline: Instant,
    fetch_timeout: Duration,
    mut sleep: S,
) -> Result<SnapshotResponse>
where
    S: FnMut(Duration) -> Fut,
    Fut: Future<Output = ()>,
{
    let backoffs = retry_backoff_sequence(max_retries, initial_backoff, Duration::from_secs(30));
    for backoff in backoffs {
        let fetch_budget_left = ensure_budget_remaining(sync_deadline, "fetch accounts snapshot")?;
        let effective_timeout = fetch_timeout.min(fetch_budget_left);

        match fetch_snapshot(cfg, client, effective_timeout).await {
            Ok(snapshot) => return Ok(snapshot),
            Err(e) => {
                warn!("snapshot fetch failed: {e:#}; retrying in {:?}", backoff);

                let sleep_budget_left =
                    ensure_budget_remaining(sync_deadline, "retry backoff before next fetch")?;
                sleep(backoff.min(sleep_budget_left)).await;
            }
        }
    }

    let fetch_budget_left = ensure_budget_remaining(sync_deadline, "fetch accounts snapshot")?;
    let effective_timeout = fetch_timeout.min(fetch_budget_left);
    fetch_snapshot(cfg, client, effective_timeout).await
}

fn remaining_budget(sync_deadline: Instant) -> Option<Duration> {
    sync_deadline.checked_duration_since(Instant::now())
}

fn ensure_budget_remaining(sync_deadline: Instant, operation: &str) -> Result<Duration> {
    let remaining = remaining_budget(sync_deadline)
        .with_context(|| format!("sync budget exhausted before {operation}"))?;
    if remaining.is_zero() {
        anyhow::bail!("sync budget exhausted before {operation}");
    }
    Ok(remaining)
}

async fn fetch_snapshot(
    cfg: &Config,
    client: &reqwest::Client,
    timeout: Duration,
) -> Result<SnapshotResponse> {
    let url = format!("{}/internal/vpn/classic/accounts", cfg.lk_internal_base_url);

    let resp = client
        .get(url)
        .query(&[("node_id", &cfg.node_id)])
        .bearer_auth(&cfg.internal_agent_token)
        .timeout(timeout)
        .send()
        .await
        .map_err(|err| map_lk_request_error(err, "accounts snapshot"))?;

    if resp.status() != StatusCode::OK {
        return Err(lk_http_status_error("accounts snapshot", resp.status()));
    }

    resp.json()
        .await
        .map_err(|err| map_lk_request_error(err, "accounts snapshot decode"))
}

fn validate_checksum(snapshot: &SnapshotResponse) -> Result<()> {
    let checksum = canonical_checksum(&snapshot.accounts);
    if checksum != snapshot.checksum {
        anyhow::bail!("snapshot checksum mismatch");
    }
    Ok(())
}

fn canonical_checksum(accounts: &[Account]) -> String {
    let mut canonical_accounts = accounts.to_vec();
    canonical_accounts.sort_by(|a, b| {
        a.username
            .cmp(&b.username)
            .then_with(|| a.password.cmp(&b.password))
            .then_with(|| a.enabled.cmp(&b.enabled))
    });

    let entries = canonical_accounts
        .iter()
        .map(|account| {
            format!(
                "{{\"username\":\"{}\",\"password\":\"{}\",\"enabled\":{}}}",
                escape_json_string(&account.username),
                escape_json_string(&account.password),
                account.enabled
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let raw = format!("{{\"accounts\":[{}]}}", entries);

    format!("{:x}", Sha256::digest(raw.as_bytes()))
}

fn escape_json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0C}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c if c.is_control() => escaped.push_str(&format!("\\u{:04x}", c as u32)),
            c => escaped.push(c),
        }
    }

    escaped
}

fn write_credentials_atomically(path: &Path, accounts: &[Account]) -> Result<()> {
    let mut fs_ops = FileSystemAtomicWriteOps;
    write_credentials_atomically_with_ops(path, accounts, &mut fs_ops)
}

fn write_credentials_atomically_with_ops(
    path: &Path,
    accounts: &[Account],
    ops: &mut impl AtomicWriteOps,
) -> Result<()> {
    let parent = path.parent().context("credentials path without parent")?;
    ops.create_parent_dir(parent)
        .with_context(|| format!("create parent dir: {}", parent.display()))?;

    let tmp_path = parent.join(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));

    let enabled_credentials = accounts
        .iter()
        .filter(|account| account.enabled)
        .map(|account| AccountCredential {
            username: account.username.clone(),
            password: account.password.clone(),
        })
        .collect::<Vec<_>>();

    let toml = toml::to_string(&CredentialsFile {
        client: enabled_credentials,
    })?;

    ops.write_tmp_and_sync(&tmp_path, toml.as_bytes())
        .with_context(|| format!("write and sync temp file: {}", tmp_path.display()))?;
    ops.rename_temp_to_target(&tmp_path, path)
        .with_context(|| format!("rename {} -> {}", tmp_path.display(), path.display()))?;
    ops.sync_parent_dir(parent)
        .with_context(|| format!("sync parent dir: {}", parent.display()))?;
    Ok(())
}

trait AtomicWriteOps {
    fn create_parent_dir(&mut self, parent: &Path) -> std::io::Result<()>;
    fn write_tmp_and_sync(&mut self, tmp_path: &Path, data: &[u8]) -> std::io::Result<()>;
    fn rename_temp_to_target(&mut self, tmp_path: &Path, path: &Path) -> std::io::Result<()>;
    fn sync_parent_dir(&mut self, parent: &Path) -> Result<()>;
}

struct FileSystemAtomicWriteOps;

impl AtomicWriteOps for FileSystemAtomicWriteOps {
    fn create_parent_dir(&mut self, parent: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(parent)
    }

    fn write_tmp_and_sync(&mut self, tmp_path: &Path, data: &[u8]) -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(tmp_path)?;
        file.write_all(data)?;
        file.sync_all()?;
        Ok(())
    }

    fn rename_temp_to_target(&mut self, tmp_path: &Path, path: &Path) -> std::io::Result<()> {
        std::fs::rename(tmp_path, path)
    }

    fn sync_parent_dir(&mut self, parent: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            let dir = std::fs::File::open(parent)
                .with_context(|| format!("open dir for sync: {}", parent.display()))?;
            dir.sync_all()
                .with_context(|| format!("sync dir metadata: {}", parent.display()))?;
        }

        #[cfg(not(unix))]
        {
            let _ = parent;
        }

        Ok(())
    }
}

fn trigger_trusttunnel_reload(reload_mode: &str, pid: i32) -> Result<()> {
    match reload_mode.to_lowercase().as_str() {
        "signal" => send_reload_signal(pid),
        "none" => Ok(()),
        _ => anyhow::bail!("unsupported TRUSTTUNNEL_RELOAD_MODE: {reload_mode}"),
    }
}

fn send_reload_signal(pid: i32) -> Result<()> {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(pid, libc::SIGHUP) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Ok(())
    }
}

async fn post_sync_report(
    cfg: &Config,
    client: &reqwest::Client,
    outcome: SyncOutcome,
    timeout: Duration,
) -> Result<()> {
    let url = format!(
        "{}/internal/vpn/classic/sync-report",
        cfg.lk_internal_base_url
    );

    let report = SyncReport {
        node_id: cfg.node_id.clone(),
        version: outcome.version,
        applied_count: outcome.applied_count,
        status: outcome.status,
        error: outcome.error,
        timestamp: Utc::now().to_rfc3339(),
    };

    let resp = client
        .post(url)
        .bearer_auth(&cfg.internal_agent_token)
        .json(&report)
        .timeout(timeout)
        .send()
        .await
        .map_err(|err| map_lk_request_error(err, "sync report"))?;

    if !resp.status().is_success() {
        return Err(lk_http_status_error("sync report", resp.status()));
    }

    Ok(())
}

async fn push_metrics(
    cfg: &Config,
    client: &reqwest::Client,
    state: SharedAgentState,
) -> Result<()> {
    push_metrics_with_retry(
        cfg,
        client,
        state,
        HEARTBEAT_MAX_RETRIES,
        Duration::from_secs(HEARTBEAT_INITIAL_BACKOFF_SECONDS),
    )
    .await
}

async fn push_metrics_with_retry(
    cfg: &Config,
    client: &reqwest::Client,
    state: SharedAgentState,
    max_retries: usize,
    initial_backoff: Duration,
) -> Result<()> {
    for (attempt, backoff) in
        retry_backoff_sequence(max_retries, initial_backoff, Duration::from_secs(30))
            .into_iter()
            .enumerate()
    {
        match push_metrics_once(cfg, client, state.clone()).await {
            Ok(()) => return Ok(()),
            Err(err) => {
                warn!(
                    "heartbeat push failed (attempt {}/{}): {}; retrying in {:?}",
                    attempt + 1,
                    max_retries + 1,
                    sanitize_error_message(&format!("{err:#}")),
                    backoff,
                );
                tokio::time::sleep(backoff).await;
            }
        }
    }

    push_metrics_once(cfg, client, state).await
}

async fn push_metrics_once(
    cfg: &Config,
    client: &reqwest::Client,
    state: SharedAgentState,
) -> Result<()> {
    let trusttunnel_tcp_reachable = check_tcp_reachable(&cfg.trusttunnel_tcp_addr).await;
    let lk_reachable = check_lk_reachable(&cfg.lk_internal_base_url).await;

    let mem_percent = read_memory_usage_percent().unwrap_or(0.0);
    let cpu_percent = read_cpu_usage_percent().unwrap_or(0.0);

    let (current_rx, current_tx) = read_network_totals().unwrap_or((0, 0));
    let prev_total = {
        let state = state.lock().await;
        state.last_network_total
    };
    let (rx_mbps, tx_mbps) = if let Some(prev) = prev_total {
        let rx_bytes_delta = current_rx.saturating_sub(prev) as f64;
        let tx_bytes_delta = current_tx.saturating_sub(prev) as f64;
        (
            (rx_bytes_delta * 8.0) / (cfg.heartbeat_interval_seconds as f64 * 1_000_000.0),
            (tx_bytes_delta * 8.0) / (cfg.heartbeat_interval_seconds as f64 * 1_000_000.0),
        )
    } else {
        (0.0, 0.0)
    };
    let new_network_total = current_rx.saturating_add(current_tx);

    let payload = HeartbeatPayload {
        node_id: &cfg.node_id,
        status: "alive",
        metrics_json: HeartbeatMetrics {
            active_connections: if trusttunnel_tcp_reachable { 1 } else { 0 },
            cpu_percent,
            mem_percent,
            rx_mbps,
            tx_mbps,
        },
    };

    let url = format!("{}/internal/nodes/heartbeat", cfg.lk_internal_base_url);
    let resp = client
        .post(url)
        .bearer_auth(&cfg.internal_agent_token)
        .json(&payload)
        .send()
        .await
        .map_err(|err| map_lk_request_error(err, "heartbeat"))?;

    if !resp.status().is_success() {
        let mut state = state.lock().await;
        state.heartbeat_failure_total = state.heartbeat_failure_total.saturating_add(1);
        state.lk_reachable = false;
        state.trusttunnel_tcp_reachable = trusttunnel_tcp_reachable;
        state.last_network_total = Some(new_network_total);
        return Err(lk_http_status_error("heartbeat", resp.status()));
    }

    let mut state = state.lock().await;
    state.heartbeat_success_total = state.heartbeat_success_total.saturating_add(1);
    state.lk_reachable = lk_reachable;
    state.trusttunnel_tcp_reachable = trusttunnel_tcp_reachable;
    state.last_network_total = Some(new_network_total);

    Ok(())
}

fn sanitize_error_message(message: &str) -> String {
    let mut masked = message.replace("INTERNAL_AGENT_TOKEN", "***");

    if let Some(idx) = masked.find("Bearer ") {
        let suffix = &masked[idx + 7..];
        let token_len = suffix
            .find(|c: char| c.is_whitespace() || c == ',' || c == '"')
            .unwrap_or(suffix.len());
        masked.replace_range(idx + 7..idx + 7 + token_len, "***");
    }

    while let Some(start) = masked.find("\"password\":\"") {
        let value_start = start + "\"password\":\"".len();
        if let Some(end_rel) = masked[value_start..].find('"') {
            masked.replace_range(value_start..value_start + end_rel, "***");
        } else {
            break;
        }
    }

    while let Some(start) = masked.find("password=") {
        let value_start = start + "password=".len();
        let value_end = masked[value_start..]
            .find(|c: char| c.is_whitespace() || c == '&' || c == ',')
            .map(|x| value_start + x)
            .unwrap_or(masked.len());
        masked.replace_range(value_start..value_end, "***");
    }

    masked
}

async fn check_lk_reachable(lk_internal_base_url: &str) -> bool {
    let parsed = match reqwest::Url::parse(lk_internal_base_url) {
        Ok(url) => url,
        Err(_) => return false,
    };
    let host = match parsed.host_str() {
        Some(host) => host,
        None => return false,
    };
    let port = parsed.port_or_known_default().unwrap_or(443);
    check_tcp_reachable(&format!("{host}:{port}")).await
}

async fn check_tcp_reachable(addr: &str) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_secs(TCP_REACHABILITY_TIMEOUT_SECONDS),
            TcpStream::connect(addr),
        )
        .await,
        Ok(Ok(_))
    )
}

fn map_lk_request_error(err: reqwest::Error, operation: &str) -> anyhow::Error {
    if err.is_timeout() {
        anyhow::anyhow!(
            "[{LK_TIMEOUT_ERROR_TAG}] {operation} timed out (connect<={}s, total<={}s)",
            LK_CONNECT_TIMEOUT_SECONDS,
            LK_REQUEST_TIMEOUT_SECONDS,
        )
    } else {
        anyhow::anyhow!("[{LK_OTHER_ERROR_TAG}] {operation} request failed: {err}")
    }
}

fn lk_http_status_error(operation: &str, status: StatusCode) -> anyhow::Error {
    anyhow::anyhow!("[{LK_OTHER_ERROR_TAG}] {operation} unexpected response status: {status}")
}

fn classify_lk_error(err: &anyhow::Error) -> Option<LkErrorKind> {
    let rendered = format!("{err:#}");
    if rendered.contains(&format!("[{LK_TIMEOUT_ERROR_TAG}]")) {
        Some(LkErrorKind::Timeout)
    } else if rendered.contains(&format!("[{LK_OTHER_ERROR_TAG}]")) {
        Some(LkErrorKind::Other)
    } else {
        None
    }
}

fn increment_lk_error_counter(state: &mut AgentState, err: &anyhow::Error) {
    match classify_lk_error(err) {
        Some(LkErrorKind::Timeout) => {
            state.lk_timeout_count = state.lk_timeout_count.saturating_add(1);
        }
        Some(LkErrorKind::Other) => {
            state.lk_error_count = state.lk_error_count.saturating_add(1);
        }
        None => {}
    }
}

fn read_memory_usage_percent() -> Result<f64> {
    let text = std::fs::read_to_string("/proc/meminfo")?;
    let mut values = HashMap::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            values.insert(
                key.trim_end_matches(':'),
                value.parse::<f64>().unwrap_or(0.0),
            );
        }
    }

    let total = *values.get("MemTotal").unwrap_or(&0.0);
    let available = *values.get("MemAvailable").unwrap_or(&0.0);
    if total <= 0.0 {
        return Ok(0.0);
    }
    Ok(((total - available) / total) * 100.0)
}

fn read_cpu_usage_percent() -> Result<f64> {
    let text = std::fs::read_to_string("/proc/loadavg")?;
    let one_min = text
        .split_whitespace()
        .next()
        .unwrap_or("0")
        .parse::<f64>()
        .unwrap_or(0.0);
    let cpus = std::thread::available_parallelism()
        .map(|x| x.get())
        .unwrap_or(1) as f64;
    Ok((one_min / cpus * 100.0).min(100.0))
}

fn read_network_totals() -> Result<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/net/dev")?;
    let mut total_rx = 0u64;
    let mut total_tx = 0u64;

    for line in text.lines().skip(2) {
        let line = line.trim();
        if line.starts_with("lo:") || line.is_empty() {
            continue;
        }
        let mut parts = line.split(':');
        let _iface = parts.next();
        let data = parts.next().unwrap_or("");
        let fields: Vec<&str> = data.split_whitespace().collect();
        if fields.len() >= 16 {
            let rx = fields[0].parse::<u64>().unwrap_or(0);
            let tx = fields[8].parse::<u64>().unwrap_or(0);
            total_rx = total_rx.saturating_add(rx);
            total_tx = total_tx.saturating_add(tx);
        }
    }

    Ok((total_rx, total_tx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use std::io;

    fn test_config(base_url: String, credentials_path: PathBuf) -> Config {
        Config {
            lk_internal_base_url: base_url,
            internal_agent_token: "token".to_string(),
            node_id: "node-1".to_string(),
            sync_interval_seconds: 60,
            heartbeat_interval_seconds: 10,
            credentials_path,
            trusttunnel_reload_mode: "signal".to_string(),
            trusttunnel_pid: 1,
            agent_port: 9105,
            trusttunnel_tcp_addr: "127.0.0.1:8443".to_string(),
        }
    }

    fn checksum_for(accounts: Vec<Account>) -> String {
        canonical_checksum(&accounts)
    }

    fn legacy_toml_checksum_for(accounts: Vec<Account>) -> String {
        let raw = toml::to_string(&CredentialsFile {
            client: accounts
                .into_iter()
                .map(|account| AccountCredential {
                    username: account.username,
                    password: account.password,
                })
                .collect(),
        })
        .expect("toml serialization");
        format!("{:x}", Sha256::digest(raw.as_bytes()))
    }

    #[tokio::test]
    async fn sends_failed_report_when_snapshot_fetch_fails() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-test-fetch-fail.toml");
        let cfg = test_config(server.base_url(), credentials_path);
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("build client");
        let state = Arc::new(Mutex::new(AgentState::default()));

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/internal/vpn/classic/accounts")
                    .query_param("node_id", "node-1");
                then.status(500);
            })
            .await;

        let _report = server
            .mock_async(|when, then| {
                when.method(POST);
                then.status(200);
            })
            .await;

        let result =
            sync_once_with_retry(&cfg, &client, state.clone(), 0, Duration::from_millis(1)).await;

        assert!(result.is_err(), "expected error");
    }

    #[tokio::test]
    async fn sends_failed_report_when_checksum_mismatch() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-test-checksum-fail.toml");
        let cfg = test_config(server.base_url(), credentials_path);
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("build client");
        let state = Arc::new(Mutex::new(AgentState::default()));

        let accounts = vec![Account {
            username: "alice".to_string(),
            password: "secret".to_string(),
            enabled: true,
        }];

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/internal/vpn/classic/accounts")
                    .query_param("node_id", "node-1");
                then.status(200).json_body(serde_json::json!({
                    "version": "v1",
                    "accounts": accounts,
                    "checksum": "deadbeef"
                }));
            })
            .await;

        let _report = server
            .mock_async(|when, then| {
                when.method(POST);
                then.status(200);
            })
            .await;

        let result =
            sync_once_with_retry(&cfg, &client, state.clone(), 0, Duration::from_millis(1)).await;

        assert!(result.is_err(), "expected error");
    }

    #[test]
    fn retry_backoff_sequence_covers_boundaries() {
        assert!(
            retry_backoff_sequence(0, Duration::from_secs(1), Duration::from_secs(30)).is_empty()
        );
        assert_eq!(
            retry_backoff_sequence(1, Duration::from_secs(1), Duration::from_secs(30)),
            vec![Duration::from_secs(1)]
        );
        assert_eq!(
            retry_backoff_sequence(3, Duration::from_secs(1), Duration::from_secs(30)),
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4)
            ]
        );
    }

    #[tokio::test]
    async fn lk_timeout_error_is_mapped() {
        let server = MockServer::start_async().await;
        let cfg = test_config(
            server.base_url(),
            std::env::temp_dir().join("trusttunnel-timeout.toml"),
        );
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(50))
            .timeout(Duration::from_millis(100))
            .no_proxy()
            .build()
            .expect("build client");

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/internal/vpn/classic/accounts")
                    .query_param("node_id", "node-1");
                then.status(200)
                    .delay(Duration::from_millis(300))
                    .json_body(serde_json::json!({
                        "version": "v1",
                        "accounts": [],
                        "checksum": "x"
                    }));
            })
            .await;

        let err = fetch_snapshot(&cfg, &client, Duration::from_millis(100))
            .await
            .err()
            .expect("timeout expected");
        assert!(err.to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn lk_network_error_connection_refused() {
        let cfg = test_config(
            "http://127.0.0.1:1".to_string(),
            std::env::temp_dir().join("trusttunnel-neterr.toml"),
        );
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(100))
            .timeout(Duration::from_millis(200))
            .no_proxy()
            .build()
            .expect("build client");

        let err = fetch_snapshot(&cfg, &client, Duration::from_millis(200))
            .await
            .err()
            .expect("network error expected");
        assert!(!err.to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn lk_network_error_unresolvable_host() {
        let cfg = test_config(
            "http://no-such-host.invalid".to_string(),
            std::env::temp_dir().join("trusttunnel-neterr-host.toml"),
        );
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(100))
            .timeout(Duration::from_millis(200))
            .no_proxy()
            .build()
            .expect("build client");

        let err = fetch_snapshot(&cfg, &client, Duration::from_millis(200))
            .await
            .err()
            .expect("dns error expected");
        assert!(!err.to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn fetch_retry_backoff_sequence_and_attempt_count() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let server = MockServer::start_async().await;
        let cfg = test_config(
            server.base_url(),
            std::env::temp_dir().join("trusttunnel-backoff.toml"),
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("build client");

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/internal/vpn/classic/accounts")
                    .query_param("node_id", "node-1");
                then.status(500);
            })
            .await;

        let calls = Arc::new(AtomicUsize::new(0));
        let sleeps = Arc::new(tokio::sync::Mutex::new(Vec::new()));

        let calls_clone = calls.clone();
        let sleeps_clone = sleeps.clone();
        let result = fetch_snapshot_with_retry_and_sleep(
            &cfg,
            &client,
            3,
            Duration::from_secs(1),
            Instant::now() + Duration::from_secs(30),
            Duration::from_secs(LK_REQUEST_TIMEOUT_SECONDS),
            move |duration| {
                let calls = calls_clone.clone();
                let sleeps = sleeps_clone.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    sleeps.lock().await.push(duration);
                }
            },
        )
        .await;

        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            *sleeps.lock().await,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4)
            ]
        );
        assert_eq!(_snapshot.hits_async().await, 4);
    }

    #[tokio::test]
    async fn sync_cycle_timeout_errors_respect_total_budget() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-sync-budget-timeout.toml");
        let cfg = test_config(server.base_url(), credentials_path);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .build()
            .expect("build client");
        let state = Arc::new(Mutex::new(AgentState::default()));

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/internal/vpn/classic/accounts")
                    .query_param("node_id", "node-1");
                then.status(200)
                    .delay(Duration::from_secs(1))
                    .json_body(serde_json::json!({
                        "version": "v1",
                        "accounts": [],
                        "checksum": "x"
                    }));
            })
            .await;

        let _report = server
            .mock_async(|when, then| {
                when.method(POST).path("/internal/vpn/classic/sync-report");
                then.status(200).delay(Duration::from_secs(1));
            })
            .await;

        let started = Instant::now();
        let result = sync_once_with_retry_budgeted(
            &cfg,
            &client,
            state,
            10,
            Duration::from_millis(50),
            Duration::from_millis(300),
            Duration::from_millis(80),
            Duration::from_millis(60),
        )
        .await;
        let elapsed = started.elapsed();

        assert!(result.is_err());
        assert!(
            elapsed <= Duration::from_millis(450),
            "elapsed: {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn sync_cycle_http_errors_respect_total_budget() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-sync-budget-errors.toml");
        let cfg = test_config(server.base_url(), credentials_path);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .build()
            .expect("build client");
        let state = Arc::new(Mutex::new(AgentState::default()));

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/internal/vpn/classic/accounts")
                    .query_param("node_id", "node-1");
                then.status(500);
            })
            .await;

        let _report = server
            .mock_async(|when, then| {
                when.method(POST).path("/internal/vpn/classic/sync-report");
                then.status(500).delay(Duration::from_secs(1));
            })
            .await;

        let started = Instant::now();
        let result = sync_once_with_retry_budgeted(
            &cfg,
            &client,
            state,
            10,
            Duration::from_millis(50),
            Duration::from_millis(300),
            Duration::from_millis(80),
            Duration::from_millis(60),
        )
        .await;
        let elapsed = started.elapsed();

        assert!(result.is_err());
        assert!(
            elapsed <= Duration::from_millis(450),
            "elapsed: {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn sync_timeout_increments_lk_timeout_count() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-sync-timeout-count.toml");
        let cfg = test_config(server.base_url(), credentials_path);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .build()
            .expect("build client");
        let state = Arc::new(Mutex::new(AgentState::default()));

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/internal/vpn/classic/accounts")
                    .query_param("node_id", "node-1");
                then.status(200)
                    .delay(Duration::from_millis(200))
                    .json_body(serde_json::json!({
                        "version": "v1",
                        "accounts": [],
                        "checksum": "x"
                    }));
            })
            .await;

        let _report = server
            .mock_async(|when, then| {
                when.method(POST).path("/internal/vpn/classic/sync-report");
                then.status(200);
            })
            .await;

        let result = sync_once_with_retry_budgeted(
            &cfg,
            &client,
            state.clone(),
            0,
            Duration::from_millis(1),
            Duration::from_millis(350),
            Duration::from_millis(50),
            Duration::from_millis(50),
        )
        .await;

        assert!(result.is_err());
        let state = state.lock().await;
        assert_eq!(state.lk_timeout_count, 1);
        assert_eq!(state.lk_error_count, 0);
    }

    #[tokio::test]
    async fn sync_http_error_increments_lk_error_count() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-sync-http-error-count.toml");
        let cfg = test_config(server.base_url(), credentials_path);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .build()
            .expect("build client");
        let state = Arc::new(Mutex::new(AgentState::default()));

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/internal/vpn/classic/accounts")
                    .query_param("node_id", "node-1");
                then.status(500);
            })
            .await;

        let _report = server
            .mock_async(|when, then| {
                when.method(POST).path("/internal/vpn/classic/sync-report");
                then.status(200);
            })
            .await;

        let result = sync_once_with_retry_budgeted(
            &cfg,
            &client,
            state.clone(),
            0,
            Duration::from_millis(1),
            Duration::from_millis(350),
            Duration::from_millis(50),
            Duration::from_millis(50),
        )
        .await;

        assert!(result.is_err());
        let state = state.lock().await;
        assert_eq!(state.lk_timeout_count, 0);
        assert_eq!(state.lk_error_count, 1);
        assert_eq!(state.sync_duration.count, 1);
        assert!(state.sync_duration.last_seconds > 0.0);
    }

    #[test]
    fn sync_cycle_worst_case_time_is_capped_at_30s() {
        let worst_case = sync_cycle_worst_case_duration(
            SYNC_MAX_RETRIES,
            Duration::from_secs(SYNC_INITIAL_BACKOFF_SECONDS),
            Duration::from_secs(LK_REQUEST_TIMEOUT_SECONDS),
            Duration::from_secs(SYNC_REPORT_TIMEOUT_SECONDS),
        );

        assert!(
            worst_case <= Duration::from_secs(30),
            "worst-case: {:?}",
            worst_case
        );
        assert_eq!(worst_case, Duration::from_secs(30));
    }

    #[test]
    fn checksum_rejects_legacy_toml_algorithm() {
        let snapshot = SnapshotResponse {
            version: "v-legacy".to_string(),
            accounts: vec![Account {
                username: "alice".to_string(),
                password: "secret".to_string(),
                enabled: true,
            }],
            checksum: legacy_toml_checksum_for(vec![Account {
                username: "alice".to_string(),
                password: "secret".to_string(),
                enabled: true,
            }]),
        };

        assert!(validate_checksum(&snapshot).is_err());
    }

    #[test]
    fn checksum_accepts_documented_canonical_example() {
        let accounts = vec![
            Account {
                username: "bob".to_string(),
                password: "pw2".to_string(),
                enabled: true,
            },
            Account {
                username: "alice".to_string(),
                password: "pw1".to_string(),
                enabled: true,
            },
        ];

        let snapshot = SnapshotResponse {
            version: "v-doc-example".to_string(),
            checksum: checksum_for(accounts.clone()),
            accounts,
        };

        assert!(validate_checksum(&snapshot).is_ok());
    }

    #[derive(Default)]
    struct RecordingAtomicWriteOps {
        steps: Vec<&'static str>,
    }

    impl AtomicWriteOps for RecordingAtomicWriteOps {
        fn create_parent_dir(&mut self, _parent: &Path) -> io::Result<()> {
            self.steps.push("create_parent_dir");
            Ok(())
        }

        fn write_tmp_and_sync(&mut self, _tmp_path: &Path, _data: &[u8]) -> io::Result<()> {
            self.steps.push("write_tmp_and_sync");
            Ok(())
        }

        fn rename_temp_to_target(&mut self, _tmp_path: &Path, _path: &Path) -> io::Result<()> {
            self.steps.push("rename_temp_to_target");
            Ok(())
        }

        fn sync_parent_dir(&mut self, _parent: &Path) -> Result<()> {
            self.steps.push("sync_parent_dir");
            Ok(())
        }
    }

    #[test]
    fn write_credentials_includes_only_enabled_accounts() {
        let dir = std::env::temp_dir();
        let path = dir.join("trusttunnel-enabled-filter-test.toml");
        let _ = std::fs::remove_file(&path);

        let accounts = vec![
            Account {
                username: "enabled-user".to_string(),
                password: "enabled-pass".to_string(),
                enabled: true,
            },
            Account {
                username: "disabled-user".to_string(),
                password: "disabled-pass".to_string(),
                enabled: false,
            },
        ];

        write_credentials_atomically(&path, &accounts).expect("write credentials");
        let content = std::fs::read_to_string(&path).expect("read credentials file");

        assert!(content.contains("enabled-user"));
        assert!(!content.contains("disabled-user"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn write_credentials_follows_expected_operation_order() {
        let mut ops = RecordingAtomicWriteOps::default();
        let path = Path::new("/tmp/trusttunnel/credentials.toml");
        let accounts = vec![Account {
            username: "user".to_string(),
            password: "pass".to_string(),
            enabled: true,
        }];

        write_credentials_atomically_with_ops(path, &accounts, &mut ops)
            .expect("atomic write should succeed");

        assert_eq!(
            ops.steps,
            vec![
                "create_parent_dir",
                "write_tmp_and_sync",
                "rename_temp_to_target",
                "sync_parent_dir"
            ]
        );
    }
}
