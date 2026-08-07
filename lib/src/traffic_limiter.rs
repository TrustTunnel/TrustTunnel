use crate::authentication::registry_based::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrafficDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct UsageRecord {
    inbound: u64,
    outbound: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClientTrafficSummary {
    pub inbound: u64,
    pub outbound: u64,
    pub limit: Option<u64>,
    pub quota_exceeded: bool,
}

struct ClientEntry {
    limit: Option<u64>,
    usage: UsageRecord,
}

/// Tracks per-client traffic usage and enforces optional byte quotas.
pub(crate) struct TrafficLimiter {
    clients: Mutex<HashMap<String, ClientEntry>>,
    usage_file: Option<PathBuf>,
    last_persist: Mutex<Instant>,
    persist_interval: Duration,
}

impl TrafficLimiter {
    pub fn new(
        clients: &[Client],
        default_limit: Option<u64>,
        usage_file: Option<PathBuf>,
    ) -> Arc<Self> {
        let mut map = Self::load_usage(usage_file.as_deref());

        for client in clients {
            let entry = map.entry(client.username.clone()).or_insert_with(|| ClientEntry {
                limit: client.max_traffic_bytes.or(default_limit),
                usage: UsageRecord::default(),
            });
            entry.limit = client.max_traffic_bytes.or(default_limit);
        }

        Arc::new(Self {
            clients: Mutex::new(map),
            usage_file,
            last_persist: Mutex::new(Instant::now()),
            persist_interval: Duration::from_secs(30),
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.clients
            .lock()
            .unwrap()
            .values()
            .any(|entry| entry.limit.is_some())
    }

    pub fn is_allowed(&self, username: &str) -> bool {
        let clients = self.clients.lock().unwrap();
        let Some(entry) = clients.get(username) else {
            return true;
        };
        Self::entry_allowed(entry)
    }

    pub fn record(&self, username: &str, direction: TrafficDirection, bytes: usize) -> bool {
        if bytes == 0 {
            return self.is_allowed(username);
        }

        let still_allowed = {
            let mut clients = self.clients.lock().unwrap();
            let entry = clients
                .entry(username.to_string())
                .or_insert_with(|| ClientEntry {
                    limit: None,
                    usage: UsageRecord::default(),
                });
            match direction {
                TrafficDirection::Inbound => {
                    entry.usage.inbound = entry.usage.inbound.saturating_add(bytes as u64);
                }
                TrafficDirection::Outbound => {
                    entry.usage.outbound = entry.usage.outbound.saturating_add(bytes as u64);
                }
            }
            Self::entry_allowed(entry)
        };

        self.maybe_persist();
        still_allowed
    }

    pub fn summary(&self, username: &str) -> ClientTrafficSummary {
        let clients = self.clients.lock().unwrap();
        let Some(entry) = clients.get(username) else {
            return ClientTrafficSummary {
                inbound: 0,
                outbound: 0,
                limit: None,
                quota_exceeded: false,
            };
        };
        ClientTrafficSummary {
            inbound: entry.usage.inbound,
            outbound: entry.usage.outbound,
            limit: entry.limit,
            quota_exceeded: !Self::entry_allowed(entry),
        }
    }

    pub fn all_summaries(&self) -> HashMap<String, ClientTrafficSummary> {
        self.clients
            .lock()
            .unwrap()
            .iter()
            .map(|(username, entry)| {
                (
                    username.clone(),
                    ClientTrafficSummary {
                        inbound: entry.usage.inbound,
                        outbound: entry.usage.outbound,
                        limit: entry.limit,
                        quota_exceeded: !Self::entry_allowed(entry),
                    },
                )
            })
            .collect()
    }

    fn entry_allowed(entry: &ClientEntry) -> bool {
        match entry.limit {
            Some(limit) => entry.usage.inbound.saturating_add(entry.usage.outbound) <= limit,
            None => true,
        }
    }

    fn load_usage(path: Option<&Path>) -> HashMap<String, ClientEntry> {
        let Some(path) = path else {
            return HashMap::new();
        };
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(_) => return HashMap::new(),
        };
        let stored: HashMap<String, UsageRecord> = match toml::from_str(&content) {
            Ok(stored) => stored,
            Err(e) => {
                log::warn!("Couldn't parse traffic usage file {}: {}", path.display(), e);
                return HashMap::new();
            }
        };
        stored
            .into_iter()
            .map(|(username, usage)| {
                (
                    username,
                    ClientEntry {
                        limit: None,
                        usage,
                    },
                )
            })
            .collect()
    }

    fn maybe_persist(&self) {
        let Some(path) = self.usage_file.as_ref() else {
            return;
        };

        let mut last_persist = self.last_persist.lock().unwrap();
        if last_persist.elapsed() < self.persist_interval {
            return;
        }
        *last_persist = Instant::now();
        drop(last_persist);

        let usage: HashMap<String, UsageRecord> = self
            .clients
            .lock()
            .unwrap()
            .iter()
            .map(|(username, entry)| (username.clone(), entry.usage.clone()))
            .collect();

        let content = match toml::to_string_pretty(&usage) {
            Ok(content) => content,
            Err(e) => {
                log::warn!("Couldn't serialize traffic usage: {}", e);
                return;
            }
        };

        if let Err(e) = std::fs::write(path, content) {
            log::warn!("Couldn't write traffic usage file {}: {}", path.display(), e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authentication::registry_based::Client;
    use tempfile::NamedTempFile;

    fn make_client(username: &str, limit: Option<u64>) -> Client {
        Client {
            username: username.into(),
            password: "pass".into(),
            max_http2_conns: None,
            max_http3_conns: None,
            max_traffic_bytes: limit,
        }
    }

    #[test]
    fn allows_traffic_under_limit() {
        let limiter = TrafficLimiter::new(&[make_client("alice", Some(100))], None, None);
        assert!(limiter.is_allowed("alice"));
        assert!(limiter.record("alice", TrafficDirection::Inbound, 40));
        assert!(limiter.record("alice", TrafficDirection::Outbound, 40));
        assert!(limiter.is_allowed("alice"));
    }

    #[test]
    fn blocks_traffic_over_limit() {
        let limiter = TrafficLimiter::new(&[make_client("alice", Some(100))], None, None);
        assert!(limiter.record("alice", TrafficDirection::Inbound, 60));
        assert!(limiter.record("alice", TrafficDirection::Outbound, 50));
        assert!(!limiter.is_allowed("alice"));
        assert!(limiter.summary("alice").quota_exceeded);
    }

    #[test]
    fn default_limit_applies_when_client_limit_missing() {
        let limiter = TrafficLimiter::new(
            &[Client {
                username: "bob".into(),
                password: "pass".into(),
                max_http2_conns: None,
                max_http3_conns: None,
                max_traffic_bytes: None,
            }],
            Some(10),
            None,
        );
        assert!(limiter.record("bob", TrafficDirection::Inbound, 10));
        assert!(!limiter.is_allowed("bob"));
    }

    #[test]
    fn persists_and_reloads_usage() {
        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();

        {
            let limiter = TrafficLimiter::new(&[make_client("alice", Some(1000))], None, Some(path.clone()));
            limiter.record("alice", TrafficDirection::Inbound, 123);
            limiter.record("alice", TrafficDirection::Outbound, 456);
            limiter.maybe_persist();
        }

        let reloaded = TrafficLimiter::new(&[make_client("alice", Some(1000))], None, Some(path));
        let summary = reloaded.summary("alice");
        assert_eq!(summary.inbound, 123);
        assert_eq!(summary.outbound, 456);
    }
}
