use anyhow::{Context, Result};
use chrono::Utc;
use log::{error, info, warn};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::net::TcpStream;

#[derive(Clone)]
struct Config {
    lk_internal_base_url: String,
    internal_agent_token: String,
    node_id: String,
    sync_interval_seconds: u64,
    credentials_path: PathBuf,
    trusttunnel_reload_signal: String,
    trusttunnel_health_addr: String,
    health_check_interval_seconds: u64,
    metrics_push_interval: u64,
}

impl Config {
    fn from_env() -> Result<Self> {
        Ok(Self {
            lk_internal_base_url: std::env::var("LK_INTERNAL_BASE_URL")?,
            internal_agent_token: std::env::var("INTERNAL_AGENT_TOKEN")?,
            node_id: std::env::var("NODE_ID")?,
            sync_interval_seconds: std::env::var("SYNC_INTERVAL_SECONDS")
                .unwrap_or_else(|_| "60".to_string())
                .parse()?,
            credentials_path: PathBuf::from(
                std::env::var("CREDENTIALS_PATH")
                    .unwrap_or_else(|_| "/shared/credentials.toml".to_string()),
            ),
            trusttunnel_reload_signal: std::env::var("TRUSTTUNNEL_RELOAD_SIGNAL")
                .unwrap_or_else(|_| "SIGHUP".to_string()),
            trusttunnel_health_addr: std::env::var("TRUSTTUNNEL_HEALTH_ADDR")
                .unwrap_or_else(|_| "localhost:443".to_string()),
            health_check_interval_seconds: std::env::var("HEALTH_CHECK_INTERVAL_SECONDS")
                .unwrap_or_else(|_| "15".to_string())
                .parse()?,
            metrics_push_interval: std::env::var("METRICS_PUSH_INTERVAL")
                .unwrap_or_else(|_| "30".to_string())
                .parse()?,
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
struct AgentState {
    last_version: Option<String>,
    last_checksum: Option<String>,
    last_network_total: Option<u64>,
    last_sync_failed: bool,
    last_health_ok: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let cfg = Config::from_env()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;

    let mut state = AgentState::default();
    let mut sync_interval = tokio::time::interval(Duration::from_secs(cfg.sync_interval_seconds));
    let mut metrics_interval =
        tokio::time::interval(Duration::from_secs(cfg.metrics_push_interval.max(10)));
    let mut health_check_interval = tokio::time::interval(Duration::from_secs(
        cfg.health_check_interval_seconds.max(1),
    ));

    loop {
        tokio::select! {
            _ = sync_interval.tick() => {
                if let Err(e) = sync_once(&cfg, &client, &mut state).await {
                    state.last_sync_failed = true;
                    error!("credentials sync failed: {e:#}");
                } else {
                    state.last_sync_failed = false;
                }
            }
            _ = metrics_interval.tick() => {
                if let Err(e) = push_metrics(&cfg, &client, &mut state).await {
                    warn!("metrics push failed: {e:#}");
                }
            }
            _ = health_check_interval.tick() => {
                state.last_health_ok = check_health(&cfg.trusttunnel_health_addr).await;
            }
            _ = tokio::signal::ctrl_c() => {
                info!("sidecar agent interrupted");
                return Ok(());
            }
        }
    }
}

async fn check_health(health_addr: &str) -> bool {
    TcpStream::connect(health_addr).await.is_ok()
}

async fn sync_once(cfg: &Config, client: &reqwest::Client, state: &mut AgentState) -> Result<()> {
    sync_once_with_retry(cfg, client, state, 5, Duration::from_secs(1)).await
}

async fn sync_once_with_retry(
    cfg: &Config,
    client: &reqwest::Client,
    state: &mut AgentState,
    max_retries: usize,
    initial_backoff: Duration,
) -> Result<()> {
    let (outcome, sync_result) =
        match fetch_snapshot_with_retry(cfg, client, max_retries, initial_backoff).await {
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

                let is_new = state.last_version.as_deref() != Some(&snapshot.version)
                    || state.last_checksum.as_deref() != Some(&snapshot.checksum);

                let sync_result = (|| -> Result<()> {
                    if is_new {
                        validate_checksum(&snapshot).context("validate_checksum")?;
                        write_credentials_atomically(&cfg.credentials_path, &snapshot.accounts)
                            .context("write_credentials_atomically")?;
                        send_reload_signal(&cfg.trusttunnel_reload_signal)
                            .context("send_reload_signal")?;
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
                    state.last_version = Some(snapshot.version);
                    state.last_checksum = Some(snapshot.checksum);
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

    post_sync_report(cfg, client, outcome).await?;
    sync_result
}

async fn fetch_snapshot_with_retry(
    cfg: &Config,
    client: &reqwest::Client,
    max_retries: usize,
    initial_backoff: Duration,
) -> Result<SnapshotResponse> {
    let mut backoff = initial_backoff;
    for _ in 0..max_retries {
        match fetch_snapshot(cfg, client).await {
            Ok(snapshot) => return Ok(snapshot),
            Err(e) => {
                warn!("snapshot fetch failed: {e:#}; retrying in {:?}", backoff);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }

    fetch_snapshot(cfg, client).await
}

async fn fetch_snapshot(cfg: &Config, client: &reqwest::Client) -> Result<SnapshotResponse> {
    let url = format!("{}/internal/vpn/classic/accounts", cfg.lk_internal_base_url);

    let resp = client
        .get(url)
        .query(&[("node_id", &cfg.node_id)])
        .bearer_auth(&cfg.internal_agent_token)
        .send()
        .await?;

    if resp.status() != StatusCode::OK {
        anyhow::bail!("unexpected snapshot response status: {}", resp.status());
    }

    Ok(resp.json().await?)
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

fn send_reload_signal(signal_name: &str) -> Result<()> {
    #[cfg(unix)]
    {
        let signal = match signal_name.to_uppercase().as_str() {
            "SIGHUP" => libc::SIGHUP,
            "SIGUSR1" => libc::SIGUSR1,
            "SIGUSR2" => libc::SIGUSR2,
            _ => anyhow::bail!("unsupported reload signal: {signal_name}"),
        };

        let pid: i32 = std::env::var("TRUSTTUNNEL_PID")
            .ok()
            .and_then(|x| x.parse().ok())
            .unwrap_or(1);
        let rc = unsafe { libc::kill(pid, signal) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = signal_name;
        Ok(())
    }
}

async fn post_sync_report(
    cfg: &Config,
    client: &reqwest::Client,
    outcome: SyncOutcome,
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
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("sync report rejected: {}", resp.status());
    }

    Ok(())
}

async fn push_metrics(
    cfg: &Config,
    client: &reqwest::Client,
    state: &mut AgentState,
) -> Result<()> {
    let mem_percent = read_memory_usage_percent().unwrap_or(0.0);
    let cpu_percent = read_cpu_usage_percent().unwrap_or(0.0);

    let (current_rx, current_tx) = read_network_totals().unwrap_or((0, 0));
    let (rx_mbps, tx_mbps) = if let Some(prev) = state.last_network_total {
        let rx_bytes_delta = current_rx.saturating_sub(prev) as f64;
        let tx_bytes_delta = current_tx.saturating_sub(prev) as f64;
        (
            (rx_bytes_delta * 8.0) / (cfg.metrics_push_interval as f64 * 1_000_000.0),
            (tx_bytes_delta * 8.0) / (cfg.metrics_push_interval as f64 * 1_000_000.0),
        )
    } else {
        (0.0, 0.0)
    };
    state.last_network_total = Some(current_rx.saturating_add(current_tx));

    let status = if state.last_sync_failed || !state.last_health_ok {
        "degraded"
    } else {
        "ok"
    };

    let payload = HeartbeatPayload {
        node_id: &cfg.node_id,
        status,
        metrics_json: HeartbeatMetrics {
            active_connections: if state.last_health_ok { 1 } else { 0 },
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
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("metrics push rejected: {}", resp.status());
    }

    Ok(())
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
            credentials_path,
            trusttunnel_reload_signal: "SIGHUP".to_string(),
            trusttunnel_health_addr: "localhost:443".to_string(),
            health_check_interval_seconds: 15,
            metrics_push_interval: 30,
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
        let client = reqwest::Client::new();
        let mut state = AgentState::default();

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(GET).path("/internal/vpn/classic/accounts");
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
            sync_once_with_retry(&cfg, &client, &mut state, 0, Duration::from_millis(1)).await;

        assert!(result.is_err(), "expected error");
    }

    #[tokio::test]
    async fn sends_failed_report_when_checksum_mismatch() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-test-checksum-fail.toml");
        let cfg = test_config(server.base_url(), credentials_path);
        let client = reqwest::Client::new();
        let mut state = AgentState::default();

        let accounts = vec![Account {
            username: "alice".to_string(),
            password: "secret".to_string(),
            enabled: true,
        }];

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(GET).path("/internal/vpn/classic/accounts");
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
            sync_once_with_retry(&cfg, &client, &mut state, 0, Duration::from_millis(1)).await;

        assert!(result.is_err(), "expected error");
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
