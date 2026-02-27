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
            metrics_push_interval: std::env::var("METRICS_PUSH_INTERVAL")
                .unwrap_or_else(|_| "30".to_string())
                .parse()?,
        })
    }
}

#[derive(Deserialize)]
struct SnapshotResponse {
    version: String,
    credentials: Vec<Credential>,
    checksum: String,
}

#[derive(Clone, Deserialize, Serialize)]
struct Credential {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct SyncReport<'a> {
    version: &'a str,
    status: &'a str,
    applied_count: usize,
    checksum: &'a str,
    error: Option<String>,
    collected_at: String,
}

#[derive(Serialize)]
struct MetricsPayload<'a> {
    node_id: &'a str,
    active_connections: u64,
    cpu_usage_percent: f64,
    memory_usage_percent: f64,
    bandwidth_mbps: f64,
    error_rate: f64,
    collected_at: String,
}

#[derive(Serialize)]
struct CredentialsFile {
    client: Vec<Credential>,
}

#[derive(Default)]
struct AgentState {
    last_version: Option<String>,
    last_checksum: Option<String>,
    last_network_total: Option<u64>,
    last_sync_failed: bool,
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
            _ = tokio::signal::ctrl_c() => {
                info!("sidecar agent interrupted");
                return Ok(());
            }
        }
    }
}

async fn sync_once(cfg: &Config, client: &reqwest::Client, state: &mut AgentState) -> Result<()> {
    let snapshot = fetch_snapshot_with_retry(cfg, client).await?;
    let is_new = state.last_version.as_deref() != Some(&snapshot.version)
        || state.last_checksum.as_deref() != Some(&snapshot.checksum);

    if is_new {
        validate_checksum(&snapshot)?;
        write_credentials_atomically(&cfg.credentials_path, &snapshot.credentials)?;
        send_reload_signal(&cfg.trusttunnel_reload_signal)?;
        info!("applied credentials snapshot version={}", snapshot.version);
    }

    post_sync_report(
        cfg,
        client,
        SyncReport {
            version: &snapshot.version,
            status: "success",
            applied_count: snapshot.credentials.len(),
            checksum: &snapshot.checksum,
            error: None,
            collected_at: Utc::now().to_rfc3339(),
        },
    )
    .await?;

    state.last_version = Some(snapshot.version);
    state.last_checksum = Some(snapshot.checksum);
    Ok(())
}

async fn fetch_snapshot_with_retry(
    cfg: &Config,
    client: &reqwest::Client,
) -> Result<SnapshotResponse> {
    let mut backoff = Duration::from_secs(1);
    for _ in 0..5 {
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
    let url = format!(
        "{}/internal/trusttunnel/nodes/{}/credentials-snapshot",
        cfg.lk_internal_base_url, cfg.node_id
    );

    let resp = client
        .get(url)
        .bearer_auth(&cfg.internal_agent_token)
        .send()
        .await?;

    if resp.status() != StatusCode::OK {
        anyhow::bail!("unexpected snapshot response status: {}", resp.status());
    }

    Ok(resp.json().await?)
}

fn validate_checksum(snapshot: &SnapshotResponse) -> Result<()> {
    let raw = toml::to_string(&CredentialsFile {
        client: snapshot.credentials.clone(),
    })?;
    let checksum = format!("{:x}", Sha256::digest(raw.as_bytes()));
    if checksum != snapshot.checksum {
        anyhow::bail!("snapshot checksum mismatch");
    }
    Ok(())
}

fn write_credentials_atomically(path: &Path, credentials: &[Credential]) -> Result<()> {
    let parent = path.parent().context("credentials path without parent")?;
    std::fs::create_dir_all(parent)?;

    let tmp_path = parent.join(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));

    let toml = toml::to_string(&CredentialsFile {
        client: credentials.to_vec(),
    })?;

    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp_path)?;
    file.write_all(toml.as_bytes())?;
    file.sync_all()?;
    drop(file);

    std::fs::rename(&tmp_path, path)?;
    Ok(())
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
    report: SyncReport<'_>,
) -> Result<()> {
    let url = format!(
        "{}/internal/trusttunnel/nodes/{}/sync-report",
        cfg.lk_internal_base_url, cfg.node_id
    );

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
    let health_ok = TcpStream::connect(&cfg.trusttunnel_health_addr)
        .await
        .is_ok();
    let memory_usage_percent = read_memory_usage_percent().unwrap_or(0.0);
    let cpu_usage_percent = read_cpu_usage_percent().unwrap_or(0.0);

    let current_net = read_network_totals().unwrap_or(0);
    let bandwidth_mbps = if let Some(prev) = state.last_network_total {
        let bytes_delta = current_net.saturating_sub(prev) as f64;
        (bytes_delta * 8.0) / (cfg.metrics_push_interval as f64 * 1_000_000.0)
    } else {
        0.0
    };
    state.last_network_total = Some(current_net);

    let payload = MetricsPayload {
        node_id: &cfg.node_id,
        active_connections: if health_ok { 1 } else { 0 },
        cpu_usage_percent,
        memory_usage_percent,
        bandwidth_mbps,
        error_rate: if state.last_sync_failed { 1.0 } else { 0.0 },
        collected_at: Utc::now().to_rfc3339(),
    };

    let url = format!("{}/internal/trusttunnel/metrics", cfg.lk_internal_base_url);
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

fn read_network_totals() -> Result<u64> {
    let text = std::fs::read_to_string("/proc/net/dev")?;
    let mut total = 0u64;

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
            total = total.saturating_add(rx).saturating_add(tx);
        }
    }

    Ok(total)
}
