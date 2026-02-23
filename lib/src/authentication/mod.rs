pub mod credentials;
pub mod jwt;
pub mod mixed;
pub mod registry_based;

use crate::log_id;
use crate::log_utils;
use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
use std::borrow::Cow;

/// Authentication request source
#[derive(Debug, Clone, PartialEq)]
pub enum Source<'this> {
    /// A client tries to authenticate using SNI
    Sni(Cow<'this, str>),
    /// A client tries to authenticate using
    /// [the basic authentication scheme](https://datatracker.ietf.org/doc/html/rfc7617)
    ProxyBasic(Cow<'this, str>),
    /// A client tries to authenticate using bearer token in Authorization header.
    ProxyBearer(Cow<'this, str>),
    /// A client provided both Bearer and Basic credentials.
    ProxyBearerAndBasic {
        bearer: Cow<'this, str>,
        basic: Cow<'this, str>,
    },
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

#[derive(Debug, Clone)]
pub enum AuthError {
    InvalidCredentials,
    InvalidToken,
    InvalidAuthHeader,
    Internal,
}

pub trait AuthProvider: Send + Sync {
    fn authenticate(&self, username: &str, password: &str) -> Result<(), AuthError>;
}

pub struct ProxyBasicAuthenticator {
    provider: Box<dyn AuthProvider>,
}

impl ProxyBasicAuthenticator {
    pub fn new(provider: Box<dyn AuthProvider>) -> Self {
        Self { provider }
    }
}

impl Authenticator for ProxyBasicAuthenticator {
    fn authenticate(&self, source: &Source<'_>, log_id: &log_utils::IdChain<u64>) -> Status {
        let basic = match source {
            Source::ProxyBasic(value) => value,
            _ => return Status::Reject,
        };

        let decoded = match BASE64_ENGINE.decode(basic.as_ref()) {
            Ok(value) => value,
            Err(_) => {
                crate::metrics::add_auth_basic_failure();
                return Status::Reject;
            }
        };

        let credentials = match String::from_utf8(decoded) {
            Ok(value) => value,
            Err(_) => {
                crate::metrics::add_auth_basic_failure();
                return Status::Reject;
            }
        };

        let mut split = credentials.splitn(2, ':');
        let username = match split.next() {
            Some(value) if !value.is_empty() => value,
            _ => {
                crate::metrics::add_auth_basic_failure();
                return Status::Reject;
            }
        };
        let password = match split.next() {
            Some(value) => value,
            None => {
                crate::metrics::add_auth_basic_failure();
                return Status::Reject;
            }
        };

        match self.provider.authenticate(username, password) {
            Ok(()) => Status::Pass,
            Err(err) => {
                log_id!(debug, log_id, "Authentication rejected: {:?}", err);
                Status::Reject
            }
        }
    }
}

impl Source<'_> {
    pub fn into_owned(self) -> Source<'static> {
        match self {
            Source::Sni(x) => Source::Sni(Cow::Owned(x.into_owned())),
            Source::ProxyBasic(x) => Source::ProxyBasic(Cow::Owned(x.into_owned())),
            Source::ProxyBearer(x) => Source::ProxyBearer(Cow::Owned(x.into_owned())),
            Source::ProxyBearerAndBasic { bearer, basic } => Source::ProxyBearerAndBasic {
                bearer: Cow::Owned(bearer.into_owned()),
                basic: Cow::Owned(basic.into_owned()),
            },
        }
    }
}
