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
#[cfg(target_os = "linux")]
use std::{
    os::fd::RawFd,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::net::TcpStream;

#[derive(Clone)]
struct Config {
    lk_internal_base_url: String,
    lk_api_base_url: String,
    internal_agent_token: String,
    node_id: String,
    sync_interval_seconds: u64,
    credentials_path: PathBuf,
    trusttunnel_reload_signal: String,
    trusttunnel_health_addr: String,
    health_check_interval_seconds: u64,
    metrics_push_interval: u64,
    cluster_id: String,
    public_host: String,
    endpoint_ip: String,
    desired_pool_size: u32,
    weight: u32,
    stage: String,
    is_enabled: bool,
    rollout_group: Option<String>,
    cluster: Option<String>,
    namespace: Option<String>,
    register_pod_name: Option<String>,
    pod_name: String,
    pod_namespace: String,
    node_name: String,
    pod_uid: String,
    pod_ip: String,
    configs_root: PathBuf,
    secrets_root: PathBuf,
    sync_secret_keys: Vec<String>,
    checklist_path: PathBuf,
    artifacts_root: PathBuf,
    legacy_credentials_flow_enabled: bool,
}

impl Config {
    fn from_env() -> Result<Self> {
        fn required_env(name: &str) -> Result<String> {
            let value =
                std::env::var(name).with_context(|| format!("missing required env {name}"))?;
            let trimmed = value.trim();
            if trimmed.is_empty() {
                anyhow::bail!("required env {name} must not be empty");
            }
            Ok(trimmed.to_string())
        }

        fn optional_env(name: &str) -> Option<String> {
            std::env::var(name)
                .ok()
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
        }

        Ok(Self {
            lk_internal_base_url: required_env("LK_INTERNAL_BASE_URL")?,
            lk_api_base_url: std::env::var("LK_API_BASE_URL")
                .or_else(|_| std::env::var("LK_INTERNAL_BASE_URL"))?,
            internal_agent_token: required_env("INTERNAL_AGENT_TOKEN")?,
            node_id: required_env("NODE_ID")?,
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
            cluster_id: required_env("CLUSTER_ID")?,
            public_host: required_env("PUBLIC_HOST")?,
            endpoint_ip: required_env("ENDPOINT_IP")?,
            desired_pool_size: required_env("DESIRED_POOL_SIZE")?.parse()?,
            weight: required_env("WEIGHT")?.parse()?,
            stage: required_env("STAGE")?,
            is_enabled: required_env("IS_ENABLED")?.parse()?,
            rollout_group: optional_env("ROLLOUT_GROUP"),
            cluster: optional_env("CLUSTER"),
            namespace: optional_env("NAMESPACE"),
            register_pod_name: optional_env("REGISTER_POD_NAME"),
            pod_name: std::env::var("POD_NAME").unwrap_or_else(|_| "unknown".to_string()),
            pod_namespace: std::env::var("POD_NAMESPACE").unwrap_or_else(|_| "default".to_string()),
            node_name: std::env::var("NODE_NAME").unwrap_or_else(|_| "unknown".to_string()),
            pod_uid: std::env::var("POD_UID").unwrap_or_else(|_| "unknown".to_string()),
            pod_ip: std::env::var("POD_IP").unwrap_or_else(|_| "0.0.0.0".to_string()),
            configs_root: PathBuf::from(
                std::env::var("TRUSTTUNNEL_CONFIGS_ROOT")
                    .unwrap_or_else(|_| "/etc/trusttunnel/configs".to_string()),
            ),
            secrets_root: PathBuf::from(
                std::env::var("TRUSTTUNNEL_SECRETS_ROOT")
                    .unwrap_or_else(|_| "/etc/trusttunnel/secrets".to_string()),
            ),
            sync_secret_keys: std::env::var("TRUSTTUNNEL_SYNC_SECRET_KEYS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(ToString::to_string)
                .collect(),
            checklist_path: PathBuf::from(
                std::env::var("SIDECAR_CHECKLIST_PATH")
                    .unwrap_or_else(|_| "/tmp/trusttunnel-sidecar-checklist.json".to_string()),
            ),
            artifacts_root: PathBuf::from(
                std::env::var("SIDECAR_ARTIFACTS_ROOT")
                    .unwrap_or_else(|_| "artifacts/akt".to_string()),
            ),
            legacy_credentials_flow_enabled: std::env::var(
                "SIDECAR_ENABLE_LEGACY_CREDENTIALS_FLOW",
            )
            .ok()
            .map(|x| x.parse())
            .transpose()?
            .unwrap_or_else(|| std::env::var("STAGE").map(|x| x != "prod").unwrap_or(true)),
        })
    }
}

#[derive(Serialize)]
struct RegisterRequest<'a> {
    cluster_id: &'a str,
    public_host: &'a str,
    endpoint_ip: &'a str,
    desired_pool_size: u32,
    weight: u32,
    stage: &'a str,
    is_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    rollout_group: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cluster: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pod_name: Option<&'a str>,
    node_name: &'a str,
    pod_namespace: &'a str,
    pod_uid: &'a str,
    pod_ip: &'a str,
}

#[derive(Deserialize)]
struct RegisterResponse {
    node_id: String,
    node_token: String,
}

#[derive(Serialize)]
struct ReconcileRequest<'a> {
    node_id: &'a str,
    desired_pool_size: u32,
    current_revision: Option<&'a str>,
}

#[derive(Deserialize)]
struct ReconcileResponse {
    revision: String,
    hash: String,
    pool_summary: PoolSummary,
    runtime_payload: RuntimePayload,
    apply_instructions: ApplyInstructions,
}

#[derive(Default, Deserialize)]
struct PoolSummary {
    #[serde(default)]
    desired_pool_size: u32,
    #[serde(default)]
    allocated_pool_size: u32,
    #[serde(default)]
    healthy_pool_size: u32,
}

#[derive(Default, Deserialize)]
struct RuntimePayload {
    #[serde(default)]
    runtime_config: Option<String>,
    #[serde(default)]
    credentials: Vec<Credential>,
}

#[derive(Default, Deserialize)]
struct ApplyInstructions {
    #[serde(default)]
    should_apply: bool,
}

#[derive(Clone, Deserialize, Serialize)]
struct Credential {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct SyncReport {
    revision: String,
    status: String,
    applied_count: usize,
    hash: String,
    error: Option<String>,
    collected_at: String,
}

#[derive(Clone)]
struct SyncOutcome {
    revision: String,
    status: String,
    applied_count: usize,
    hash: String,
    error: Option<String>,
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
struct HeartbeatPayload<'a> {
    status: &'a str,
    last_seen: String,
    health: HeartbeatHealth<'a>,
    clients_count: usize,
    checklist_url: Option<String>,
    akt_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_sync_error: Option<&'a str>,
}

#[derive(Serialize)]
struct HeartbeatHealth<'a> {
    pod_name: &'a str,
    pod_namespace: &'a str,
    node_name: &'a str,
    pod_ip: &'a str,
    sync_failed: bool,
    endpoint_healthy: bool,
}

#[derive(Serialize, Deserialize)]
struct CredentialsFile {
    client: Vec<Credential>,
}

#[derive(Clone)]
struct SyncedFile {
    path: String,
    content: String,
    checksum: String,
}

#[derive(Serialize)]
struct ConfigsPushPayload {
    configs: Vec<ConfigEntry>,
    secrets: Vec<SecretEntry>,
    collected_at: String,
}

#[derive(Serialize)]
struct ConfigEntry {
    path: String,
    content: String,
    checksum: String,
}

#[derive(Serialize)]
struct SecretEntry {
    path: String,
    keys_count: u64,
    checksum: String,
    masked: String,
    value_encrypted: Option<String>,
}

#[derive(Default)]
struct AgentState {
    node_id: Option<String>,
    last_seen_revision: Option<String>,
    last_apply_status: Option<String>,
    last_health_status: Option<String>,
    last_sync_error: Option<String>,
    last_network_total: Option<u64>,
    node_token: Option<String>,
    last_files_checksum: Option<String>,
    checklist_store: Option<ChecklistStore>,
}

struct RuntimeConfigPaths {
    current: PathBuf,
    previous: PathBuf,
    staged: PathBuf,
}

fn runtime_config_paths(current: &Path) -> RuntimeConfigPaths {
    let parent = current.parent().unwrap_or_else(|| Path::new("."));
    let file_name = current
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    RuntimeConfigPaths {
        current: current.to_path_buf(),
        previous: parent.join(format!(".{file_name}.previous")),
        staged: parent.join(format!(".{file_name}.staged")),
    }
}

fn switch_staged_to_current(paths: &RuntimeConfigPaths) -> Result<()> {
    if let Some(parent) = paths.current.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if paths.previous.exists() {
        let _ = std::fs::remove_file(&paths.previous);
    }
    if paths.current.exists() {
        std::fs::rename(&paths.current, &paths.previous).with_context(|| {
            format!(
                "rename {} -> {}",
                paths.current.display(),
                paths.previous.display()
            )
        })?;
    }
    std::fs::rename(&paths.staged, &paths.current).with_context(|| {
        format!(
            "rename {} -> {}",
            paths.staged.display(),
            paths.current.display()
        )
    })?;
    if let Some(parent) = paths.current.parent() {
        let dir = std::fs::File::open(parent)?;
        let _ = dir.sync_all();
    }
    Ok(())
}

fn rollback_to_previous(paths: &RuntimeConfigPaths) -> Result<()> {
    let previous = std::fs::read(&paths.previous)
        .with_context(|| format!("read previous runtime config: {}", paths.previous.display()))?;
    write_runtime_config_atomically(&paths.current, std::str::from_utf8(&previous)?)
        .context("restore current from previous")
}

#[derive(Clone, Serialize, Deserialize)]
struct ChecklistTask {
    id: String,
    title: String,
    status: String,
    done_at: Option<String>,
    name: Option<String>,
    date: Option<String>,
    result: Option<String>,
    details: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct AktReport {
    generated_at: String,
    tasks_completed: Vec<String>,
    summary: String,
    name: String,
    date: String,
    result: String,
    details: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct ChecklistDocument {
    checklist: Vec<ChecklistTask>,
    akt: Option<AktReport>,
}

#[derive(Default, Clone)]
struct ChecklistStore {
    checklist_url: Option<String>,
    akt_url: Option<String>,
}

impl ChecklistStore {
    fn init(cfg: &Config) -> Result<Self> {
        let doc = ChecklistDocument {
            checklist: vec![
                ChecklistTask {
                    id: "register".to_string(),
                    title: "Register node in LK".to_string(),
                    status: "pending".to_string(),
                    done_at: None,
                    name: None,
                    date: None,
                    result: None,
                    details: None,
                },
                ChecklistTask {
                    id: "reconcile".to_string(),
                    title: "Reconcile desired state".to_string(),
                    status: "pending".to_string(),
                    done_at: None,
                    name: None,
                    date: None,
                    result: None,
                    details: None,
                },
                ChecklistTask {
                    id: "apply".to_string(),
                    title: "Apply runtime payload".to_string(),
                    status: "pending".to_string(),
                    done_at: None,
                    name: None,
                    date: None,
                    result: None,
                    details: None,
                },
                ChecklistTask {
                    id: "rollback".to_string(),
                    title: "Rollback to previous runtime payload".to_string(),
                    status: "pending".to_string(),
                    done_at: None,
                    name: None,
                    date: None,
                    result: None,
                    details: None,
                },
            ],
            akt: None,
        };

        write_json_pretty(&cfg.checklist_path, &doc)?;
        let checklist_url = Some(format!("file://{}", cfg.checklist_path.display()));
        let akt_url = Some(write_akt_artifact(cfg, "init", &doc)?);
        Ok(Self {
            checklist_url,
            akt_url,
        })
    }

    fn ensure_urls(&mut self, cfg: &Config) -> Result<()> {
        if self.checklist_url.is_none() {
            self.checklist_url = Some(format!("file://{}", cfg.checklist_path.display()));
        }
        if self.akt_url.is_none() {
            let text = std::fs::read_to_string(&cfg.checklist_path)?;
            let doc: ChecklistDocument = serde_json::from_str(&text)?;
            self.akt_url = Some(write_akt_artifact(cfg, "heartbeat", &doc)?);
        }
        Ok(())
    }

    fn mark_event(
        &mut self,
        cfg: &Config,
        task_id: &str,
        command_id: &str,
        result: &str,
        details: impl Into<String>,
    ) -> Result<()> {
        let text = std::fs::read_to_string(&cfg.checklist_path)?;
        let mut doc: ChecklistDocument = serde_json::from_str(&text)?;
        let date = Utc::now().to_rfc3339();
        let details = details.into();
        for task in &mut doc.checklist {
            if task.id == task_id {
                task.status = match result {
                    "success" | "skipped" => "done".to_string(),
                    "failed" => "failed".to_string(),
                    _ => "in_progress".to_string(),
                };
                task.done_at = Some(date.clone());
                task.name = Some(task_id.to_string());
                task.date = Some(date.clone());
                task.result = Some(result.to_string());
                task.details = Some(details.clone());
            }
        }
        let completed = doc
            .checklist
            .iter()
            .filter(|t| t.status == "done")
            .map(|t| t.id.clone())
            .collect::<Vec<_>>();
        let akt = AktReport {
            generated_at: date.clone(),
            tasks_completed: completed,
            summary: format!("Checklist progress updated after {task_id}"),
            name: task_id.to_string(),
            date,
            result: result.to_string(),
            details,
        };
        doc.akt = Some(akt);
        write_json_pretty(&cfg.checklist_path, &doc)?;
        self.checklist_url = Some(format!("file://{}", cfg.checklist_path.display()));
        self.akt_url = Some(write_akt_artifact(cfg, command_id, &doc)?);
        Ok(())
    }
}

fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}

fn write_akt_artifact(cfg: &Config, command_id: &str, doc: &ChecklistDocument) -> Result<String> {
    std::fs::create_dir_all(&cfg.artifacts_root)?;
    let ts = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let file_name = format!("{command_id}-{}-{ts}.json", cfg.node_id);
    let path = cfg.artifacts_root.join(file_name);
    std::fs::write(&path, serde_json::to_string_pretty(doc)?)?;
    let human_name = path.with_extension("txt");
    let summary = doc
        .checklist
        .iter()
        .map(|t| {
            format!(
                "- {} {}",
                if t.status == "done" {
                    "✅"
                } else if t.status == "failed" {
                    "❌"
                } else {
                    "❌"
                },
                t.title
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(
        &human_name,
        format!("актработа for node {}\n{}\n", cfg.node_id, summary),
    )?;
    Ok(format!("file://{}", path.display()))
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let cfg = Config::from_env()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;

    let mut state = AgentState::default();
    state.checklist_store = Some(ChecklistStore::init(&cfg)?);
    let mut sync_interval = tokio::time::interval(Duration::from_secs(cfg.sync_interval_seconds));
    let mut metrics_interval =
        tokio::time::interval(Duration::from_secs(cfg.metrics_push_interval.max(10)));
    let mut health_check_interval = tokio::time::interval(Duration::from_secs(
        cfg.health_check_interval_seconds.max(1),
    ));
    let (resource_events_tx, mut resource_events_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut resource_watcher = configure_resource_watcher(&cfg, resource_events_tx.clone())?;

    loop {
        tokio::select! {
            _ = sync_interval.tick() => {
                if resource_watcher.is_none() {
                    resource_watcher = configure_resource_watcher(&cfg, resource_events_tx.clone())?;
                }
                if let Err(e) = sync_once(&cfg, &client, &mut state).await {
                    error!("credentials sync failed: {e:#}");
                }
            }
            _ = metrics_interval.tick() => {
                if let Err(e) = push_metrics(&cfg, &client, &mut state).await {
                    warn!("metrics push failed: {e:#}");
                }
                if let Err(e) = push_heartbeat(&cfg, &client, &mut state).await {
                    warn!("heartbeat push failed: {e:#}");
                }
            }
            _ = health_check_interval.tick() => {
                state.last_health_status = Some(if check_health(&cfg.trusttunnel_health_addr).await { "ok".to_string() } else { "degraded".to_string() });
            }
            Some(()) = resource_events_rx.recv() => {
                if let Err(e) = sync_mounted_resources(&cfg, &client, &mut state).await {
                    warn!("resource sync on fs event failed: {e:#}");
                }
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

#[cfg(target_os = "linux")]
struct ResourceWatcher {
    stop: Arc<AtomicBool>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
    fd: RawFd,
}

#[cfg(target_os = "linux")]
impl Drop for ResourceWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = unsafe { libc::close(self.fd) };
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(not(target_os = "linux"))]
struct ResourceWatcher;

#[cfg(not(target_os = "linux"))]
fn configure_resource_watcher(
    _cfg: &Config,
    _tx: tokio::sync::mpsc::UnboundedSender<()>,
) -> Result<Option<ResourceWatcher>> {
    Ok(None)
}

#[cfg(target_os = "linux")]
fn configure_resource_watcher(
    cfg: &Config,
    tx: tokio::sync::mpsc::UnboundedSender<()>,
) -> Result<Option<ResourceWatcher>> {
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    let mut watched = false;
    let mask = libc::IN_CREATE
        | libc::IN_MODIFY
        | libc::IN_DELETE
        | libc::IN_MOVED_FROM
        | libc::IN_MOVED_TO
        | libc::IN_CLOSE_WRITE;

    for root in [&cfg.configs_root, &cfg.secrets_root] {
        if !root.exists() {
            continue;
        }

        let c_path = std::ffi::CString::new(root.to_string_lossy().as_bytes())?;
        let watch_rc = unsafe { libc::inotify_add_watch(fd, c_path.as_ptr(), mask) };
        if watch_rc < 0 {
            warn!(
                "failed to watch {}: {}",
                root.display(),
                std::io::Error::last_os_error()
            );
        } else {
            watched = true;
        }
    }

    if !watched {
        let _ = unsafe { libc::close(fd) };
        return Ok(None);
    }

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let thread_handle = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while !stop_thread.load(Ordering::Relaxed) {
            let bytes_read = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if bytes_read > 0 {
                let _ = tx.send(());
                continue;
            }

            if bytes_read == 0 {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }

            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::WouldBlock {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }

            break;
        }
    });

    Ok(Some(ResourceWatcher {
        stop,
        thread_handle: Some(thread_handle),
        fd,
    }))
}

async fn sync_once(cfg: &Config, client: &reqwest::Client, state: &mut AgentState) -> Result<()> {
    ensure_registration(cfg, client, state).await?;
    sync_once_with_retry(cfg, client, state, 5, Duration::from_secs(1)).await?;
    sync_mounted_resources(cfg, client, state).await
}

async fn sync_mounted_resources(
    cfg: &Config,
    client: &reqwest::Client,
    state: &mut AgentState,
) -> Result<()> {
    let node_id = state
        .node_id
        .as_deref()
        .context("registered node id missing")?;
    let node_token = state
        .node_token
        .as_deref()
        .context("registered node token missing")?;

    let configs = collect_synced_files(&cfg.configs_root)?;
    let secrets = collect_synced_files(&cfg.secrets_root)?;
    let digest = resources_checksum(&configs, &secrets);
    if state.last_files_checksum.as_deref() == Some(&digest) {
        return Ok(());
    }

    let payload = ConfigsPushPayload {
        configs: configs
            .iter()
            .map(|f| ConfigEntry {
                path: f.path.clone(),
                content: f.content.clone(),
                checksum: f.checksum.clone(),
            })
            .collect(),
        secrets: secrets
            .iter()
            .map(|f| {
                let file_name = Path::new(&f.path)
                    .file_name()
                    .and_then(|x| x.to_str())
                    .unwrap_or_default();
                SecretEntry {
                    path: f.path.clone(),
                    keys_count: 1,
                    checksum: f.checksum.clone(),
                    masked: "***".to_string(),
                    value_encrypted: cfg
                        .sync_secret_keys
                        .iter()
                        .any(|k| k == file_name)
                        .then(|| f.content.clone()),
                }
            })
            .collect(),
        collected_at: Utc::now().to_rfc3339(),
    };

    let url = format!(
        "{}/api/trusttunnel/nodes/{}/configs",
        cfg.lk_api_base_url, node_id
    );
    let resp = client
        .post(url)
        .bearer_auth(node_token)
        .json(&payload)
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("configs push rejected: {}", resp.status());
    }

    state.last_files_checksum = Some(digest);
    Ok(())
}

fn collect_synced_files(root: &Path) -> Result<Vec<SyncedFile>> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    collect_synced_files_inner(root, root, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn collect_synced_files_inner(root: &Path, path: &Path, out: &mut Vec<SyncedFile>) -> Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            collect_synced_files_inner(root, &entry_path, out)?;
            continue;
        }

        if file_type.is_file() {
            let rel = entry_path
                .strip_prefix(root)
                .unwrap_or(&entry_path)
                .to_string_lossy()
                .to_string();
            let content = std::fs::read_to_string(&entry_path).unwrap_or_default();
            let checksum = format!("{:x}", Sha256::digest(content.as_bytes()));
            out.push(SyncedFile {
                path: rel,
                content,
                checksum,
            });
        }
    }
    Ok(())
}

fn resources_checksum(configs: &[SyncedFile], secrets: &[SyncedFile]) -> String {
    let canonical = configs
        .iter()
        .map(|x| format!("config:{}:{}", x.path, x.checksum))
        .chain(
            secrets
                .iter()
                .map(|x| format!("secret:{}:{}", x.path, x.checksum)),
        )
        .collect::<Vec<_>>()
        .join("|");
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

async fn ensure_registration(
    cfg: &Config,
    client: &reqwest::Client,
    state: &mut AgentState,
) -> Result<()> {
    if state.node_id.is_some() && state.node_token.is_some() {
        return Ok(());
    }

    if let Some(store) = &mut state.checklist_store {
        let _ = store.mark_event(
            cfg,
            "register",
            "register",
            "started",
            "Registration started",
        );
    }

    let url = format!("{}/api/trusttunnel/nodes/register", cfg.lk_api_base_url);
    let payload = RegisterRequest {
        cluster_id: &cfg.cluster_id,
        public_host: &cfg.public_host,
        endpoint_ip: &cfg.endpoint_ip,
        desired_pool_size: cfg.desired_pool_size,
        weight: cfg.weight,
        stage: &cfg.stage,
        is_enabled: cfg.is_enabled,
        rollout_group: cfg.rollout_group.as_deref(),
        cluster: cfg.cluster.as_deref(),
        namespace: cfg.namespace.as_deref(),
        pod_name: cfg.register_pod_name.as_deref().or(Some(&cfg.pod_name)),
        node_name: &cfg.node_name,
        pod_namespace: &cfg.pod_namespace,
        pod_uid: &cfg.pod_uid,
        pod_ip: &cfg.pod_ip,
    };

    let resp = client
        .post(url)
        .bearer_auth(&cfg.internal_agent_token)
        .json(&payload)
        .send()
        .await?;

    if !resp.status().is_success() {
        if let Some(store) = &mut state.checklist_store {
            let _ = store.mark_event(
                cfg,
                "register",
                "register",
                "failed",
                format!("Registration rejected with status {}", resp.status()),
            );
        }
        anyhow::bail!("registration rejected: {}", resp.status());
    }

    let register_response: RegisterResponse = resp.json().await?;
    state.node_id = Some(register_response.node_id);
    state.node_token = Some(register_response.node_token);
    if let Some(store) = &mut state.checklist_store {
        let _ = store.mark_event(
            cfg,
            "register",
            "register",
            "success",
            "Node registration completed",
        );
    }
    info!("node registration completed");
    Ok(())
}

async fn sync_once_with_retry(
    cfg: &Config,
    client: &reqwest::Client,
    state: &mut AgentState,
    max_retries: usize,
    initial_backoff: Duration,
) -> Result<()> {
    let node_id = state.node_id.as_deref().unwrap_or(&cfg.node_id);
    if let Some(store) = &mut state.checklist_store {
        let _ = store.mark_event(
            cfg,
            "reconcile",
            "reconcile",
            "started",
            "Reconcile started",
        );
    }
    let (outcome, sync_result) = match reconcile_with_retry(
        cfg,
        client,
        node_id,
        state.last_seen_revision.as_deref(),
        max_retries,
        initial_backoff,
    )
    .await
    {
        Ok(reconcile) => {
            let mut outcome = SyncOutcome {
                revision: reconcile.revision.clone(),
                status: "success".to_string(),
                applied_count: 0,
                hash: reconcile.hash.clone(),
                error: None,
            };

            if let Some(store) = &mut state.checklist_store {
                let _ = store.mark_event(
                    cfg,
                    "reconcile",
                    "reconcile",
                    "success",
                    format!("Reconcile received revision {}", reconcile.revision),
                );
            }

            let should_apply = reconcile.apply_instructions.should_apply
                && state.last_seen_revision.as_deref() != Some(&reconcile.revision);

            let sync_result = if should_apply {
                if let Some(store) = &mut state.checklist_store {
                    let _ = store.mark_event(cfg, "apply", "apply", "started", "Apply started");
                }
                let rendered_config = render_runtime_config(
                    &reconcile.runtime_payload,
                    cfg.legacy_credentials_flow_enabled,
                )?;
                let paths = runtime_config_paths(&cfg.credentials_path);
                write_runtime_config_atomically(&paths.staged, &rendered_config.contents)
                    .context("write_staged_credentials_atomically")?;
                switch_staged_to_current(&paths).context("switch_staged_to_current")?;

                let apply_error = if let Err(err) =
                    send_reload_signal(&cfg.trusttunnel_reload_signal).context("send_reload_signal")
                {
                    Some(err)
                } else if !check_health(&cfg.trusttunnel_health_addr).await {
                    Some(anyhow::anyhow!("healthcheck failed after reload"))
                } else {
                    None
                };

                if apply_error.is_none() {
                    info!("applied reconcile revision={} desired_pool={} allocated_pool={} healthy_pool={}",
                        reconcile.revision,
                        reconcile.pool_summary.desired_pool_size,
                        reconcile.pool_summary.allocated_pool_size,
                        reconcile.pool_summary.healthy_pool_size,
                    );
                    state.last_seen_revision = Some(reconcile.revision.clone());
                    state.last_apply_status = Some("applied".to_string());
                    state.last_health_status = Some("ok".to_string());
                    state.last_sync_error = None;
                    if let Some(store) = &mut state.checklist_store {
                        let _ = store.mark_event(
                            cfg,
                            "apply",
                            "apply",
                            "success",
                            format!("Applied reconcile revision {}", reconcile.revision),
                        );
                    }
                    outcome.applied_count = rendered_config.clients_count;
                    Ok(())
                } else {
                    let apply_error = apply_error.expect("apply_error must be set");
                    if let Some(store) = &mut state.checklist_store {
                        let _ = store.mark_event(
                            cfg,
                            "apply",
                            "apply",
                            "failed",
                            format!(
                                "Apply failed for revision {}: {}",
                                reconcile.revision, apply_error
                            ),
                        );
                    }
                    let rollback_result = async {
                        if let Some(store) = &mut state.checklist_store {
                            let _ = store.mark_event(
                                cfg,
                                "rollback",
                                "rollback",
                                "started",
                                format!("Rollback started for revision {}", reconcile.revision),
                            );
                        }
                        rollback_to_previous(&paths).context("rollback_to_previous")?;
                        send_reload_signal(&cfg.trusttunnel_reload_signal)
                            .context("send_reload_signal_for_rollback")?;
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        if !check_health(&cfg.trusttunnel_health_addr).await {
                            anyhow::bail!("healthcheck failed after rollback");
                        }
                        Ok::<(), anyhow::Error>(())
                    }
                    .await;

                    match rollback_result {
                        Ok(_) => {
                            if let Some(store) = &mut state.checklist_store {
                                let _ = store.mark_event(
                                    cfg,
                                    "rollback",
                                    "rollback",
                                    "success",
                                    format!(
                                        "Rollback completed after failed apply for revision {}",
                                        reconcile.revision
                                    ),
                                );
                            }
                            state.last_sync_error = Some(apply_error.to_string());
                            Err(apply_error)
                        }
                        Err(rollback_err) => {
                            if let Some(store) = &mut state.checklist_store {
                                let _ = store.mark_event(
                                    cfg,
                                    "rollback",
                                    "rollback",
                                    "failed",
                                    format!(
                                        "Rollback failed for revision {}: {}",
                                        reconcile.revision, rollback_err
                                    ),
                                );
                            }
                            state.last_sync_error = Some(format!(
                                "{}; rollback failed: {}",
                                apply_error, rollback_err
                            ));
                            Err(anyhow::anyhow!(
                                "{}; rollback failed: {}",
                                apply_error,
                                rollback_err
                            ))
                        }
                    }
                }
            } else {
                state.last_sync_error = None;
                state.last_apply_status = Some("noop".to_string());
                if let Some(store) = &mut state.checklist_store {
                    let _ = store.mark_event(
                        cfg,
                        "apply",
                        "apply",
                        "skipped",
                        format!("No apply required for revision {}", reconcile.revision),
                    );
                }
                Ok(())
            };

            if sync_result.is_err() {
                outcome.status = "failed".to_string();
                outcome.error = sync_result
                    .as_ref()
                    .err()
                    .map(|e| format!("sync failed: {e}"));
            }

            (outcome, sync_result)
        }
        Err(err) => {
            state.last_sync_error = Some(format!("reconcile failed: {err}"));
            if let Some(store) = &mut state.checklist_store {
                let _ = store.mark_event(
                    cfg,
                    "reconcile",
                    "reconcile",
                    "failed",
                    format!("Reconcile failed: {}", err),
                );
            }
            let outcome = SyncOutcome {
                revision: state
                    .last_seen_revision
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                status: "failed".to_string(),
                applied_count: 0,
                hash: "unknown".to_string(),
                error: Some(format!("reconcile failed: {err}")),
            };
            (outcome, Err(err))
        }
    };

    post_sync_report(cfg, client, node_id, outcome).await?;
    sync_result
}

#[derive(Debug)]
struct RenderedRuntimeConfig {
    contents: String,
    clients_count: usize,
}

fn render_runtime_config(
    payload: &RuntimePayload,
    legacy_credentials_flow_enabled: bool,
) -> Result<RenderedRuntimeConfig> {
    if let Some(runtime_config) = payload
        .runtime_config
        .as_ref()
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
    {
        return Ok(RenderedRuntimeConfig {
            contents: runtime_config.to_string(),
            clients_count: count_clients_from_rendered_config(runtime_config),
        });
    }

    if legacy_credentials_flow_enabled {
        let rendered = toml::to_string(&CredentialsFile {
            client: payload.credentials.clone(),
        })?;
        return Ok(RenderedRuntimeConfig {
            clients_count: payload.credentials.len(),
            contents: rendered,
        });
    }

    anyhow::bail!(
        "runtime_payload.runtime_config is required when legacy credentials flow is disabled"
    )
}

async fn reconcile_with_retry(
    cfg: &Config,
    client: &reqwest::Client,
    node_id: &str,
    current_revision: Option<&str>,
    max_retries: usize,
    initial_backoff: Duration,
) -> Result<ReconcileResponse> {
    let mut backoff = initial_backoff;
    for _ in 0..max_retries {
        match reconcile(cfg, client, node_id, current_revision).await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                warn!("reconcile failed: {e:#}; retrying in {:?}", backoff);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }

    reconcile(cfg, client, node_id, current_revision).await
}

async fn reconcile(
    cfg: &Config,
    client: &reqwest::Client,
    node_id: &str,
    current_revision: Option<&str>,
) -> Result<ReconcileResponse> {
    let url = format!(
        "{}/internal/trusttunnel/nodes/reconcile",
        cfg.lk_internal_base_url
    );

    let payload = ReconcileRequest {
        node_id,
        desired_pool_size: cfg.desired_pool_size,
        current_revision,
    };

    let resp = client
        .post(url)
        .bearer_auth(&cfg.internal_agent_token)
        .json(&payload)
        .send()
        .await?;

    if resp.status() != StatusCode::OK {
        anyhow::bail!("unexpected reconcile response status: {}", resp.status());
    }

    Ok(resp.json().await?)
}

fn write_runtime_config_atomically(path: &Path, contents: &str) -> Result<()> {
    let mut fs_ops = FileSystemAtomicWriteOps;
    write_runtime_config_atomically_with_ops(path, contents, &mut fs_ops)
}

fn write_runtime_config_atomically_with_ops(
    path: &Path,
    contents: &str,
    ops: &mut impl AtomicWriteOps,
) -> Result<()> {
    let parent = path.parent().context("credentials path without parent")?;
    ops.create_parent_dir(parent)
        .with_context(|| format!("create parent dir: {}", parent.display()))?;

    let tmp_path = parent.join(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));

    ops.write_tmp_and_sync(&tmp_path, contents.as_bytes())
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
        if signal_name.eq_ignore_ascii_case("NOOP") {
            return Ok(());
        }
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
    node_id: &str,
    outcome: SyncOutcome,
) -> Result<()> {
    let url = format!(
        "{}/internal/trusttunnel/nodes/{}/sync-report",
        cfg.lk_internal_base_url, node_id
    );

    let report = SyncReport {
        revision: outcome.revision,
        status: outcome.status,
        applied_count: outcome.applied_count,
        hash: outcome.hash,
        error: outcome.error,
        collected_at: Utc::now().to_rfc3339(),
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
        node_id: state.node_id.as_deref().unwrap_or(&cfg.node_id),
        active_connections: if state.last_health_status.as_deref() == Some("ok") {
            1
        } else {
            0
        },
        cpu_usage_percent,
        memory_usage_percent,
        bandwidth_mbps,
        error_rate: if state.last_apply_status.as_deref() == Some("failed")
            || state.last_health_status.as_deref() != Some("ok")
        {
            1.0
        } else {
            0.0
        },
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

async fn push_heartbeat(
    cfg: &Config,
    client: &reqwest::Client,
    state: &mut AgentState,
) -> Result<()> {
    ensure_registration(cfg, client, state).await?;
    if let Some(store) = &mut state.checklist_store {
        store.ensure_urls(cfg)?;
    }

    let node_id = state
        .node_id
        .as_deref()
        .context("registered node id missing")?;
    let node_token = state
        .node_token
        .as_deref()
        .context("registered node token missing")?;

    let payload = HeartbeatPayload {
        status: if state.last_apply_status.as_deref() == Some("failed")
            || state.last_health_status.as_deref() == Some("degraded")
        {
            "degraded"
        } else {
            "ok"
        },
        last_seen: Utc::now().to_rfc3339(),
        health: HeartbeatHealth {
            pod_name: &cfg.pod_name,
            pod_namespace: &cfg.pod_namespace,
            node_name: &cfg.node_name,
            pod_ip: &cfg.pod_ip,
            sync_failed: state.last_apply_status.as_deref() == Some("failed"),
            endpoint_healthy: state.last_health_status.as_deref() == Some("ok"),
        },
        clients_count: count_clients_from_runtime_file(&cfg.credentials_path),
        checklist_url: state
            .checklist_store
            .as_ref()
            .and_then(|store| store.checklist_url.clone()),
        akt_url: state
            .checklist_store
            .as_ref()
            .and_then(|store| store.akt_url.clone()),
        last_sync_error: state.last_sync_error.as_deref(),
    };

    let url = format!(
        "{}/api/trusttunnel/nodes/{}/heartbeat",
        cfg.lk_api_base_url, node_id
    );
    let resp = client
        .post(url)
        .bearer_auth(node_token)
        .json(&payload)
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("heartbeat push rejected: {}", resp.status());
    }

    Ok(())
}

fn count_clients_from_rendered_config(content: &str) -> usize {
    toml::from_str::<CredentialsFile>(content)
        .map(|f| f.client.len())
        .unwrap_or(0)
}

fn count_clients_from_runtime_file(path: &Path) -> usize {
    match std::fs::read_to_string(path) {
        Ok(content) => count_clients_from_rendered_config(&content),
        Err(_) => 0,
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

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::json;
    use std::io;

    fn test_config(base_url: String, credentials_path: PathBuf) -> Config {
        Config {
            lk_internal_base_url: base_url.clone(),
            lk_api_base_url: base_url.clone(),
            internal_agent_token: "token".to_string(),
            node_id: "node-1".to_string(),
            sync_interval_seconds: 60,
            credentials_path,
            trusttunnel_reload_signal: "SIGHUP".to_string(),
            trusttunnel_health_addr: "localhost:443".to_string(),
            health_check_interval_seconds: 15,
            metrics_push_interval: 30,
            cluster_id: "cluster-a".to_string(),
            public_host: "gw.example.com".to_string(),
            endpoint_ip: "10.10.0.11".to_string(),
            desired_pool_size: 1,
            weight: 100,
            stage: "prod".to_string(),
            is_enabled: true,
            rollout_group: None,
            cluster: None,
            namespace: None,
            register_pod_name: None,
            pod_name: "pod-1".to_string(),
            pod_namespace: "default".to_string(),
            node_name: "node-a".to_string(),
            pod_uid: "pod-uid".to_string(),
            pod_ip: "10.10.0.11".to_string(),
            configs_root: std::env::temp_dir().join("trusttunnel-configs"),
            secrets_root: std::env::temp_dir().join("trusttunnel-secrets"),
            sync_secret_keys: Vec::new(),
            checklist_path: std::env::temp_dir().join("trusttunnel-checklist.json"),
            artifacts_root: std::env::temp_dir().join("trusttunnel-artifacts"),
            legacy_credentials_flow_enabled: true,
        }
    }

    #[tokio::test]
    async fn sends_failed_report_when_reconcile_fails() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-test-fetch-fail.toml");
        let mut cfg = test_config(server.base_url(), credentials_path);
        cfg.trusttunnel_reload_signal = "BAD".to_string();
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client");
        let mut state = AgentState::default();
        state.checklist_store = ChecklistStore::init(&cfg).ok();

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/reconcile");
                then.status(500);
            })
            .await;

        let failed_report = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/node-1/sync-report")
                    .json_body_partial(json!({ "status": "failed" }).to_string());
                then.status(200);
            })
            .await;

        let result =
            sync_once_with_retry(&cfg, &client, &mut state, 0, Duration::from_millis(1)).await;

        assert!(result.is_err());
        failed_report.assert_async().await;
    }

    #[tokio::test]
    async fn sends_failed_report_when_apply_fails() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-test-checksum-fail.toml");
        let mut cfg = test_config(server.base_url(), credentials_path);
        cfg.trusttunnel_reload_signal = "BAD".to_string();
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client");
        let mut state = AgentState::default();
        state.checklist_store = ChecklistStore::init(&cfg).ok();

        let credentials = vec![Credential {
            username: "alice".to_string(),
            password: "secret".to_string(),
        }];

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/reconcile");
                then.status(200).json_body(json!({
                    "revision": "r1",
                    "hash": "h1",
                    "pool_summary": {},
                    "runtime_payload": {"credentials": credentials},
                    "apply_instructions": {"should_apply": true}
                }));
            })
            .await;

        let failed_report = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/node-1/sync-report")
                    .json_body_partial(json!({ "status": "failed" }).to_string());
                then.status(200);
            })
            .await;

        let result =
            sync_once_with_retry(&cfg, &client, &mut state, 0, Duration::from_millis(1)).await;

        assert!(result.is_err());
        failed_report.assert_async().await;
    }

    #[tokio::test]
    async fn sends_success_report_on_successful_sync() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-test-success.toml");
        let cfg = test_config(server.base_url(), credentials_path);
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client");

        let credentials = vec![Credential {
            username: "bob".to_string(),
            password: "pw".to_string(),
        }];
        let _snapshot = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/reconcile");
                then.status(200).json_body(json!({
                    "revision": "r2",
                    "hash": "h2",
                    "pool_summary": {},
                    "runtime_payload": {"credentials": credentials},
                    "apply_instructions": {"should_apply": true}
                }));
            })
            .await;

        let success_report = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/node-1/sync-report")
                    .json_body_partial(
                        json!({ "status": "success", "revision": "r2" }).to_string(),
                    );
                then.status(200);
            })
            .await;

        let mut state = AgentState {
            last_seen_revision: Some("r2".to_string()),
            ..Default::default()
        };

        let result =
            sync_once_with_retry(&cfg, &client, &mut state, 0, Duration::from_millis(1)).await;

        assert!(result.is_ok());
        success_report.assert_async().await;
    }

    #[tokio::test]
    async fn skips_apply_when_revision_unchanged() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-test-noop.toml");
        std::fs::write(
            &credentials_path,
            "client = []
",
        )
        .expect("seed credentials file");
        let cfg = test_config(server.base_url(), credentials_path.clone());
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client");

        let reconcile_mock = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/reconcile");
                then.status(200).json_body(json!({
                    "revision": "r2",
                    "hash": "h2",
                    "pool_summary": {},
                    "runtime_payload": {"credentials": [{"username": "new", "password": "pw"}]},
                    "apply_instructions": {"should_apply": false}
                }));
            })
            .await;

        let success_report = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/node-1/sync-report")
                    .json_body_partial(
                        json!({ "status": "success", "applied_count": 0 }).to_string(),
                    );
                then.status(200);
            })
            .await;

        let mut state = AgentState {
            last_seen_revision: Some("r2".to_string()),
            ..Default::default()
        };

        sync_once_with_retry(&cfg, &client, &mut state, 0, Duration::from_millis(1))
            .await
            .expect("sync should be no-op");

        let content = std::fs::read_to_string(&credentials_path).expect("read credentials");
        assert_eq!(
            content,
            "client = []
"
        );
        reconcile_mock.assert_async().await;
        success_report.assert_async().await;
    }

    #[tokio::test]
    async fn registers_once_and_pushes_heartbeat_with_node_token() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-test-heartbeat.toml");
        let cfg = test_config(server.base_url(), credentials_path);
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client");
        let mut state = AgentState {
            last_apply_status: Some("success".to_string()),
            last_health_status: Some("ok".to_string()),
            checklist_store: ChecklistStore::init(&cfg).ok(),
            ..Default::default()
        };

        let register_mock = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/api/trusttunnel/nodes/register")
                    .header("authorization", "Bearer token");
                then.status(200).json_body(json!({
                    "node_id": "registered-node",
                    "node_token": "registered-token"
                }));
            })
            .await;

        let heartbeat_mock = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/api/trusttunnel/nodes/registered-node/heartbeat")
                    .header("authorization", "Bearer registered-token")
                    .json_body_partial(json!({ "status": "ok" }).to_string());
                then.status(200);
            })
            .await;

        push_heartbeat(&cfg, &client, &mut state)
            .await
            .expect("heartbeat push should succeed");

        register_mock.assert_async().await;
        heartbeat_mock.assert_async().await;
    }

    #[tokio::test]
    async fn ensure_registration_is_idempotent_without_duplicate_register_requests() {
        let server = MockServer::start_async().await;
        let cfg = test_config(
            server.base_url(),
            std::env::temp_dir().join("trusttunnel-test-idempotent-register.toml"),
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client");
        let mut state = AgentState::default();

        let register_mock = server
            .mock_async(|when, then| {
                when.method(POST).path("/api/trusttunnel/nodes/register");
                then.status(200).json_body(json!({
                    "node_id": "registered-node",
                    "node_token": "registered-token"
                }));
            })
            .await;

        ensure_registration(&cfg, &client, &mut state)
            .await
            .expect("first registration");
        ensure_registration(&cfg, &client, &mut state)
            .await
            .expect("second registration");

        register_mock.assert_hits_async(1).await;
        assert_eq!(state.node_id.as_deref(), Some("registered-node"));
    }

    #[tokio::test]
    async fn reregister_on_restart_reuses_single_lk_node_id_for_requests() {
        let server = MockServer::start_async().await;
        let cfg = test_config(
            server.base_url(),
            std::env::temp_dir().join("trusttunnel-test-reregister-restart.toml"),
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client");

        let register_mock = server
            .mock_async(|when, then| {
                when.method(POST).path("/api/trusttunnel/nodes/register");
                then.status(200).json_body(json!({
                    "node_id": "stable-node-id",
                    "node_token": "stable-token"
                }));
            })
            .await;

        let heartbeat_mock = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/api/trusttunnel/nodes/stable-node-id/heartbeat")
                    .header("authorization", "Bearer stable-token");
                then.status(200);
            })
            .await;

        let mut first_state = AgentState {
            last_apply_status: Some("success".to_string()),
            last_health_status: Some("ok".to_string()),
            ..Default::default()
        };
        push_heartbeat(&cfg, &client, &mut first_state)
            .await
            .expect("first heartbeat");

        let mut second_state = AgentState {
            last_apply_status: Some("success".to_string()),
            last_health_status: Some("ok".to_string()),
            ..Default::default()
        };
        push_heartbeat(&cfg, &client, &mut second_state)
            .await
            .expect("second heartbeat after restart");

        register_mock.assert_hits_async(2).await;
        heartbeat_mock.assert_hits_async(2).await;
    }

    fn free_local_addr() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        let addr = listener.local_addr().expect("local addr");
        drop(listener);
        addr.to_string()
    }

    #[tokio::test]
    async fn reload_fail_triggers_failed_sync_report() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-test-reload-fail.toml");
        std::fs::write(
            &credentials_path,
            "client = []
",
        )
        .expect("seed current");
        let mut cfg = test_config(server.base_url(), credentials_path.clone());
        cfg.trusttunnel_reload_signal = "BAD".to_string();
        cfg.trusttunnel_health_addr = free_local_addr();
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client");

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/reconcile");
                then.status(200).json_body(json!({
                    "revision": "r-reload-fail",
                    "hash": "h1",
                    "pool_summary": {},
                    "runtime_payload": {"runtime_config": "client = []"},
                    "apply_instructions": {"should_apply": true}
                }));
            })
            .await;

        let failed_report = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/node-1/sync-report")
                    .json_body_partial(json!({ "status": "failed" }).to_string());
                then.status(200);
            })
            .await;

        let mut state = AgentState::default();
        let result =
            sync_once_with_retry(&cfg, &client, &mut state, 0, Duration::from_millis(1)).await;
        assert!(result.is_err());
        assert!(state
            .last_sync_error
            .as_deref()
            .unwrap_or_default()
            .contains("rollback failed"));
        failed_report.assert_async().await;
    }

    #[tokio::test]
    async fn health_fail_after_reload_rolls_back_and_sets_last_sync_error() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-test-health-fail.toml");
        std::fs::write(
            &credentials_path,
            "client = []
",
        )
        .expect("seed current");
        let mut cfg = test_config(server.base_url(), credentials_path.clone());
        cfg.trusttunnel_reload_signal = "NOOP".to_string();
        cfg.trusttunnel_health_addr = free_local_addr();
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client");

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/reconcile");
                then.status(200).json_body(json!({
                    "revision": "r-health-fail",
                    "hash": "h1",
                    "pool_summary": {},
                    "runtime_payload": {"runtime_config": "[[client]]
username='u'
password='p'
"},
                    "apply_instructions": {"should_apply": true}
                }));
            })
            .await;

        let _failed_report = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/node-1/sync-report");
                then.status(200);
            })
            .await;

        let mut state = AgentState::default();
        let result =
            sync_once_with_retry(&cfg, &client, &mut state, 0, Duration::from_millis(1)).await;
        assert!(result.is_err());
        assert!(state
            .last_sync_error
            .as_deref()
            .unwrap_or_default()
            .contains("healthcheck failed after reload"));
    }

    #[tokio::test]
    async fn rollback_success_after_health_fail_restores_previous_config() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-test-rollback-success.toml");
        std::fs::write(
            &credentials_path,
            "client = []
",
        )
        .expect("seed current");
        let mut cfg = test_config(server.base_url(), credentials_path.clone());
        cfg.trusttunnel_reload_signal = "NOOP".to_string();
        let addr = free_local_addr();
        cfg.trusttunnel_health_addr = addr.clone();
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client");

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _listener = tokio::net::TcpListener::bind(addr)
                .await
                .expect("bind health");
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/reconcile");
                then.status(200).json_body(json!({
                    "revision": "r-rollback-success",
                    "hash": "h1",
                    "pool_summary": {},
                    "runtime_payload": {"runtime_config": "[[client]]
username='x'
password='y'
"},
                    "apply_instructions": {"should_apply": true}
                }));
            })
            .await;

        let _report = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/node-1/sync-report");
                then.status(200);
            })
            .await;

        let mut state = AgentState::default();
        let result =
            sync_once_with_retry(&cfg, &client, &mut state, 0, Duration::from_millis(1)).await;
        assert!(result.is_err());
        let restored = std::fs::read_to_string(&credentials_path).expect("read current");
        assert_eq!(
            restored,
            "client = []
"
        );
        assert!(state
            .last_sync_error
            .as_deref()
            .unwrap_or_default()
            .contains("healthcheck failed after reload"));
    }

    #[tokio::test]
    async fn rollback_failure_sets_combined_error() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-test-rollback-failure.toml");
        std::fs::write(
            &credentials_path,
            "client = []
",
        )
        .expect("seed current");
        let mut cfg = test_config(server.base_url(), credentials_path.clone());
        cfg.trusttunnel_reload_signal = "NOOP".to_string();
        cfg.trusttunnel_health_addr = free_local_addr();
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client");

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/reconcile");
                then.status(200).json_body(json!({
                    "revision": "r-rollback-failure",
                    "hash": "h1",
                    "pool_summary": {},
                    "runtime_payload": {"runtime_config": "[[client]]
username='x'
password='y'
"},
                    "apply_instructions": {"should_apply": true}
                }));
            })
            .await;

        let _report = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/node-1/sync-report");
                then.status(200);
            })
            .await;

        let previous_path = runtime_config_paths(&credentials_path).previous;
        std::fs::remove_file(&previous_path).ok();

        let mut state = AgentState::default();
        let result =
            sync_once_with_retry(&cfg, &client, &mut state, 0, Duration::from_millis(1)).await;
        assert!(result.is_err());
        assert!(state
            .last_sync_error
            .as_deref()
            .unwrap_or_default()
            .contains("rollback failed"));
    }

    #[test]
    fn collects_files_from_nested_directories() {
        let root = std::env::temp_dir().join(format!("trusttunnel-collect-{}", std::process::id()));
        let nested = root.join("app");
        std::fs::create_dir_all(&nested).expect("create dirs");
        std::fs::write(nested.join("config.yaml"), "abc").expect("write");

        let files = collect_synced_files(&root).expect("collect files");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "app/config.yaml");
        assert_eq!(files[0].content, "abc");

        let _ = std::fs::remove_dir_all(&root);
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
    fn write_runtime_config_follows_expected_operation_order() {
        let mut ops = RecordingAtomicWriteOps::default();
        let path = Path::new("/tmp/trusttunnel/credentials.toml");
        write_runtime_config_atomically_with_ops(
            path,
            "[[client]]\nusername=\"user\"\npassword=\"pass\"\n",
            &mut ops,
        )
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
    #[test]
    fn checklist_store_initializes_and_writes_file() {
        let cfg = test_config(
            "http://localhost".to_string(),
            std::env::temp_dir().join("cred-a.toml"),
        );
        let _ = std::fs::remove_file(&cfg.checklist_path);
        let store = ChecklistStore::init(&cfg).expect("init checklist");
        assert!(store.checklist_url.is_some());
        let raw = std::fs::read_to_string(&cfg.checklist_path).expect("checklist file");
        assert!(raw.contains("\"register\""));
    }

    #[test]
    fn counts_clients_from_toml_credentials() {
        let file = std::env::temp_dir().join("trusttunnel-clients-count.toml");
        std::fs::write(
            &file,
            "[[client]]\nusername='a'\npassword='1'\n[[client]]\nusername='b'\npassword='2'\n",
        )
        .expect("write");
        assert_eq!(count_clients_from_runtime_file(&file), 2);
    }

    #[test]
    fn render_runtime_config_prefers_runtime_payload_config() {
        let payload = RuntimePayload {
            runtime_config: Some("[[client]]\nusername='alice'\npassword='pw'\n".to_string()),
            credentials: vec![Credential {
                username: "legacy".to_string(),
                password: "legacy".to_string(),
            }],
        };

        let rendered = render_runtime_config(&payload, false).expect("render runtime config");
        assert!(rendered.contents.contains("alice"));
        assert_eq!(rendered.clients_count, 1);
    }

    #[test]
    fn render_runtime_config_fails_without_feature_flag_and_runtime_config() {
        let payload = RuntimePayload {
            runtime_config: None,
            credentials: vec![Credential {
                username: "legacy".to_string(),
                password: "legacy".to_string(),
            }],
        };

        let err = render_runtime_config(&payload, false).expect_err("must fail");
        assert!(err
            .to_string()
            .contains("runtime_payload.runtime_config is required"));
    }

    #[test]
    fn render_runtime_config_uses_legacy_credentials_when_enabled() {
        let payload = RuntimePayload {
            runtime_config: None,
            credentials: vec![Credential {
                username: "legacy".to_string(),
                password: "legacy".to_string(),
            }],
        };

        let rendered = render_runtime_config(&payload, true).expect("legacy render");
        assert!(rendered.contents.contains("legacy"));
        assert_eq!(rendered.clients_count, 1);
    }
}
