use crate::authentication::{
    AuthError, AuthProvider, Authenticator, ProxyBasicAuthenticator, Source, Status,
};
use crate::log_utils;
use crate::metrics;
use ring::digest::{digest, SHA256};
use serde::Deserialize;
use std::collections::HashMap;

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
        }
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
}

impl AuthProvider for CredentialsAuth {
    fn authenticate(&self, username: &str, password: &str) -> Result<(), AuthError> {
        let client = self.find_client_by_login(username).ok_or_else(|| {
            metrics::add_auth_basic_failure();
            AuthError::InvalidCredentials
        })?;

        if Self::verify_password(&client.password, password) {
            metrics::add_auth_basic_success();
            return Ok(());
        }

        metrics::add_auth_basic_failure();
        Err(AuthError::InvalidCredentials)
    }
}
