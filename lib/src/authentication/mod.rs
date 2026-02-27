pub mod registry_based;

use crate::log_utils;
use std::borrow::Cow;

/// Authentication request source
#[derive(Clone, PartialEq)]
pub enum Source<'this> {
    /// A client tries to authenticate using SNI
    Sni(Cow<'this, str>),
    /// A client tries to authenticate using
    /// [the basic authentication scheme](https://datatracker.ietf.org/doc/html/rfc7617)
    ProxyBasic(Cow<'this, str>),
}

impl std::fmt::Debug for Source<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Sni(_) => write!(f, "Sni(__stripped__)"),
            Source::ProxyBasic(_) => write!(f, "ProxyBasic(__stripped__)"),
        }
    }
}

/// Authentication procedure status
#[derive(Clone, PartialEq)]
pub enum Status {
    /// Success
    Pass,
    /// Failure
    Reject,
}

/// The authenticator abstract interface
pub trait Authenticator: Send + Sync {
    /// Authenticate client
    fn authenticate(&self, source: &Source<'_>, log_id: &log_utils::IdChain<u64>) -> Status;
}

pub fn tunnel_token_from_credentials(username: &str, password: &str) -> String {
    let data = format!("{username}:{password}");
    let digest = ring::digest::digest(&ring::digest::SHA256, data.as_bytes());
    hex::encode(digest.as_ref())
}

impl Source<'_> {
    pub fn into_owned(self) -> Source<'static> {
        match self {
            Source::Sni(x) => Source::Sni(Cow::Owned(x.into_owned())),
            Source::ProxyBasic(x) => Source::ProxyBasic(Cow::Owned(x.into_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_debug_scrubs_sni() {
        let source = Source::Sni("secret_credentials".into());
        let debug_output = format!("{:?}", source);
        assert!(!debug_output.contains("secret_credentials"));
        assert!(debug_output.contains("__stripped__"));
    }

    #[test]
    fn source_debug_scrubs_proxy_basic() {
        let source = Source::ProxyBasic("dXNlcjpwYXNzd29yZA==".into());
        let debug_output = format!("{:?}", source);
        assert!(!debug_output.contains("dXNlcjpwYXNzd29yZA=="));
        assert!(debug_output.contains("__stripped__"));
    }
}
