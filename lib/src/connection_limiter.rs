use crate::authentication::registry_based::Client;
use crate::tls_demultiplexer::Protocol;
use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

struct ClientEntry {
    max_http2: Option<u32>,
    max_http3: Option<u32>,
    http2_count: u32,
    http3_count: u32,
}

/// Tracks active connections per client credentials and enforces per-client limits.
pub(crate) struct ConnectionLimiter {
    clients: Mutex<HashMap<String, ClientEntry>>,
    default_max_http2: Option<u32>,
    default_max_http3: Option<u32>,
}

/// RAII guard that decrements the connection count when dropped.
pub(crate) struct ConnectionGuard {
    limiter: Arc<ConnectionLimiter>,
    creds: String,
    protocol: Protocol,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.limiter.release(&self.creds, self.protocol);
    }
}

impl ConnectionLimiter {
    /// Creates a new ConnectionLimiter.
    ///
    /// Note: The limiter captures the client list at initialization time and does not
    /// automatically update if clients are modified at runtime. A server restart is
    /// required to reflect changes in client credentials or limits.
    pub fn new(
        clients: &[Client],
        default_max_http2: Option<u32>,
        default_max_http3: Option<u32>,
    ) -> Self {
        let map = clients
            .iter()
            .map(|c| {
                let key = BASE64_ENGINE.encode(format!("{}:{}", c.username, c.password));
                (
                    key,
                    ClientEntry {
                        max_http2: c.max_http2_conns,
                        max_http3: c.max_http3_conns,
                        http2_count: 0,
                        http3_count: 0,
                    },
                )
            })
            .collect();

        Self {
            clients: Mutex::new(map),
            default_max_http2,
            default_max_http3,
        }
    }

    /// Try to acquire a connection slot for the given credentials and protocol.
    ///
    /// Returns `Some(guard)` on success — the guard releases the slot on drop.
    /// Returns `None` if the per-client limit is exceeded.
    /// Returns `None` for unknown credentials (should not happen after authentication).
    pub fn try_acquire(
        self: &Arc<Self>,
        creds: &str,
        protocol: Protocol,
    ) -> Option<ConnectionGuard> {
        let mut clients = self.clients.lock().unwrap();

        let entry = clients.get_mut(creds)?;

        let (current, limit) = match protocol {
            Protocol::Http1 | Protocol::Http2 => {
                let limit = entry.max_http2.or(self.default_max_http2);
                (&mut entry.http2_count, limit)
            }
            Protocol::Http3 => {
                let limit = entry.max_http3.or(self.default_max_http3);
                (&mut entry.http3_count, limit)
            }
        };

        if let Some(max) = limit {
            if *current >= max {
                return None;
            }
        }

        *current += 1;
        Some(ConnectionGuard {
            limiter: self.clone(),
            creds: creds.to_owned(),
            protocol,
        })
    }

    fn release(&self, creds: &str, protocol: Protocol) {
        let mut clients = self.clients.lock().unwrap();
        if let Some(entry) = clients.get_mut(creds) {
            match protocol {
                Protocol::Http1 | Protocol::Http2 => {
                    entry.http2_count = entry.http2_count.saturating_sub(1);
                }
                Protocol::Http3 => {
                    entry.http3_count = entry.http3_count.saturating_sub(1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authentication::registry_based::Client;

    fn make_client(username: &str, password: &str) -> Client {
        Client {
            username: username.into(),
            password: password.into(),
            max_http2_conns: None,
            max_http3_conns: None,
            max_traffic_bytes: None,
        }
    }

    fn make_client_with_limits(
        username: &str,
        password: &str,
        h2: Option<u32>,
        h3: Option<u32>,
    ) -> Client {
        Client {
            username: username.into(),
            password: password.into(),
            max_http2_conns: h2,
            max_http3_conns: h3,
            max_traffic_bytes: None,
        }
    }

    fn creds(username: &str, password: &str) -> String {
        BASE64_ENGINE.encode(format!("{}:{}", username, password))
    }

    #[test]
    fn no_limits_always_passes() {
        let limiter = Arc::new(ConnectionLimiter::new(&[make_client("u", "p")], None, None));
        let key = creds("u", "p");
        let g1 = limiter.try_acquire(&key, Protocol::Http2).unwrap();
        let g2 = limiter.try_acquire(&key, Protocol::Http2).unwrap();
        let g3 = limiter.try_acquire(&key, Protocol::Http3).unwrap();
        drop((g1, g2, g3));
    }

    #[test]
    fn global_http2_limit_enforced() {
        let limiter = Arc::new(ConnectionLimiter::new(
            &[make_client("u", "p")],
            Some(2),
            None,
        ));
        let key = creds("u", "p");

        let g1 = limiter.try_acquire(&key, Protocol::Http2).unwrap();
        let g2 = limiter.try_acquire(&key, Protocol::Http2).unwrap();
        assert!(
            limiter.try_acquire(&key, Protocol::Http2).is_none(),
            "must be denied at limit=2"
        );

        drop(g1);
        let g3 = limiter.try_acquire(&key, Protocol::Http2).unwrap();
        drop((g2, g3));
    }

    #[test]
    fn global_http3_limit_enforced() {
        let limiter = Arc::new(ConnectionLimiter::new(
            &[make_client("u", "p")],
            None,
            Some(1),
        ));
        let key = creds("u", "p");

        let g1 = limiter.try_acquire(&key, Protocol::Http3).unwrap();
        assert!(
            limiter.try_acquire(&key, Protocol::Http3).is_none(),
            "must be denied at limit=1"
        );

        drop(g1);
        limiter.try_acquire(&key, Protocol::Http3).unwrap();
    }

    #[test]
    fn http2_and_http3_counters_are_independent() {
        let limiter = Arc::new(ConnectionLimiter::new(
            &[make_client("u", "p")],
            Some(1),
            Some(1),
        ));
        let key = creds("u", "p");

        let _g2 = limiter.try_acquire(&key, Protocol::Http2).unwrap();
        let _g3 = limiter.try_acquire(&key, Protocol::Http3).unwrap();
        assert!(
            limiter.try_acquire(&key, Protocol::Http2).is_none(),
            "http2 must be at limit"
        );
        assert!(
            limiter.try_acquire(&key, Protocol::Http3).is_none(),
            "http3 must be at limit"
        );
    }

    #[test]
    fn per_client_override_takes_precedence_over_global() {
        let clients = vec![
            make_client_with_limits("alice", "pass", Some(5), None),
            make_client("bob", "pass"),
        ];
        let limiter = Arc::new(ConnectionLimiter::new(&clients, Some(1), None));

        let alice = creds("alice", "pass");
        let bob = creds("bob", "pass");

        let _a1 = limiter.try_acquire(&alice, Protocol::Http2).unwrap();
        let _a2 = limiter.try_acquire(&alice, Protocol::Http2).unwrap();
        let _a3 = limiter.try_acquire(&alice, Protocol::Http2).unwrap();
        let _a4 = limiter.try_acquire(&alice, Protocol::Http2).unwrap();
        let _a5 = limiter.try_acquire(&alice, Protocol::Http2).unwrap();
        assert!(
            limiter.try_acquire(&alice, Protocol::Http2).is_none(),
            "alice: must be denied at override limit=5"
        );

        let _b1 = limiter.try_acquire(&bob, Protocol::Http2).unwrap();
        assert!(
            limiter.try_acquire(&bob, Protocol::Http2).is_none(),
            "bob: must be denied at global limit=1"
        );
    }

    #[test]
    fn limits_are_per_client_not_shared() {
        let clients = vec![make_client("alice", "pass"), make_client("bob", "pass")];
        let limiter = Arc::new(ConnectionLimiter::new(&clients, Some(1), None));

        let alice = creds("alice", "pass");
        let bob = creds("bob", "pass");

        let _ga = limiter.try_acquire(&alice, Protocol::Http2).unwrap();
        let _gb = limiter.try_acquire(&bob, Protocol::Http2).unwrap();
        assert!(
            limiter.try_acquire(&alice, Protocol::Http2).is_none(),
            "alice at limit"
        );
        assert!(
            limiter.try_acquire(&bob, Protocol::Http2).is_none(),
            "bob at limit"
        );
    }

    #[test]
    fn unknown_credentials_denied() {
        let limiter = Arc::new(ConnectionLimiter::new(&[make_client("u", "p")], None, None));
        assert!(limiter
            .try_acquire("unknown_creds", Protocol::Http2)
            .is_none());
    }
}
