use crate::authentication::registry_based::CredentialsAuth;
use crate::authentication::{AuthError, AuthProvider};
use crate::metrics;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use boring::hash::MessageDigest;
use boring::pkey::PKey;
use boring::sign::Verifier;
use ring::hmac;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum JwtAlgorithm {
    RS256,
    HS256,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JwtAuthConfig {
    pub algorithm: JwtAlgorithm,
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub leeway_seconds: u64,
    pub username_claim: String,
    pub device_id_claim: Option<String>,
    pub metrics_jwt_error_enabled: bool,
    pub public_key_path: Option<String>,
    pub hmac_secret_env: Option<String>,
}

enum JwtVerifier {
    Rs256(Vec<u8>),
    Hs256(Vec<u8>),
}

pub struct JwtAuth {
    verifier: JwtVerifier,
    issuer: Option<String>,
    audience: Option<String>,
    leeway_seconds: u64,
    username_claim: String,
    device_id_claim: Option<String>,
    metrics_jwt_error_enabled: bool,
    jwt_cache: Mutex<HashMap<String, JwtCacheEntry>>,
    cache_ttl: Duration,
    revocation_interval: Duration,
    next_revocation_sync: Mutex<Instant>,
}

#[derive(Clone)]
struct JwtCacheEntry {
    result: Result<JwtClaims, AuthError>,
    expires_at: Instant,
}

#[derive(Clone)]
pub struct JwtClaims {
    pub username: String,
    pub device_id: Option<String>,
}

impl JwtAuth {
    pub fn from_config(config: &JwtAuthConfig) -> Result<Self, AuthError> {
        let verifier = match config.algorithm {
            JwtAlgorithm::RS256 => {
                let path = config.public_key_path.as_ref().ok_or(AuthError::Internal)?;
                let pem = std::fs::read(path).map_err(|_| AuthError::Internal)?;
                JwtVerifier::Rs256(pem)
            }
            JwtAlgorithm::HS256 => {
                let env_name = config.hmac_secret_env.as_ref().ok_or(AuthError::Internal)?;
                let secret = std::env::var(env_name).map_err(|_| AuthError::Internal)?;
                JwtVerifier::Hs256(secret.into_bytes())
            }
        };

        Ok(Self {
            verifier,
            issuer: config.issuer.clone(),
            audience: config.audience.clone(),
            leeway_seconds: config.leeway_seconds,
            username_claim: config.username_claim.clone(),
            device_id_claim: config.device_id_claim.clone(),
            metrics_jwt_error_enabled: config.metrics_jwt_error_enabled,
            jwt_cache: Mutex::new(HashMap::new()),
            cache_ttl: Duration::from_secs(5),
            revocation_interval: Duration::from_secs(15),
            next_revocation_sync: Mutex::new(Instant::now() + Duration::from_secs(15)),
        })
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
}

impl AuthProvider for JwtAuth {
    fn authenticate(&self, username: &str, token: &str) -> Result<(), AuthError> {
        let claims = match self.validate_token(token) {
            Ok(claims) => claims,
            Err(error) => {
                metrics::add_auth_jwt_failure();
                return Err(error);
            }
        };
        if claims.username != username {
            metrics::add_auth_jwt_failure();
            return Err(self.observe_jwt_error(AuthError::InvalidToken));
        }
        metrics::add_auth_jwt_success();
        Ok(())
    }
}

impl JwtAuth {
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

        self.jwt_cache.lock().expect("cache lock poisoned").clear();
        *next_sync = now + self.revocation_interval;
    }

    fn token_cache_key(token: &str) -> String {
        hex::encode(ring::digest::digest(
            &ring::digest::SHA256,
            token.as_bytes(),
        ))
    }

    pub fn validate_token(&self, token: &str) -> Result<JwtClaims, AuthError> {
        self.maybe_sync_revocation();
        let cache_key = Self::token_cache_key(token);
        let cache_now = Instant::now();
        if let Some(entry) = self
            .jwt_cache
            .lock()
            .expect("cache lock poisoned")
            .get(&cache_key)
            .cloned()
            .filter(|entry| entry.expires_at >= cache_now)
        {
            return entry.result;
        }

        let (header_b64, payload_b64, signature_b64) =
            split_token(token).map_err(|e| self.observe_jwt_error(e))?;
        let header = URL_SAFE_NO_PAD
            .decode(header_b64)
            .map_err(|_| self.observe_jwt_error(AuthError::InvalidToken))?;
        let payload = URL_SAFE_NO_PAD
            .decode(payload_b64)
            .map_err(|_| self.observe_jwt_error(AuthError::InvalidToken))?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature_b64)
            .map_err(|_| self.observe_jwt_error(AuthError::InvalidToken))?;

        let header_str = std::str::from_utf8(&header)
            .map_err(|_| self.observe_jwt_error(AuthError::InvalidToken))?;
        let payload_str = std::str::from_utf8(&payload)
            .map_err(|_| self.observe_jwt_error(AuthError::InvalidToken))?;

        verify_algorithm(header_str, &self.verifier).map_err(|e| self.observe_jwt_error(e))?;

        let signing_input = format!("{}.{}", header_b64, payload_b64);
        match &self.verifier {
            JwtVerifier::Rs256(public_key_der) => {
                let key = PKey::public_key_from_pem(public_key_der)
                    .map_err(|_| self.observe_jwt_error(AuthError::InvalidToken))?;
                let mut verifier = Verifier::new(MessageDigest::sha256(), &key)
                    .map_err(|_| self.observe_jwt_error(AuthError::InvalidToken))?;
                verifier
                    .update(signing_input.as_bytes())
                    .map_err(|_| self.observe_jwt_error(AuthError::InvalidToken))?;
                let valid = verifier
                    .verify(&signature)
                    .map_err(|_| self.observe_jwt_error(AuthError::InvalidToken))?;
                if !valid {
                    return Err(self.observe_jwt_error(AuthError::InvalidToken));
                }
            }
            JwtVerifier::Hs256(secret) => {
                let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
                hmac::verify(&key, signing_input.as_bytes(), &signature)
                    .map_err(|_| self.observe_jwt_error(AuthError::InvalidToken))?;
            }
        }

        let exp = parse_u64_claim(payload_str, "exp").ok_or_else(|| {
            metrics::add_auth_jwt_failure();
            self.observe_jwt_error(AuthError::InvalidToken)
        })?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| self.observe_jwt_error(AuthError::InvalidToken))?
            .as_secs();
        if exp + self.leeway_seconds < now {
            return Err(self.observe_jwt_error(AuthError::InvalidToken));
        }

        if let Some(issuer) = &self.issuer {
            let iss = parse_string_claim(payload_str, "iss").ok_or_else(|| {
                metrics::add_auth_jwt_failure();
                self.observe_jwt_error(AuthError::InvalidToken)
            })?;
            if iss != *issuer {
                return Err(self.observe_jwt_error(AuthError::InvalidToken));
            }
        }

        if let Some(audience) = &self.audience {
            if !claim_matches_audience(payload_str, audience) {
                return Err(self.observe_jwt_error(AuthError::InvalidToken));
            }
        }

        let token_username =
            parse_string_claim(payload_str, &self.username_claim).ok_or_else(|| {
                metrics::add_auth_jwt_failure();
                self.observe_jwt_error(AuthError::InvalidToken)
            })?;

        let token_device_id = self
            .device_id_claim
            .as_ref()
            .map(|device_id_claim| {
                parse_string_claim(payload_str, device_id_claim)
                    .ok_or_else(|| self.observe_jwt_error(AuthError::InvalidToken))
                    .and_then(|device_id| {
                        if device_id.is_empty() {
                            Err(self.observe_jwt_error(AuthError::InvalidToken))
                        } else {
                            Ok(device_id)
                        }
                    })
            })
            .transpose()?;

        let claims = JwtClaims {
            username: token_username,
            device_id: token_device_id,
        };

        self.jwt_cache.lock().expect("cache lock poisoned").insert(
            cache_key,
            JwtCacheEntry {
                result: Ok(claims.clone()),
                expires_at: cache_now + self.cache_ttl,
            },
        );

        Ok(claims)
    }

    pub fn authenticate_with_registry(
        &self,
        token: &str,
        credentials: &CredentialsAuth,
    ) -> Result<(), AuthError> {
        let claims = match self.validate_token(token) {
            Ok(claims) => claims,
            Err(error) => {
                metrics::add_auth_jwt_failure();
                return Err(error);
            }
        };
        let client = credentials
            .find_client_by_login(&claims.username)
            .ok_or_else(|| {
                metrics::add_auth_jwt_failure();
                self.observe_jwt_error(AuthError::InvalidToken)
            })?;

        if client.username != claims.username {
            metrics::add_auth_jwt_failure();
            return Err(self.observe_jwt_error(AuthError::InvalidToken));
        }

        if let Some(token_device_id) = claims.device_id.as_deref() {
            let expected_device_id = client.device_id.as_deref().ok_or_else(|| {
                metrics::add_auth_jwt_failure();
                self.observe_jwt_error(AuthError::InvalidToken)
            })?;
            if expected_device_id != token_device_id {
                metrics::add_auth_jwt_failure();
                return Err(self.observe_jwt_error(AuthError::InvalidToken));
            }
        }

        metrics::add_auth_jwt_success();
        Ok(())
    }

    fn observe_jwt_error(&self, error: AuthError) -> AuthError {
        if self.metrics_jwt_error_enabled && matches!(error, AuthError::InvalidToken) {
            metrics::add_jwt_validation_error("invalid_token");
        }
        error
    }
}

fn split_token(token: &str) -> Result<(&str, &str, &str), AuthError> {
    let mut parts = token.split('.');
    let p1 = parts.next().ok_or(AuthError::InvalidToken)?;
    let p2 = parts.next().ok_or(AuthError::InvalidToken)?;
    let p3 = parts.next().ok_or(AuthError::InvalidToken)?;
    if parts.next().is_some() || p1.is_empty() || p2.is_empty() || p3.is_empty() {
        return Err(AuthError::InvalidToken);
    }
    Ok((p1, p2, p3))
}

fn verify_algorithm(header_json: &str, verifier: &JwtVerifier) -> Result<(), AuthError> {
    let alg = parse_string_claim(header_json, "alg").ok_or(AuthError::InvalidToken)?;
    match (alg.as_str(), verifier) {
        ("RS256", JwtVerifier::Rs256(_)) | ("HS256", JwtVerifier::Hs256(_)) => Ok(()),
        _ => Err(AuthError::InvalidToken),
    }
}

fn parse_string_claim(json: &str, key: &str) -> Option<String> {
    let idx = json.find(&format!("\"{}\"", key))?;
    let rest = &json[idx..];
    let colon = rest.find(':')?;
    let value = rest[colon + 1..].trim_start();
    if !value.starts_with('"') {
        return None;
    }
    let mut escaped = false;
    let mut out = String::new();
    for ch in value[1..].chars() {
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            return Some(out);
        }
        out.push(ch);
    }
    None
}

fn parse_u64_claim(json: &str, key: &str) -> Option<u64> {
    let idx = json.find(&format!("\"{}\"", key))?;
    let rest = &json[idx..];
    let colon = rest.find(':')?;
    let value = rest[colon + 1..].trim_start();
    let mut end = 0;
    for (i, ch) in value.char_indices() {
        if !ch.is_ascii_digit() {
            break;
        }
        end = i + ch.len_utf8();
    }
    if end == 0 {
        return None;
    }
    value[..end].parse().ok()
}

fn claim_matches_audience(payload_json: &str, expected: &str) -> bool {
    if let Some(single) = parse_string_claim(payload_json, "aud") {
        return single == expected;
    }

    let key = "\"aud\"";
    let idx = match payload_json.find(key) {
        Some(i) => i,
        None => return false,
    };
    let rest = &payload_json[idx..];
    let colon = match rest.find(':') {
        Some(i) => i,
        None => return false,
    };
    let value = rest[colon + 1..].trim_start();
    if !value.starts_with('[') {
        return false;
    }

    value
        .split(']')
        .next()
        .is_some_and(|arr| arr.contains(&format!("\"{}\"", expected)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    const TEST_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAwEYaoqp+EraPn9VNa1QQ
9p9h7vlRKTK3418kODh8qE8AVqqfbq7ysPuPV9OJ2v1J/+3LyK3YBk3+qlgwkmeV
8mMzUD8ShIugIo6lCU9XkiqKEbW0ecZGPO9v3t8LksCSlGuFt2BNplSLWGscfMM2
KGvc3VooOkpvGLoaUwFPu6/3cbytZNgF8Kx8U9Xr0gTNugrWfG4bSrSjihjNuERp
gq54BbplVWdOYyEnbbEZk5E7lX3va1CDI1PhO66Md4w5+WUjAMAeREi7nw289Rjw
UqR5QkioJxdzKbaw5NtEDqd3+sKNdKdU576hDQhdsYxzsZA9xaXorztL4Apz5QOa
IwIDAQAB
-----END PUBLIC KEY-----
";

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }
    fn b64(x: &str) -> String {
        URL_SAFE_NO_PAD.encode(x)
    }

    fn hs_token(payload: &str, secret: &str) -> String {
        let h = b64(r#"{"alg":"HS256","typ":"JWT"}"#);
        let p = b64(payload);
        let s = format!("{}.{}", h, p);
        let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
        let sig = hmac::sign(&key, s.as_bytes());
        format!("{}.{}", s, URL_SAFE_NO_PAD.encode(sig.as_ref()))
    }
    const VALID_RS_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJhbGljZSIsImV4cCI6NDEwMjQ0NDgwMCwiaXNzIjoiaXNzIiwiYXVkIjoiYXVkIn0.MZSnq0BV1CnULzQPL2q1MC1TZMizFNrrvDYrheEcHWf_0OovYInZPTohv2-V-KAIY0rE2_5N5XcWOEe2k4hkVOca9gcZtd2fGiAKEQw-RZjNAxaxSsBbNLLvATgUHlKev1dl5DPUMTuYQXnJMKt7Lr2TE_HYTGut7BbfeDrBY5_CXTm9wBTvSKgZdWuET8Hhi4v04FrMbFdXNarQy9vMPcjxPFhmTHNj3ovK8S_71IbS6iFndi6Duqz-j-UaeU0T6-aVNXqmzbIKyiydSpLfy538CQldYwju8dFgK__vvIaaq6FKbNwtWLY4zVSBF41jA-J4eWGcnonW7C96mj4ATA";

    #[test]
    fn valid_rs256_token() {
        let f = tempfile::NamedTempFile::new().unwrap();
        fs::write(f.path(), TEST_PUBLIC_KEY).unwrap();
        let auth = JwtAuth::from_config(&JwtAuthConfig {
            algorithm: JwtAlgorithm::RS256,
            issuer: Some("iss".into()),
            audience: Some("aud".into()),
            leeway_seconds: 0,
            username_claim: "sub".into(),
            device_id_claim: None,
            metrics_jwt_error_enabled: true,
            public_key_path: Some(f.path().to_string_lossy().to_string()),
            hmac_secret_env: None,
        })
        .unwrap();
        assert!(auth.authenticate("alice", VALID_RS_TOKEN).is_ok());
    }

    #[test]
    fn jwt_failures() {
        std::env::set_var("JWT_SECRET", "secret");
        let auth = JwtAuth::from_config(&JwtAuthConfig {
            algorithm: JwtAlgorithm::HS256,
            issuer: Some("iss".into()),
            audience: Some("aud".into()),
            leeway_seconds: 0,
            username_claim: "sub".into(),
            device_id_claim: None,
            metrics_jwt_error_enabled: true,
            public_key_path: None,
            hmac_secret_env: Some("JWT_SECRET".into()),
        })
        .unwrap();
        assert!(auth
            .authenticate(
                "alice",
                &hs_token(
                    &format!(
                        r#"{{"sub":"alice","exp":{},"iss":"iss","aud":"aud"}}"#,
                        now() - 1
                    ),
                    "secret"
                )
            )
            .is_err());
        assert!(auth
            .authenticate(
                "alice",
                &hs_token(
                    &format!(
                        r#"{{"sub":"bob","exp":{},"iss":"iss","aud":"aud"}}"#,
                        now() + 300
                    ),
                    "secret"
                )
            )
            .is_err());
        assert!(auth
            .authenticate(
                "alice",
                &hs_token(
                    &format!(
                        r#"{{"sub":"alice","exp":{},"iss":"bad","aud":"aud"}}"#,
                        now() + 300
                    ),
                    "secret"
                )
            )
            .is_err());
        assert!(auth
            .authenticate(
                "alice",
                &hs_token(
                    &format!(
                        r#"{{"sub":"alice","exp":{},"iss":"iss","aud":"bad"}}"#,
                        now() + 300
                    ),
                    "secret"
                )
            )
            .is_err());
        assert!(auth
            .authenticate(
                "alice",
                &hs_token(r#"{"sub":"alice","iss":"iss","aud":"aud"}"#, "secret")
            )
            .is_err());
        let mut bad = hs_token(
            &format!(
                r#"{{"sub":"alice","exp":{},"iss":"iss","aud":"aud"}}"#,
                now() + 300
            ),
            "secret",
        );
        bad.push('x');
        assert!(auth.authenticate("alice", &bad).is_err());
    }

    #[test]
    fn validates_device_id_claim_if_configured() {
        std::env::set_var("JWT_SECRET", "secret");
        let auth = JwtAuth::from_config(&JwtAuthConfig {
            algorithm: JwtAlgorithm::HS256,
            issuer: Some("iss".into()),
            audience: Some("aud".into()),
            leeway_seconds: 0,
            username_claim: "sub".into(),
            device_id_claim: Some("device_id".into()),
            metrics_jwt_error_enabled: true,
            public_key_path: None,
            hmac_secret_env: Some("JWT_SECRET".into()),
        })
        .unwrap();

        assert!(auth
            .authenticate(
                "alice",
                &hs_token(
                    &format!(
                        r#"{{"sub":"alice","exp":{},"iss":"iss","aud":"aud","device_id":"dev-1"}}"#,
                        now() + 300
                    ),
                    "secret"
                )
            )
            .is_ok());

        assert!(auth
            .authenticate(
                "alice",
                &hs_token(
                    &format!(
                        r#"{{"sub":"alice","exp":{},"iss":"iss","aud":"aud"}}"#,
                        now() + 300
                    ),
                    "secret"
                )
            )
            .is_err());

        assert!(auth
            .authenticate(
                "alice",
                &hs_token(
                    &format!(
                        r#"{{"sub":"alice","exp":{},"iss":"iss","aud":"aud","device_id":""}}"#,
                        now() + 300
                    ),
                    "secret"
                )
            )
            .is_err());
    }

    #[test]
    fn skips_device_id_claim_check_if_not_configured() {
        std::env::set_var("JWT_SECRET", "secret");
        let auth = JwtAuth::from_config(&JwtAuthConfig {
            algorithm: JwtAlgorithm::HS256,
            issuer: Some("iss".into()),
            audience: Some("aud".into()),
            leeway_seconds: 0,
            username_claim: "sub".into(),
            device_id_claim: None,
            metrics_jwt_error_enabled: true,
            public_key_path: None,
            hmac_secret_env: Some("JWT_SECRET".into()),
        })
        .unwrap();

        assert!(auth
            .authenticate(
                "alice",
                &hs_token(
                    &format!(
                        r#"{{"sub":"alice","exp":{},"iss":"iss","aud":"aud"}}"#,
                        now() + 300
                    ),
                    "secret"
                )
            )
            .is_ok());
    }
}
