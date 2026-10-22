use crate::settings::{Settings, TlsHostInfo, TlsHostsSettings, ValidationError};

/// The `[subscription]` settings parsed from `vpn.toml`.
///
/// Served over an existing main TLS host or a dedicated host from
/// [`TlsHostsSettings::subscription_hosts`]; live connection parameters not
/// listed here are derived from the endpoint configuration at resolve time.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[cfg_attr(feature = "rt_doc", derive(macros::RuntimeDoc))]
pub struct SubscriptionSettings {
    /// Master switch. When `false` the endpoint does not serve subscription data.
    #[serde(default)]
    pub enabled: bool,
    /// Hostname matching a `main_hosts` entry; required.
    #[serde(default)]
    pub hostname: Option<String>,
    /// HTTP path at which the subscription JSON is served.
    #[serde(default = "SubscriptionSettings::default_path")]
    pub path: String,
    /// Endpoint address (`host:port` / `ip:port`) placed in the JSON response; required.
    #[serde(default)]
    pub address: Option<String>,
    /// Human-readable server label; creation-only hint for clients.
    #[serde(default)]
    pub name: Option<String>,
    /// Suggested DNS upstreams; creation-only hint for clients.
    #[serde(default)]
    pub dns_upstreams: Vec<String>,
    /// Custom SNI value for the client's TLS handshake. Omitted from the JSON
    /// response when unset or empty.
    #[serde(default)]
    pub custom_sni: Option<String>,
    /// TLS client random hex prefix (`prefix[/mask]`) for connection filtering.
    /// Validated as `hex[/hex]`. Omitted from the JSON when unset or empty.
    #[serde(default)]
    pub client_random_prefix: Option<String>,
    /// Hex auth key from which the client derives its TLS `client_random` for
    /// connection filtering. Mutually exclusive with `client_random_prefix`.
    /// Omitted from the JSON when unset or empty.
    #[serde(default)]
    pub client_random_auth_key: Option<String>,
}

impl SubscriptionSettings {
    pub fn default_path() -> String {
        "/subscription".to_string()
    }
}

impl Default for SubscriptionSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            hostname: None,
            path: Self::default_path(),
            address: None,
            name: None,
            dns_upstreams: Vec::new(),
            custom_sni: None,
            client_random_prefix: None,
            client_random_auth_key: None,
        }
    }
}

/// Validate the fields that do not depend on TLS hosts.
pub(crate) fn validate(
    sub: &SubscriptionSettings,
    settings: &Settings,
) -> Result<(), ValidationError> {
    let path = sub.path.as_str();
    if path.is_empty() || !path.starts_with('/') || path == "/" {
        return Err(ValidationError::InvalidPath(format!(
            "subscription path: {path}"
        )));
    }

    let address = sub.address.as_deref().unwrap_or("");
    let has_valid_port = address
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse::<u16>().ok())
        .is_some_and(|port| port != 0);
    if !has_valid_port {
        return Err(ValidationError::Subscription(format!(
            "subscription address is not a valid host:port: {address}"
        )));
    }

    if let Some(prefix) = sub
        .client_random_prefix
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        let (prefix_part, mask_part) = prefix.split_once('/').unwrap_or((prefix, ""));
        if hex::decode(prefix_part).is_err() {
            return Err(ValidationError::Subscription(format!(
                "subscription client_random_prefix is not valid hex: {prefix}"
            )));
        }
        if !mask_part.is_empty() && hex::decode(mask_part).is_err() {
            return Err(ValidationError::Subscription(format!(
                "subscription client_random_prefix mask is not valid hex: {prefix}"
            )));
        }
    }

    let auth_key = sub
        .client_random_auth_key
        .as_deref()
        .filter(|s| !s.is_empty());
    let has_prefix = sub
        .client_random_prefix
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    if auth_key.is_some() && has_prefix {
        return Err(ValidationError::Subscription(
            "subscription client_random_auth_key and client_random_prefix are mutually exclusive"
                .to_string(),
        ));
    }
    if let Some(auth_key) = auth_key {
        // The auth key is a secret, do not log its value
        if !crate::rules::is_valid_hex(auth_key) {
            return Err(ValidationError::Subscription(
                "subscription client_random_auth_key is not valid hex".to_string(),
            ));
        }
        if let Some(rules_engine) = &settings.rules_engine {
            let matching_rule = rules_engine.config().rule.iter().find(|r| {
                r.client_random_auth_key
                    .as_ref()
                    .map(|s| s.to_lowercase())
                    .as_deref()
                    == Some(auth_key.to_lowercase().as_str())
            });
            match matching_rule {
                None => {
                    return Err(ValidationError::Subscription(
                        "no rule found in rules.toml with matching client_random_auth_key; \
                         add the rule first or unset subscription client_random_auth_key"
                            .to_string(),
                    ));
                }
                Some(rule) if rule.action == crate::rules::RuleAction::Deny => {
                    return Err(ValidationError::Subscription(
                        "matched rule in rules.toml for client_random_auth_key has action 'deny'"
                            .to_string(),
                    ));
                }
                Some(_) => {}
            }
        }
    }

    check_path_overlap(path, settings.ping_path.as_deref())?;
    check_path_overlap(path, settings.speedtest_path.as_deref())?;
    if let Some(reverse_proxy) = settings.reverse_proxy.as_ref() {
        check_path_overlap(path, Some(reverse_proxy.path_mask.as_str()))?;
    }

    Ok(())
}

fn check_path_overlap(path: &str, other: Option<&str>) -> Result<(), ValidationError> {
    let Some(other) = other else {
        return Ok(());
    };
    if path == other || path.starts_with(other) || other.starts_with(path) {
        return Err(ValidationError::InvalidPath(format!(
            "subscription path overlaps an existing path: {path} vs {other}"
        )));
    }
    Ok(())
}

/// Resolved runtime subscription configuration.
///
/// Built by [`resolve`] from the parsed settings and the loaded TLS hosts. The
/// certificate is read once (at load/reload) so request handling is self-contained.
#[derive(Clone)]
pub(crate) struct SubscriptionConfig {
    pub(crate) enabled: bool,
    pub(crate) path: String,
    pub(crate) hostname: String,
    pub(crate) address: String,
    pub(crate) name: Option<String>,
    pub(crate) dns_upstreams: Vec<String>,
    /// `Some(pem)` when the certificate is not system-verifiable; `None` otherwise.
    pub(crate) certificate: Option<String>,
    /// Custom SNI; `None` when unset/empty (omitted from the JSON).
    pub(crate) custom_sni: Option<String>,
    /// Client-random hex prefix; `None` when unset/empty (omitted from the JSON).
    pub(crate) client_random_prefix: Option<String>,
    /// Hex auth key for derived client random; `None` when unset/empty (omitted
    /// from the JSON).
    pub(crate) client_random_auth_key: Option<String>,
}

/// The JSON body served to an authenticated user.
#[derive(serde::Serialize)]
pub(crate) struct SubscriptionResponse<'a> {
    pub version: u32,
    pub hostname: &'a str,
    pub address: &'a str,
    pub username: &'a str,
    pub password: &'a str,
    pub has_ipv6: bool,
    pub upstream_protocol: &'a str,
    pub anti_dpi: bool,
    pub skip_verification: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_sni: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_random_prefix: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_random_auth_key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dns_upstreams: &'a Vec<String>,
}

/// Return the main host whose hostname matches `[subscription].hostname`.
///
/// The subscription section describes a main host, so the section is invalid
/// when its `hostname` is absent from `main_hosts`.
pub(crate) fn matching_host<'a>(
    sub: &SubscriptionSettings,
    hosts: &'a TlsHostsSettings,
) -> Result<&'a TlsHostInfo, ValidationError> {
    let hostname = sub.hostname.as_deref().unwrap_or("");
    hosts
        .main_hosts
        .iter()
        .find(|h| h.hostname == hostname)
        .ok_or_else(|| {
            ValidationError::Subscription(format!(
                "subscription hostname '{hostname}' is not present in main_hosts"
            ))
        })
}

/// Resolve an already validated [`SubscriptionSettings`] into a
/// [`SubscriptionConfig`].
///
/// Matches the configured hostname against `main_hosts`, reads the matched
/// host's certificate chain, and checks its system-verifiability to decide
/// whether the certificate must be embedded in the subscription response.
/// Returns `Err` (without mutating any runtime state) on any failure, so
/// reload callers can keep the previous configuration.
pub(crate) fn resolve(
    sub: Option<&SubscriptionSettings>,
    hosts: &TlsHostsSettings,
) -> Result<Option<SubscriptionConfig>, ValidationError> {
    let Some(sub) = sub else {
        return Ok(None);
    };
    let host = matching_host(sub, hosts)?;

    let system_verifiable =
        crate::cert_verification::is_system_verifiable(&host.cert_chain_path, &host.hostname);

    let certificate = if system_verifiable {
        None
    } else {
        Some(std::fs::read_to_string(&host.cert_chain_path).map_err(|e| {
            ValidationError::Subscription(format!(
                "failed to read certificate '{}': {}",
                host.cert_chain_path, e
            ))
        })?)
    };

    Ok(Some(SubscriptionConfig {
        enabled: sub.enabled,
        path: sub.path.clone(),
        hostname: sub.hostname.clone().unwrap_or_default(),
        address: sub.address.clone().unwrap_or_default(),
        name: sub.name.clone(),
        dns_upstreams: sub.dns_upstreams.clone(),
        certificate,
        custom_sni: sub.custom_sni.clone().filter(|s| !s.is_empty()),
        client_random_prefix: sub.client_random_prefix.clone().filter(|s| !s.is_empty()),
        client_random_auth_key: sub.client_random_auth_key.clone().filter(|s| !s.is_empty()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{
        Http1Settings, Http2Settings, ListenProtocolSettings, QuicSettings, Settings,
        ValidationError,
    };

    fn enabled_sub() -> SubscriptionSettings {
        SubscriptionSettings {
            enabled: true,
            hostname: Some("vpn.example.com".to_string()),
            path: SubscriptionSettings::default_path(),
            address: Some("1.2.3.4:443".to_string()),
            name: None,
            dns_upstreams: vec![],
            custom_sni: None,
            client_random_prefix: None,
            client_random_auth_key: None,
        }
    }

    #[derive(serde::Deserialize)]
    struct Wrapper {
        #[serde(default)]
        subscription: Option<SubscriptionSettings>,
    }

    #[test]
    fn parse_defaults_when_minimal() {
        let toml = "[subscription]\nenabled = true\n";
        let w: Wrapper = toml::from_str(toml).unwrap();
        let sub = w.subscription.expect("section present");
        assert!(sub.enabled);
        assert_eq!(sub.path, "/subscription");
        assert_eq!(sub.hostname, None);
        assert_eq!(sub.address, None);
        assert!(sub.dns_upstreams.is_empty());
    }

    #[test]
    fn parse_absent_section_is_none() {
        let w: Wrapper = toml::from_str("").unwrap();
        assert!(w.subscription.is_none());
    }

    #[test]
    fn validate_accepts_disabled_section_with_valid_path() {
        let mut sub = enabled_sub();
        sub.enabled = false;
        let settings = Settings::default();
        assert!(validate(&sub, &settings).is_ok());
    }

    #[test]
    fn validate_disabled_section_rejects_invalid_address() {
        let mut sub = enabled_sub();
        sub.enabled = false;
        sub.address = Some("no-port-here".to_string());
        let settings = Settings::default();
        assert!(matches!(
            validate(&sub, &settings),
            Err(ValidationError::Subscription(_))
        ));
    }

    #[test]
    fn validate_disabled_section_rejects_bad_path() {
        let mut sub = enabled_sub();
        sub.enabled = false;
        sub.path = "/".to_string();
        let settings = Settings::default();
        assert!(matches!(
            validate(&sub, &settings),
            Err(ValidationError::InvalidPath(_))
        ));
    }

    #[test]
    fn validate_rejects_root_path() {
        let mut sub = enabled_sub();
        sub.path = "/".to_string();
        let settings = Settings::default();
        assert!(matches!(
            validate(&sub, &settings),
            Err(ValidationError::InvalidPath(_))
        ));
    }

    #[test]
    fn validate_rejects_address_without_port() {
        let mut sub = enabled_sub();
        sub.address = Some("1.2.3.4".to_string());
        let settings = Settings::default();
        assert!(matches!(
            validate(&sub, &settings),
            Err(ValidationError::Subscription(_))
        ));
    }

    #[test]
    fn validate_rejects_invalid_client_random_prefix() {
        let mut sub = enabled_sub();
        sub.client_random_prefix = Some("nothex".to_string());
        let settings = Settings::default();
        assert!(matches!(
            validate(&sub, &settings),
            Err(ValidationError::Subscription(_))
        ));
    }

    #[test]
    fn validate_accepts_valid_client_random_prefix_with_mask() {
        let mut sub = enabled_sub();
        sub.client_random_prefix = Some("aabbcc/ff00ff".to_string());
        let settings = Settings::default();
        assert!(validate(&sub, &settings).is_ok());
    }

    fn settings_with_auth_key_rule(key: &str, action: crate::rules::RuleAction) -> Settings {
        Settings::builder()
            .listen_address(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .unwrap()
            .listen_protocols(ListenProtocolSettings {
                http1: Some(Http1Settings::builder().build()),
                http2: Some(Http2Settings::builder().build()),
                quic: Some(QuicSettings::builder().build()),
            })
            .rules_engine(crate::rules::RulesEngine::from_config(
                crate::rules::RulesConfig {
                    rule: vec![crate::rules::Rule {
                        cidr: None,
                        client_random_prefix: None,
                        client_random_auth_key: Some(key.to_string()),
                        action,
                    }],
                },
            ))
            .build()
            .unwrap()
    }

    #[test]
    fn validate_rejects_auth_key_with_prefix() {
        let mut sub = enabled_sub();
        sub.client_random_auth_key = Some("aabbccdd".to_string());
        sub.client_random_prefix = Some("aabbcc/ff00ff".to_string());
        let settings = Settings::default();
        assert!(matches!(
            validate(&sub, &settings),
            Err(ValidationError::Subscription(_))
        ));
    }

    #[test]
    fn validate_rejects_non_hex_auth_key() {
        let mut sub = enabled_sub();
        sub.client_random_auth_key = Some("nothex".to_string());
        let settings = Settings::default();
        assert!(matches!(
            validate(&sub, &settings),
            Err(ValidationError::Subscription(_))
        ));
    }

    #[test]
    fn validate_rejects_auth_key_without_matching_rule() {
        let mut sub = enabled_sub();
        sub.client_random_auth_key = Some("aabbccdd".to_string());
        let settings = Settings::default();
        assert!(matches!(
            validate(&sub, &settings),
            Err(ValidationError::Subscription(_))
        ));
    }

    #[test]
    fn validate_accepts_auth_key_with_matching_allow_rule() {
        let mut sub = enabled_sub();
        sub.client_random_auth_key = Some("AABBCCDD".to_string());
        let settings = settings_with_auth_key_rule("aabbccdd", crate::rules::RuleAction::Allow);
        assert!(validate(&sub, &settings).is_ok());
    }

    #[test]
    fn validate_rejects_auth_key_with_deny_rule() {
        let mut sub = enabled_sub();
        sub.client_random_auth_key = Some("aabbccdd".to_string());
        let settings = settings_with_auth_key_rule("aabbccdd", crate::rules::RuleAction::Deny);
        assert!(matches!(
            validate(&sub, &settings),
            Err(ValidationError::Subscription(_))
        ));
    }

    #[test]
    fn validate_rejects_path_overlap_with_ping() {
        let mut sub = enabled_sub();
        sub.path = "/ping".to_string();

        let mut settings = Settings::default();
        settings.ping_path = Some("/ping".to_string());

        assert!(matches!(
            validate(&sub, &settings),
            Err(ValidationError::InvalidPath(_))
        ));
    }

    #[test]
    fn validate_accepts_enabled_section() {
        let settings = Settings::default();
        assert!(validate(&enabled_sub(), &settings).is_ok());
    }

    fn tls_hosts_with(hostname: &str) -> TlsHostsSettings {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("trusttunnel_sub_test_cert_{id}.pem"));
        std::fs::write(&tmp, b"").unwrap();
        let path = tmp.to_str().unwrap();
        toml::from_str(&format!(
            "[[main_hosts]]\nhostname = \"{hostname}\"\ncert_chain_path = \"{path}\"\nprivate_key_path = \"{path}\"\n"
        ))
        .unwrap()
    }

    #[test]
    fn matching_host_returns_matching_host() {
        let hosts = tls_hosts_with("vpn.example.com");
        let host = matching_host(&enabled_sub(), &hosts).unwrap();
        assert_eq!(host.hostname, "vpn.example.com");
    }

    #[test]
    fn matching_host_rejects_unknown_hostname() {
        let hosts = tls_hosts_with("other.example.com");
        assert!(matches!(
            matching_host(&enabled_sub(), &hosts),
            Err(ValidationError::Subscription(_))
        ));
    }
}
