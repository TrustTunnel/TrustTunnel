use crate::metrics::Metrics;
use crate::shutdown::Shutdown;
use crate::tls_demultiplexer::Protocol;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(crate) struct SessionGuard {
    enabled: bool,
    max_active_sessions_per_user: usize,
    stale_ttl: Duration,
    cleanup_interval: Duration,
    registry: Arc<Mutex<SessionRegistry>>,
    metrics: Arc<Metrics>,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionEntry {
    pub session_id: String,
    pub username: String,
    pub remote_addr: IpAddr,
    pub protocol: Protocol,
    pub started_at: Instant,
    pub last_seen_at: Instant,
}

#[derive(Default)]
pub(crate) struct SessionRegistry {
    users: HashMap<String, HashMap<String, SessionEntry>>,
}

pub(crate) struct SessionHandle {
    session_id: String,
    username: String,
    guard: Weak<SessionGuard>,
}

#[derive(Debug)]
pub(crate) enum AcquireError {
    MaxSessionsExceeded { current_active_sessions: usize },
}

impl Display for AcquireError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MaxSessionsExceeded {
                current_active_sessions,
            } => {
                write!(
                    f,
                    "max sessions exceeded, current_active_sessions={}",
                    current_active_sessions
                )
            }
        }
    }
}

impl SessionGuard {
    pub(crate) fn new(
        enabled: bool,
        max_active_sessions_per_user: usize,
        stale_ttl: Duration,
        cleanup_interval: Duration,
        metrics: Arc<Metrics>,
    ) -> Arc<Self> {
        Arc::new(Self {
            enabled,
            max_active_sessions_per_user,
            stale_ttl,
            cleanup_interval,
            registry: Default::default(),
            metrics,
        })
    }

    pub(crate) fn spawn_cleanup_task(self: &Arc<Self>, shutdown: Arc<Mutex<Shutdown>>) {
        if !self.enabled {
            return;
        }

        let guard = Arc::clone(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(guard.cleanup_interval);
            let (mut shutdown_notification, _shutdown_completion) = {
                let shutdown = shutdown.lock().unwrap();
                (shutdown.notification_handler(), shutdown.completion_guard())
            };

            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        guard.reap_stale();
                    }
                    _ = shutdown_notification.wait() => {
                        break;
                    }
                }
            }
        });
    }

    pub(crate) fn try_acquire(
        self: &Arc<Self>,
        session_id: String,
        username: String,
        remote_addr: IpAddr,
        protocol: Protocol,
    ) -> Result<SessionHandle, AcquireError> {
        if !self.enabled {
            return Ok(SessionHandle {
                session_id,
                username,
                guard: Arc::downgrade(self),
            });
        }

        let mut registry = self.registry.lock().unwrap();
        let sessions = registry.users.entry(username.clone()).or_default();
        if sessions.len() >= self.max_active_sessions_per_user {
            self.metrics.inc_session_guard_rejections_total();
            return Err(AcquireError::MaxSessionsExceeded {
                current_active_sessions: sessions.len(),
            });
        }

        let now = Instant::now();
        sessions.insert(
            session_id.clone(),
            SessionEntry {
                session_id: session_id.clone(),
                username: username.clone(),
                remote_addr,
                protocol,
                started_at: now,
                last_seen_at: now,
            },
        );

        self.refresh_metrics(&registry);

        Ok(SessionHandle {
            session_id,
            username,
            guard: Arc::downgrade(self),
        })
    }

    pub(crate) fn touch(&self, username: &str, session_id: &str) {
        if !self.enabled {
            return;
        }

        let mut registry = self.registry.lock().unwrap();
        if let Some(user_sessions) = registry.users.get_mut(username) {
            if let Some(session) = user_sessions.get_mut(session_id) {
                session.last_seen_at = Instant::now();
            }
        }
    }

    pub(crate) fn release(&self, username: &str, session_id: &str) {
        if !self.enabled {
            return;
        }

        let mut registry = self.registry.lock().unwrap();
        if let Some(user_sessions) = registry.users.get_mut(username) {
            user_sessions.remove(session_id);
            if user_sessions.is_empty() {
                registry.users.remove(username);
            }
        }

        self.refresh_metrics(&registry);
    }

    pub(crate) fn active_sessions_for_user(&self, username: &str) -> usize {
        let registry = self.registry.lock().unwrap();
        registry.users.get(username).map_or(0, HashMap::len)
    }

    pub(crate) fn total_sessions(&self) -> usize {
        let registry = self.registry.lock().unwrap();
        registry.users.values().map(HashMap::len).sum()
    }

    fn reap_stale(&self) {
        if !self.enabled {
            return;
        }

        let now = Instant::now();
        let mut reaped = 0u64;
        let mut registry = self.registry.lock().unwrap();
        registry.users.retain(|_, sessions| {
            sessions.retain(|_, session| {
                let stale = now.duration_since(session.last_seen_at) >= self.stale_ttl;
                if stale {
                    reaped += 1;
                }
                !stale
            });
            !sessions.is_empty()
        });

        if reaped > 0 {
            self.metrics.inc_session_guard_stale_reaped_total_by(reaped);
        }

        self.refresh_metrics(&registry);
    }

    fn refresh_metrics(&self, registry: &SessionRegistry) {
        let total: i64 = registry.users.values().map(HashMap::len).sum::<usize>() as i64;
        let users_at_limit = registry
            .users
            .values()
            .filter(|sessions| sessions.len() >= self.max_active_sessions_per_user)
            .count() as i64;
        let registry_size = registry.users.len() as i64;

        self.metrics
            .set_session_guard_active_sessions_total(total.max(0));
        self.metrics
            .set_session_guard_users_at_limit(users_at_limit.max(0));
        self.metrics
            .set_session_guard_registry_size(registry_size.max(0));
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        if let Some(guard) = self.guard.upgrade() {
            guard.release(&self.username, &self.session_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SessionGuard;
    use crate::metrics::Metrics;
    use crate::tls_demultiplexer::Protocol;
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;

    fn test_guard(enabled: bool) -> std::sync::Arc<SessionGuard> {
        SessionGuard::new(
            enabled,
            3,
            Duration::from_millis(20),
            Duration::from_millis(5),
            Metrics::new().unwrap(),
        )
    }

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, n))
    }

    #[test]
    fn single_user_up_to_three_sessions() {
        let guard = test_guard(true);
        let _s1 = guard
            .try_acquire("s1".into(), "alice".into(), ip(1), Protocol::Http2)
            .unwrap();
        let _s2 = guard
            .try_acquire("s2".into(), "alice".into(), ip(2), Protocol::Http2)
            .unwrap();
        let _s3 = guard
            .try_acquire("s3".into(), "alice".into(), ip(3), Protocol::Http3)
            .unwrap();

        assert_eq!(guard.active_sessions_for_user("alice"), 3);
    }

    #[test]
    fn fourth_session_is_rejected() {
        let guard = test_guard(true);
        let _s1 = guard
            .try_acquire("s1".into(), "alice".into(), ip(1), Protocol::Http2)
            .unwrap();
        let _s2 = guard
            .try_acquire("s2".into(), "alice".into(), ip(2), Protocol::Http2)
            .unwrap();
        let _s3 = guard
            .try_acquire("s3".into(), "alice".into(), ip(3), Protocol::Http2)
            .unwrap();

        assert!(guard
            .try_acquire("s4".into(), "alice".into(), ip(4), Protocol::Http2)
            .is_err());
    }

    #[test]
    fn another_user_not_affected() {
        let guard = test_guard(true);
        let _a1 = guard
            .try_acquire("a1".into(), "alice".into(), ip(1), Protocol::Http2)
            .unwrap();
        let _a2 = guard
            .try_acquire("a2".into(), "alice".into(), ip(2), Protocol::Http2)
            .unwrap();
        let _a3 = guard
            .try_acquire("a3".into(), "alice".into(), ip(3), Protocol::Http2)
            .unwrap();

        let _b1 = guard
            .try_acquire("b1".into(), "bob".into(), ip(4), Protocol::Http3)
            .unwrap();

        assert_eq!(guard.active_sessions_for_user("bob"), 1);
    }

    #[test]
    fn release_frees_slot() {
        let guard = test_guard(true);
        let s1 = guard
            .try_acquire("s1".into(), "alice".into(), ip(1), Protocol::Http2)
            .unwrap();
        let _s2 = guard
            .try_acquire("s2".into(), "alice".into(), ip(2), Protocol::Http2)
            .unwrap();
        let _s3 = guard
            .try_acquire("s3".into(), "alice".into(), ip(3), Protocol::Http2)
            .unwrap();

        drop(s1);

        let _s4 = guard
            .try_acquire("s4".into(), "alice".into(), ip(4), Protocol::Http2)
            .unwrap();
        assert_eq!(guard.active_sessions_for_user("alice"), 3);
    }

    #[tokio::test]
    async fn stale_session_reaped_by_cleaner() {
        let guard = test_guard(true);
        let _s1 = guard
            .try_acquire("s1".into(), "alice".into(), ip(1), Protocol::Http2)
            .unwrap();

        tokio::time::sleep(Duration::from_millis(40)).await;
        guard.reap_stale();

        assert_eq!(guard.active_sessions_for_user("alice"), 0);
    }

    #[test]
    fn disabled_guard_does_nothing() {
        let guard = test_guard(false);
        let mut handles = Vec::new();
        for idx in 0..10 {
            handles.push(
                guard
                    .try_acquire(format!("s{idx}"), "alice".into(), ip(1), Protocol::Http2)
                    .unwrap(),
            );
        }
        assert_eq!(handles.len(), 10);
    }
}
