use crate::authentication::{
    AuthError, AuthProvider, Authenticator, ProxyBasicAuthenticator, Source, Status,
};
use crate::log_utils;
use crate::metrics;
use ring::digest::{digest, SHA256};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A client descriptor
#[derive(Deserialize, Clone)]
pub struct Client {
    /// The client username in LK.
    pub username: String,
    /// The client password hash (SHA-256 hex by default).
    pub password: String,
    /// Optional device-scoped login alias for Basic auth.
    #[serde(default)]
    pub device_user: Option<String>,
    /// Optional device id from LK.
    #[serde(default)]
    pub device_id: Option<String>,
}

pub struct CredentialsAuth {
    clients_by_username: HashMap<String, Client>,
    clients_by_device_user: HashMap<String, String>,
    basic_cache: Mutex<HashMap<String, CacheEntry>>,
    cache_ttl: Duration,
    revocation_interval: Duration,
    next_revocation_sync: Mutex<Instant>,
}

#[derive(Clone, Copy)]
struct CacheEntry {
    passed: bool,
    expires_at: Instant,
}

/// Backward-compatible wrapper for previous authenticator type.
pub struct RegistryBasedAuthenticator {
    inner: ProxyBasicAuthenticator,
}

impl RegistryBasedAuthenticator {
    pub fn new(clients: &[Client]) -> Self {
        Self {
            inner: ProxyBasicAuthenticator::new(Box::new(CredentialsAuth::new(clients))),
        }
    }
}

impl Authenticator for RegistryBasedAuthenticator {
    fn authenticate(&self, source: &Source<'_>, log_id: &log_utils::IdChain<u64>) -> Status {
        self.inner.authenticate(source, log_id)
    }
}

impl CredentialsAuth {
    pub fn new(clients: &[Client]) -> Self {
        let clients_by_username: HashMap<String, Client> = clients
            .iter()
            .cloned()
            .map(|x| (x.username.clone(), x))
            .collect();

        let clients_by_device_user = clients
            .iter()
            .filter_map(|x| {
                x.device_user
                    .as_ref()
                    .map(|device_user| (device_user.clone(), x.username.clone()))
            })
            .collect();

        Self {
            clients_by_username,
            clients_by_device_user,
            basic_cache: Mutex::new(HashMap::new()),
            cache_ttl: Duration::from_secs(5),
            revocation_interval: Duration::from_secs(15),
            next_revocation_sync: Mutex::new(Instant::now() + Duration::from_secs(15)),
        }
    }

    pub fn with_cache_tuning(mut self, cache_ttl: Duration, revocation_interval: Duration) -> Self {
        self.cache_ttl = cache_ttl;
        self.revocation_interval = revocation_interval;
        *self
            .next_revocation_sync
            .lock()
            .expect("cache lock poisoned") = Instant::now() + revocation_interval;
        self
    }

    pub fn find_client_by_login(&self, login: &str) -> Option<&Client> {
        self.clients_by_username.get(login).or_else(|| {
            self.clients_by_device_user
                .get(login)
                .and_then(|username| self.clients_by_username.get(username))
        })
    }

    fn verify_password(expected_password: &str, provided_password: &str) -> bool {
        let digest_hex = hex::encode(digest(&SHA256, provided_password.as_bytes()));

        if let Some(expected_digest) = expected_password.strip_prefix("sha256:") {
            return expected_digest.eq_ignore_ascii_case(&digest_hex);
        }

        if expected_password.len() == 64
            && expected_password.chars().all(|c| c.is_ascii_hexdigit())
            && expected_password.eq_ignore_ascii_case(&digest_hex)
        {
            return true;
        }

        // Backward compatibility for plaintext credentials.
        expected_password == provided_password
    }

    fn maybe_sync_revocation(&self) {
        if self.revocation_interval.is_zero() {
            return;
        }

        let now = Instant::now();
        let mut next_sync = self
            .next_revocation_sync
            .lock()
            .expect("cache lock poisoned");
        if now < *next_sync {
            return;
        }

        self.basic_cache
            .lock()
            .expect("cache lock poisoned")
            .clear();
        *next_sync = now + self.revocation_interval;
    }

    fn cache_key(username: &str, password: &str) -> String {
        let digest_hex = hex::encode(digest(
            &SHA256,
            format!("{username}\0{password}").as_bytes(),
        ));
        format!("basic:{digest_hex}")
    }
}

impl AuthProvider for CredentialsAuth {
    fn authenticate(&self, username: &str, password: &str) -> Result<(), AuthError> {
        self.maybe_sync_revocation();
        let cache_key = Self::cache_key(username, password);
        let now = Instant::now();
        if let Some(cached) = self
            .basic_cache
            .lock()
            .expect("cache lock poisoned")
            .get(&cache_key)
            .copied()
            .filter(|entry| entry.expires_at >= now)
        {
            if cached.passed {
                metrics::add_auth_basic_success();
                return Ok(());
            }

            metrics::add_auth_basic_failure();
            return Err(AuthError::InvalidCredentials);
        }

        let client = self.find_client_by_login(username).ok_or_else(|| {
            metrics::add_auth_basic_failure();
            AuthError::InvalidCredentials
        })?;

        if Self::verify_password(&client.password, password) {
            self.basic_cache
                .lock()
                .expect("cache lock poisoned")
                .insert(
                    cache_key,
                    CacheEntry {
                        passed: true,
                        expires_at: now + self.cache_ttl,
                    },
                );
            metrics::add_auth_basic_success();
            return Ok(());
        }

        self.basic_cache
            .lock()
            .expect("cache lock poisoned")
            .insert(
                cache_key,
                CacheEntry {
                    passed: false,
                    expires_at: now + self.cache_ttl,
                },
            );

        metrics::add_auth_basic_failure();
        Err(AuthError::InvalidCredentials)
    }
}
