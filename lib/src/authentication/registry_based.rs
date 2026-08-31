use crate::authentication::Authenticator;
use crate::{authentication, log_utils};
use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
use serde::Deserialize;
use std::collections::HashMap;

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
}

/// The [`Authenticator`] implementation which checks presence of a client in the list.
/// Is only able to authenticate a client using the Proxy basic authorization.
pub struct RegistryBasedAuthenticator {
    /// Maps the base64-encoded `username:password` credentials to the username.
    clients: HashMap<String, String>,
}

impl RegistryBasedAuthenticator {
    pub fn new(clients: &[Client]) -> Self {
        Self {
            clients: clients
                .iter()
                .map(|x| {
                    (
                        BASE64_ENGINE.encode(format!("{}:{}", x.username, x.password)),
                        x.username.clone(),
                    )
                })
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
        if self.clients.contains_key(creds.as_ref()) {
            authentication::Status::Pass
        } else {
            authentication::Status::Reject
        }
    }

    fn username(&self, source: &authentication::Source<'_>) -> Option<String> {
        let creds = match &source {
            authentication::Source::ProxyBasic(str) => str,
            authentication::Source::Sni(str) => str,
        };
        self.clients.get(creds.as_ref()).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authentication::Status;
    use crate::log_utils::{IdChain, IdItem};

    fn make_client(username: &str, password: &str) -> Client {
        Client {
            username: username.into(),
            password: password.into(),
            max_http2_conns: None,
            max_http3_conns: None,
        }
    }

    fn creds(username: &str, password: &str) -> String {
        BASE64_ENGINE.encode(format!("{}:{}", username, password))
    }

    fn log_id() -> IdChain<u64> {
        IdChain::from(IdItem::new("TEST={}", 0u64))
    }

    #[test]
    fn resolves_username_from_credentials() {
        let auth = RegistryBasedAuthenticator::new(&[make_client("alice", "secret")]);

        let proxy = authentication::Source::ProxyBasic(creds("alice", "secret").into());
        assert_eq!(auth.username(&proxy).as_deref(), Some("alice"));

        let sni = authentication::Source::Sni(creds("alice", "secret").into());
        assert_eq!(auth.username(&sni).as_deref(), Some("alice"));
    }

    #[test]
    fn unknown_credentials_have_no_username() {
        let auth = RegistryBasedAuthenticator::new(&[make_client("alice", "secret")]);
        let unknown = authentication::Source::ProxyBasic(creds("bob", "secret").into());
        assert_eq!(auth.username(&unknown), None);
    }

    #[test]
    fn authenticate_still_works_with_map_backing() {
        let auth = RegistryBasedAuthenticator::new(&[make_client("alice", "secret")]);
        let ok = authentication::Source::ProxyBasic(creds("alice", "secret").into());
        let bad = authentication::Source::ProxyBasic(creds("alice", "wrong").into());
        assert!(auth.authenticate(&ok, &log_id()) == Status::Pass);
        assert!(auth.authenticate(&bad, &log_id()) == Status::Reject);
    }
}
