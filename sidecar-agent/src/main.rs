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
    clients_export_path: PathBuf,
    clients_configmap_name: Option<String>,
    clients_configmap_namespace: String,
    clients_configmap_key: String,
    kube_api_url: String,
    kube_token_path: PathBuf,
}

impl Config {
    fn from_env() -> Result<Self> {
        Ok(Self {
            lk_internal_base_url: std::env::var("LK_INTERNAL_BASE_URL")?,
            lk_api_base_url: std::env::var("LK_API_BASE_URL")
                .or_else(|_| std::env::var("LK_INTERNAL_BASE_URL"))?,
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
            cluster_id: std::env::var("CLUSTER_ID").unwrap_or_else(|_| "unknown".to_string()),
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
            clients_export_path: PathBuf::from(
                std::env::var("SIDECAR_CLIENTS_EXPORT_PATH")
                    .unwrap_or_else(|_| "/tmp/clients.json".to_string()),
            ),
            clients_configmap_name: std::env::var("SIDECAR_CLIENTS_CONFIGMAP").ok(),
            clients_configmap_namespace: std::env::var("SIDECAR_CLIENTS_CONFIGMAP_NAMESPACE")
                .unwrap_or_else(|_| std::env::var("POD_NAMESPACE").unwrap_or_else(|_| "default".to_string())),
            clients_configmap_key: std::env::var("SIDECAR_CLIENTS_CONFIGMAP_KEY")
                .unwrap_or_else(|_| "clients.json".to_string()),
            kube_api_url: std::env::var("KUBERNETES_SERVICE_HOST")
                .map(|host| {
                    let port = std::env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".to_string());
                    format!("https://{host}:{port}")
                })
                .unwrap_or_else(|_| "https://kubernetes.default.svc:443".to_string()),
            kube_token_path: PathBuf::from(
                std::env::var("KUBE_TOKEN_PATH").unwrap_or_else(|_| "/var/run/secrets/kubernetes.io/serviceaccount/token".to_string()),
            ),
        })
    }
}

#[derive(Serialize)]
struct RegisterRequest<'a> {
    cluster_id: &'a str,
    node_name: &'a str,
    pod_name: &'a str,
    pod_namespace: &'a str,
    pod_uid: &'a str,
    pod_ip: &'a str,
}

#[derive(Deserialize)]
struct RegisterResponse {
    node_id: String,
    node_token: String,
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
struct SyncReport {
    version: String,
    status: String,
    applied_count: usize,
    checksum: String,
    error: Option<String>,
    collected_at: String,
}

#[derive(Clone)]
struct SyncOutcome {
    version: String,
    status: String,
    applied_count: usize,
    checksum: String,
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
    last_version: Option<String>,
    last_checksum: Option<String>,
    last_network_total: Option<u64>,
    last_sync_failed: bool,
    last_health_ok: bool,
    registered_node_id: Option<String>,
    node_token: Option<String>,
    last_files_checksum: Option<String>,
    checklist_store: Option<ChecklistStore>,
}

#[derive(Clone, Serialize, Deserialize)]
struct ChecklistTask {
    id: String,
    title: String,
    status: String,
    done_at: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct AktReport {
    generated_at: String,
    tasks_completed: Vec<String>,
    summary: String,
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
                    id: "register-node".to_string(),
                    title: "Register node in LK".to_string(),
                    status: "pending".to_string(),
                    done_at: None,
                },
                ChecklistTask {
                    id: "sync-configmap".to_string(),
                    title: "Sync ConfigMap".to_string(),
                    status: "pending".to_string(),
                    done_at: None,
                },
                ChecklistTask {
                    id: "send-heartbeat".to_string(),
                    title: "Send heartbeat".to_string(),
                    status: "pending".to_string(),
                    done_at: None,
                },
            ],
            akt: None,
        };

        write_json_pretty(&cfg.checklist_path, &doc)?;
        Ok(Self {
            checklist_url: Some(format!("file://{}", cfg.checklist_path.display())),
            akt_url: None,
        })
    }

    fn mark_done(&mut self, cfg: &Config, task_id: &str, command_id: &str) -> Result<()> {
        let text = std::fs::read_to_string(&cfg.checklist_path)?;
        let mut doc: ChecklistDocument = serde_json::from_str(&text)?;
        for task in &mut doc.checklist {
            if task.id == task_id {
                task.status = "done".to_string();
                task.done_at = Some(Utc::now().to_rfc3339());
            }
        }
        let completed = doc
            .checklist
            .iter()
            .filter(|t| t.status == "done")
            .map(|t| t.id.clone())
            .collect::<Vec<_>>();
        let akt = AktReport {
            generated_at: Utc::now().to_rfc3339(),
            tasks_completed: completed,
            summary: format!("Checklist progress updated after {task_id}"),
        };
        doc.akt = Some(akt);
        write_json_pretty(&cfg.checklist_path, &doc)?;
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
                "- [{}] {}",
                if t.status == "done" { "x" } else { " " },
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
                if let Err(e) = push_heartbeat(&cfg, &client, &mut state).await {
                    warn!("heartbeat push failed: {e:#}");
                }
            }
            _ = health_check_interval.tick() => {
                state.last_health_ok = check_health(&cfg.trusttunnel_health_addr).await;
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
        .registered_node_id
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
    if state.registered_node_id.is_some() && state.node_token.is_some() {
        return Ok(());
    }

    let url = format!("{}/api/trusttunnel/nodes/register", cfg.lk_api_base_url);
    let payload = RegisterRequest {
        cluster_id: &cfg.cluster_id,
        node_name: &cfg.node_name,
        pod_name: &cfg.pod_name,
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
        anyhow::bail!("registration rejected: {}", resp.status());
    }

    let register_response: RegisterResponse = resp.json().await?;
    state.registered_node_id = Some(register_response.node_id);
    state.node_token = Some(register_response.node_token);
    if let Some(store) = &mut state.checklist_store {
        let _ = store.mark_done(cfg, "register-node", "register");
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
    let (outcome, sync_result) =
        match fetch_snapshot_with_retry(cfg, client, max_retries, initial_backoff).await {
            Ok(snapshot) => {
                let mut outcome = SyncOutcome {
                    version: snapshot.version.clone(),
                    status: "success".to_string(),
                    applied_count: snapshot.credentials.len(),
                    checksum: snapshot.checksum.clone(),
                    error: None,
                };

                let is_new = state.last_version.as_deref() != Some(&snapshot.version)
                    || state.last_checksum.as_deref() != Some(&snapshot.checksum);

                let sync_result = (|| -> Result<()> {
                    if is_new {
                        validate_checksum(&snapshot).context("validate_checksum")?;
                        write_credentials_atomically(&cfg.credentials_path, &snapshot.credentials)
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
                    let _ = sync_clients_targets(cfg, client, &snapshot.credentials).await;
                    state.last_version = Some(snapshot.version);
                    state.last_checksum = Some(snapshot.checksum);
                    if let Some(store) = &mut state.checklist_store {
                        let _ = store.mark_done(cfg, "sync-configmap", "sync-configmap");
                    }
                }

                (outcome, sync_result)
            }
            Err(err) => {
                let outcome = SyncOutcome {
                    version: "unknown".to_string(),
                    status: "failed".to_string(),
                    applied_count: 0,
                    checksum: "unknown".to_string(),
                    error: Some(format!("fetch_snapshot failed: {err}")),
                };
                (outcome, Err(err))
            }
        };

    post_sync_report(cfg, client, outcome).await?;
    sync_result
}

async fn sync_clients_targets(cfg: &Config, client: &reqwest::Client, credentials: &[Credential]) -> Result<()> {
    write_clients_export_file(&cfg.clients_export_path, credentials)?;
    if let Some(configmap_name) = &cfg.clients_configmap_name {
        sync_clients_configmap(cfg, client, configmap_name, credentials).await?;
    }
    Ok(())
}

fn write_clients_export_file(path: &Path, credentials: &[Credential]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(credentials)?)?;
    Ok(())
}

async fn sync_clients_configmap(
    cfg: &Config,
    client: &reqwest::Client,
    configmap_name: &str,
    credentials: &[Credential],
) -> Result<()> {
    let token = std::fs::read_to_string(&cfg.kube_token_path)
        .map(|x| x.trim().to_string())
        .context("read kube token")?;
    let clients_json = serde_json::to_string_pretty(credentials)?;
    let url = format!(
        "{}/api/v1/namespaces/{}/configmaps/{}",
        cfg.kube_api_url, cfg.clients_configmap_namespace, configmap_name
    );

    for attempt in 0..3 {
        let patch_body = serde_json::json!({
            "data": {
                cfg.clients_configmap_key.clone(): clients_json,
            }
        });
        let resp = client
            .patch(&url)
            .bearer_auth(&token)
            .header("Content-Type", "application/merge-patch+json")
            .json(&patch_body)
            .send()
            .await?;

        if resp.status().is_success() {
            return Ok(());
        }

        if resp.status().as_u16() == 409 && attempt < 2 {
            tokio::time::sleep(Duration::from_millis(200 * (attempt + 1) as u64)).await;
            continue;
        }

        anyhow::bail!("configmap sync rejected: {}", resp.status());
    }

    anyhow::bail!("configmap sync conflict retry exhausted")
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
    let checksum = canonical_checksum(&snapshot.credentials);
    if checksum != snapshot.checksum {
        anyhow::bail!("snapshot checksum mismatch");
    }
    Ok(())
}

fn canonical_checksum(credentials: &[Credential]) -> String {
    let mut canonical_credentials = credentials.to_vec();
    canonical_credentials.sort_by(|a, b| {
        a.username
            .cmp(&b.username)
            .then_with(|| a.password.cmp(&b.password))
    });

    let entries = canonical_credentials
        .iter()
        .map(|credential| {
            format!(
                "{{\"username\":\"{}\",\"password\":\"{}\"}}",
                escape_json_string(&credential.username),
                escape_json_string(&credential.password)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let raw = format!("{{\"credentials\":[{}]}}", entries);

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

fn write_credentials_atomically(path: &Path, credentials: &[Credential]) -> Result<()> {
    let mut fs_ops = FileSystemAtomicWriteOps;
    write_credentials_atomically_with_ops(path, credentials, &mut fs_ops)
}

fn write_credentials_atomically_with_ops(
    path: &Path,
    credentials: &[Credential],
    ops: &mut impl AtomicWriteOps,
) -> Result<()> {
    let parent = path.parent().context("credentials path without parent")?;
    ops.create_parent_dir(parent)
        .with_context(|| format!("create parent dir: {}", parent.display()))?;

    let tmp_path = parent.join(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));

    let toml = toml::to_string(&CredentialsFile {
        client: credentials.to_vec(),
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
        "{}/internal/trusttunnel/nodes/{}/sync-report",
        cfg.lk_internal_base_url, cfg.node_id
    );

    let report = SyncReport {
        version: outcome.version,
        status: outcome.status,
        applied_count: outcome.applied_count,
        checksum: outcome.checksum,
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
        node_id: &cfg.node_id,
        active_connections: if state.last_health_ok { 1 } else { 0 },
        cpu_usage_percent,
        memory_usage_percent,
        bandwidth_mbps,
        error_rate: if state.last_sync_failed || !state.last_health_ok {
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

    let node_id = state
        .registered_node_id
        .as_deref()
        .context("registered node id missing")?;
    let node_token = state
        .node_token
        .as_deref()
        .context("registered node token missing")?;

    let payload = HeartbeatPayload {
        status: if state.last_sync_failed || !state.last_health_ok {
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
            sync_failed: state.last_sync_failed,
            endpoint_healthy: state.last_health_ok,
        },
        clients_count: count_clients_from_credentials_file(&cfg.credentials_path),
        checklist_url: state
            .checklist_store
            .as_ref()
            .and_then(|store| store.checklist_url.clone()),
        akt_url: state
            .checklist_store
            .as_ref()
            .and_then(|store| store.akt_url.clone()),
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

    if let Some(store) = &mut state.checklist_store {
        let _ = store.mark_done(cfg, "send-heartbeat", "heartbeat");
    }

    Ok(())
}

fn count_clients_from_credentials_file(path: &Path) -> usize {
    match std::fs::read_to_string(path) {
        Ok(content) => toml::from_str::<CredentialsFile>(&content)
            .map(|f| f.client.len())
            .unwrap_or(0),
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
            clients_export_path: std::env::temp_dir().join("trusttunnel-clients.json"),
            clients_configmap_name: None,
            clients_configmap_namespace: "default".to_string(),
            clients_configmap_key: "clients.json".to_string(),
            kube_api_url: base_url,
            kube_token_path: std::env::temp_dir().join("trusttunnel-kube-token"),
        }
    }

    fn checksum_for(credentials: Vec<Credential>) -> String {
        canonical_checksum(&credentials)
    }

    fn legacy_toml_checksum_for(credentials: Vec<Credential>) -> String {
        let raw = toml::to_string(&CredentialsFile {
            client: credentials,
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
            .expect("client");
        let mut state = AgentState::default();
        state.checklist_store = ChecklistStore::init(&cfg).ok();

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/internal/trusttunnel/nodes/node-1/credentials-snapshot");
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
    async fn sends_failed_report_when_checksum_mismatch() {
        let server = MockServer::start_async().await;
        let credentials_path = std::env::temp_dir().join("trusttunnel-test-checksum-fail.toml");
        let cfg = test_config(server.base_url(), credentials_path);
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
                when.method(GET)
                    .path("/internal/trusttunnel/nodes/node-1/credentials-snapshot");
                then.status(200).json_body(json!({
                    "version": "v1",
                    "credentials": credentials,
                    "checksum": "deadbeef"
                }));
            })
            .await;

        let failed_report = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/node-1/sync-report")
                    .json_body_partial(json!({ "status": "failed", "version": "v1" }).to_string());
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
        let checksum = checksum_for(credentials.clone());

        let _snapshot = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/internal/trusttunnel/nodes/node-1/credentials-snapshot");
                then.status(200).json_body(json!({
                    "version": "v2",
                    "credentials": credentials,
                    "checksum": checksum
                }));
            })
            .await;

        let success_report = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/internal/trusttunnel/nodes/node-1/sync-report")
                    .json_body_partial(json!({ "status": "success", "version": "v2" }).to_string());
                then.status(200);
            })
            .await;

        let mut state = AgentState {
            last_version: Some("v2".to_string()),
            last_checksum: Some(checksum),
            ..Default::default()
        };

        let result =
            sync_once_with_retry(&cfg, &client, &mut state, 0, Duration::from_millis(1)).await;

        assert!(result.is_ok());
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
            last_sync_failed: false,
            last_health_ok: true,
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

    #[test]
    fn checksum_rejects_legacy_toml_algorithm() {
        let snapshot = SnapshotResponse {
            version: "v-legacy".to_string(),
            credentials: vec![Credential {
                username: "alice".to_string(),
                password: "secret".to_string(),
            }],
            checksum: legacy_toml_checksum_for(vec![Credential {
                username: "alice".to_string(),
                password: "secret".to_string(),
            }]),
        };

        assert!(validate_checksum(&snapshot).is_err());
    }

    #[test]
    fn checksum_accepts_documented_canonical_example() {
        let snapshot = SnapshotResponse {
            version: "v-doc-example".to_string(),
            credentials: vec![
                Credential {
                    username: "bob".to_string(),
                    password: "pw2".to_string(),
                },
                Credential {
                    username: "alice".to_string(),
                    password: "pw1".to_string(),
                },
            ],
            checksum: "84cf9958ba7047e33b96652394c2ee7314185913a2517bf89954472c1bdafb14"
                .to_string(),
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
    fn write_credentials_follows_expected_operation_order() {
        let mut ops = RecordingAtomicWriteOps::default();
        let path = Path::new("/tmp/trusttunnel/credentials.toml");
        let credentials = vec![Credential {
            username: "user".to_string(),
            password: "pass".to_string(),
        }];

        write_credentials_atomically_with_ops(path, &credentials, &mut ops)
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
        assert!(raw.contains("register-node"));
    }

    #[test]
    fn counts_clients_from_toml_credentials() {
        let file = std::env::temp_dir().join("trusttunnel-clients-count.toml");
        std::fs::write(
            &file,
            "[[client]]\nusername='a'\npassword='1'\n[[client]]\nusername='b'\npassword='2'\n",
        )
        .expect("write");
        assert_eq!(count_clients_from_credentials_file(&file), 2);
    }

    #[test]
    fn writes_clients_export_file_as_json() {
        let path = std::env::temp_dir().join("trusttunnel-clients-export.json");
        let creds = vec![Credential {
            username: "alice".to_string(),
            password: "pw".to_string(),
        }];

        write_clients_export_file(&path, &creds).expect("write clients export");
        let raw = std::fs::read_to_string(&path).expect("read export");
        assert!(raw.contains("alice"));
    }

    #[tokio::test]
    async fn syncs_clients_configmap_using_patch() {
        let server = MockServer::start_async().await;
        let mut cfg = test_config(
            server.base_url(),
            std::env::temp_dir().join("trusttunnel-test-cm.toml"),
        );
        cfg.clients_configmap_name = Some("clients-cm".to_string());
        cfg.kube_token_path = std::env::temp_dir().join("trusttunnel-kube-token-sync");
        std::fs::write(&cfg.kube_token_path, "kube-token").expect("token write");

        let patch_mock = server
            .mock_async(|when, then| {
                when.method("PATCH")
                    .path("/api/v1/namespaces/default/configmaps/clients-cm")
                    .header("authorization", "Bearer kube-token");
                then.status(200).json_body(json!({"ok": true}));
            })
            .await;

        let client = reqwest::Client::builder().no_proxy().build().expect("client");
        sync_clients_configmap(
            &cfg,
            &client,
            "clients-cm",
            &[Credential {
                username: "u1".to_string(),
                password: "p1".to_string(),
            }],
        )
        .await
        .expect("configmap sync");

        patch_mock.assert_async().await;
    }

}
