use crate::authentication::Authenticator;
use crate::{authentication, log_utils};
use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::HashSet;

/// A client descriptor
#[derive(Deserialize)]
pub struct Client {
    /// The client username
    pub username: String,
    /// The client password
    pub password: String,
    /// Maximum number of simultaneous HTTP/1 and HTTP/2 connections for this client.
    /// Overrides `default_max_http2_conns_per_client` from the main config.
    /// If absent, the global default applies (or unlimited if no default is set).
    pub max_http2_conns: Option<u32>,
    /// Maximum number of simultaneous HTTP/3 (QUIC) connections for this client.
    /// Overrides `default_max_http3_conns_per_client` from the main config.
    /// If absent, the global default applies (or unlimited if no default is set).
    pub max_http3_conns: Option<u32>,
    /// Maximum total traffic (inbound + outbound bytes) for this client.
    /// Overrides `default_max_traffic_bytes_per_client` from the main config.
    /// If absent, the global default applies (or unlimited if no default is set).
    pub max_traffic_bytes: Option<u64>,
}

/// The [`Authenticator`] implementation which checks presence of a client in the list.
/// Is only able to authenticate a client using the Proxy basic authorization.
pub struct RegistryBasedAuthenticator {
    clients: HashSet<Cow<'static, str>>,
}

impl RegistryBasedAuthenticator {
    pub fn new(clients: &[Client]) -> Self {
        Self {
            clients: clients
                .iter()
                .map(|x| BASE64_ENGINE.encode(format!("{}:{}", x.username, x.password)))
                .map(Cow::Owned)
                .collect(),
        }
    }
}

impl Authenticator for RegistryBasedAuthenticator {
    fn authenticate(
        &self,
        source: &authentication::Source<'_>,
        _log_id: &log_utils::IdChain<u64>,
    ) -> authentication::Status {
        let creds = match &source {
            authentication::Source::ProxyBasic(str) => str,
            authentication::Source::Sni(str) => str,
        };
        if self.clients.contains(creds.as_ref()) {
            authentication::Status::Pass
        } else {
            authentication::Status::Reject
        }
    }
}
