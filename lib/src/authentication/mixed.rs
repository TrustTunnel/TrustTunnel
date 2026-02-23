use crate::authentication::jwt::JwtAuth;
use crate::authentication::registry_based::CredentialsAuth;
use crate::authentication::{AuthProvider, Authenticator, Source, Status};
use crate::log_id;
use crate::log_utils;
use crate::metrics;
use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;

pub struct MixedAuth {
    jwt: JwtAuth,
    credentials: CredentialsAuth,
}

impl MixedAuth {
    pub fn new(jwt: JwtAuth, credentials: CredentialsAuth) -> Self {
        Self { jwt, credentials }
    }

    fn authenticate_basic(&self, basic: &str) -> Status {
        let decoded = match BASE64_ENGINE.decode(basic) {
            Ok(value) => value,
            Err(_) => {
                metrics::add_auth_basic_failure();
                return Status::Reject;
            }
        };

        let credentials = match String::from_utf8(decoded) {
            Ok(value) => value,
            Err(_) => {
                metrics::add_auth_basic_failure();
                return Status::Reject;
            }
        };

        let mut split = credentials.splitn(2, ':');
        let username = match split.next() {
            Some(value) if !value.is_empty() => value,
            _ => {
                metrics::add_auth_basic_failure();
                return Status::Reject;
            }
        };
        let password = match split.next() {
            Some(value) => value,
            None => {
                metrics::add_auth_basic_failure();
                return Status::Reject;
            }
        };

        match self.credentials.authenticate(username, password) {
            Ok(()) => Status::Pass,
            Err(_) => Status::Reject,
        }
    }

    fn authenticate_bearer(&self, token: &str) -> Status {
        match self
            .jwt
            .authenticate_with_registry(token, &self.credentials)
        {
            Ok(()) => Status::Pass,
            Err(_) => Status::Reject,
        }
    }
}

impl Authenticator for MixedAuth {
    fn authenticate(&self, source: &Source<'_>, log_id: &log_utils::IdChain<u64>) -> Status {
        match source {
            Source::ProxyBearer(token) => {
                let jwt_result = self.authenticate_bearer(token);
                if matches!(jwt_result, Status::Pass) {
                    return Status::Pass;
                }

                log_id!(debug, log_id, "Bearer authentication failed");
                Status::Reject
            }
            Source::ProxyBasic(value) => self.authenticate_basic(value),
            Source::ProxyBearerAndBasic { bearer, basic } => {
                let jwt_result = self.authenticate_bearer(bearer);
                if matches!(jwt_result, Status::Pass) {
                    return Status::Pass;
                }
                log_id!(
                    debug,
                    log_id,
                    "Bearer authentication failed, trying Basic fallback"
                );
                self.authenticate_basic(basic)
            }
            _ => Status::Reject,
        }
    }
}
