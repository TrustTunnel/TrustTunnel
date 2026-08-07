pub mod registry_based;

use crate::log_utils;
use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
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

impl Source<'_> {
    pub fn into_owned(self) -> Source<'static> {
        match self {
            Source::Sni(x) => Source::Sni(Cow::Owned(x.into_owned())),
            Source::ProxyBasic(x) => Source::ProxyBasic(Cow::Owned(x.into_owned())),
        }
    }
}

pub fn username_from_source(source: &Source<'_>) -> Option<String> {
    let creds = match source {
        Source::ProxyBasic(s) | Source::Sni(s) => s.as_ref(),
    };
    BASE64_ENGINE
        .decode(creds)
        .ok()
        .and_then(|v| String::from_utf8(v).ok())
        .and_then(|s| s.splitn(2, ':').next().map(str::to_string))
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

    #[test]
    fn username_from_credentials_matches_proxy_basic_encoding() {
        let encoded = BASE64_ENGINE.encode("alice:secret");
        let source = Source::ProxyBasic(encoded.into());
        assert_eq!(super::username_from_source(&source), Some("alice".into()));
    }

    #[test]
    fn username_from_sni_credentials() {
        let encoded = BASE64_ENGINE.encode("bob:secret");
        let source = Source::Sni(encoded.into());
        assert_eq!(super::username_from_source(&source), Some("bob".into()));
    }
}
