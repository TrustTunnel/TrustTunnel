use crate::{
    authentication::registry_based, cert_verification::CertificateVerifier,
    settings::TlsHostsSettings, utils::ToTomlComment,
};
#[cfg(feature = "rt_doc")]
use macros::{Getter, RuntimeDoc};
use once_cell::sync::Lazy;
use toml_edit::{value, Document};

/// Percent-encode a username/password for the `userinfo` component of a URL
/// per RFC 3986. Encodes any byte outside the unreserved set `A-Za-z0-9-._~`.
fn percent_encode_userinfo(s: &str) -> String {
    fn is_unreserved(b: u8) -> bool {
        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
    }
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// Split the part of a URL following the scheme into the authority component
/// and the remaining tail (path, query, fragment). The authority ends at the
/// first `/`, `?`, or `#` per RFC 3986.
fn split_authority(url: &str) -> (&str, &str) {
    let authority_end = url.find(['/', '?', '#']).unwrap_or(url.len());
    url.split_at(authority_end)
}

/// Build the subscription URL `https://<user>:<pass>@<host><path>`.
///
/// `base` is `https://<host><path>` (optionally overridden via `--subscription-url`,
/// in which case any embedded userinfo is stripped). Credentials are always
/// appended and percent-encoded.
pub fn build_subscription_url(base: &str, username: &str, password: &str) -> String {
    let rest = base.strip_prefix("https://").unwrap_or(base);
    // Strip userinfo only from the authority part: a '@' in the path or query
    // is valid and must be preserved.
    let (authority, tail) = split_authority(rest);
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host_path = format!("{authority}{tail}");
    format!(
        "https://{}:{}@{host_path}",
        percent_encode_userinfo(username),
        percent_encode_userinfo(password),
    )
}

/// Validate a `--subscription-url` override.
///
/// The override is the host+path base only: an `https://` URL with no embedded
/// userinfo. Credentials are always appended from `credentials.toml`, so an
/// override carrying `user:pass@` is rejected to surface the misuse rather than
/// silently stripping it. A `@` outside the authority (i.e. in the path or
/// query) is left alone.
pub fn validate_subscription_url_override(url: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| "--subscription-url must be an https:// URL".to_string())?;

    let (authority, _) = split_authority(rest);
    if authority.is_empty() {
        return Err("--subscription-url must specify a host".to_string());
    }
    if authority.contains('@') {
        return Err("--subscription-url must not contain credentials; \
             they are appended from credentials.toml"
            .to_string());
    }
    Ok(())
}

/// Derive the port for the subscription URL from the `[subscription] address`
/// value. Returns `None` — the port is omitted from the URL — when `address`
/// is `None`, carries no valid non-zero port, or the port is the HTTPS
/// default (443).
pub fn subscription_url_port(address: Option<&str>) -> Option<u16> {
    let port: u16 = address?.rsplit_once(':')?.1.parse().ok()?;
    (port != 0 && port != 443).then_some(port)
}

#[allow(clippy::too_many_arguments)]
pub fn build(
    client: &String,
    addresses: Vec<String>,
    username: &[registry_based::Client],
    hostsettings: &TlsHostsSettings,
    custom_sni: Option<String>,
    client_random_prefix: Option<String>,
    name: Option<String>,
    dns_upstreams: Vec<String>,
    subscription_url: Option<String>,
) -> ClientConfig {
    let user = username
        .iter()
        .find(|x| x.username == *client)
        .expect("There is no user config for specified username");

    let host = hostsettings
        .main_hosts
        .first()
        .expect("Can't find main host inside hosts config");

    let certificate =
        std::fs::read_to_string(&host.cert_chain_path).expect("Failed to load certificate");

    // Check if certificate is system-verifiable
    let cert_is_system_verifiable = CertificateVerifier::new()
        .ok()
        .map(|verifier| verifier.is_system_verifiable(&host.cert_chain_path, &host.hostname))
        .unwrap_or(false);

    ClientConfig {
        hostname: host.hostname.clone(),
        addresses,
        custom_sni: custom_sni.unwrap_or_default(),
        has_ipv6: true, // Hardcoded to true, client could change this himself
        username: user.username.clone(),
        password: user.password.clone(),
        client_random_prefix: client_random_prefix.unwrap_or_default(),
        skip_verification: false,
        certificate,
        cert_is_system_verifiable,
        upstream_protocol: "http2".into(),
        anti_dpi: false,
        name: name.unwrap_or_default(),
        dns_upstreams,
        subscription_url,
    }
}

#[cfg_attr(feature = "rt_doc", derive(Getter, RuntimeDoc))]
pub struct ClientConfig {
    /// Endpoint host name, used for TLS session establishment
    hostname: String,
    /// Endpoint addresses in `IP:port` or `hostname:port` format
    addresses: Vec<String>,
    /// Custom SNI value for TLS handshake.
    /// If set, this value is used as the TLS SNI instead of the hostname.
    custom_sni: String,
    /// Whether IPv6 traffic can be routed through the endpoint
    has_ipv6: bool,
    /// Username for authorization
    username: String,
    /// Password for authorization
    password: String,
    /// TLS client random hex prefix for connection filtering.
    /// Must have a corresponding rule in rules.toml.
    client_random_prefix: String,
    /// Skip the endpoint certificate verification?
    /// That is, any certificate is accepted with this one set to true.
    skip_verification: bool,
    /// Endpoint certificate in PEM format.
    /// If not specified, the endpoint certificate is verified using the system storage.
    certificate: String,
    /// True if cert can be verified by system CAs (used to omit cert from deep-link)
    cert_is_system_verifiable: bool,
    /// Protocol to be used to communicate with the endpoint [http2, http3]
    upstream_protocol: String,
    /// Is anti-DPI measures should be enabled
    anti_dpi: bool,
    /// Human-readable server display name
    name: String,
    /// DNS upstreams to use when connected to this endpoint
    dns_upstreams: Vec<String>,
    /// Subscription URL (HTTPS, credentials embedded). Present only when
    /// `[subscription]` is enabled on the endpoint. Included in both TOML and
    /// deep-link exports.
    subscription_url: Option<String>,
}

impl ClientConfig {
    pub fn compose_toml(&self) -> String {
        let mut doc: Document = TEMPLATE.parse().unwrap();
        doc["hostname"] = value(&self.hostname);
        let vec = toml_edit::Array::from_iter(self.addresses.iter().map(|x| x.as_str()));
        doc["addresses"] = value(vec);
        doc["custom_sni"] = value(&self.custom_sni);
        doc["has_ipv6"] = value(self.has_ipv6);
        doc["username"] = value(&self.username);
        doc["password"] = value(&self.password);
        doc["client_random_prefix"] = value(&self.client_random_prefix);
        doc["skip_verification"] = value(self.skip_verification);
        if self.cert_is_system_verifiable {
            doc["certificate"] = value("");
        } else {
            doc["certificate"] = value(&self.certificate);
        }
        doc["upstream_protocol"] = value(&self.upstream_protocol);
        doc["anti_dpi"] = value(self.anti_dpi);
        if !self.name.is_empty() {
            doc["name"] = value(&self.name);
        }
        if !self.dns_upstreams.is_empty() {
            let vec = toml_edit::Array::from_iter(self.dns_upstreams.iter().map(|x| x.as_str()));
            doc["dns_upstreams"] = value(vec);
        }
        if let Some(url) = self.subscription_url.as_ref() {
            doc["subscription_url"] = value(url);
        }
        doc.to_string()
    }

    /// Generate a deep-link URI (tt://?) for this client configuration.
    pub fn compose_deeplink(&self) -> std::io::Result<String> {
        use trusttunnel_deeplink::{DeepLinkConfig, Protocol};

        // Convert certificate from PEM to DER if needed
        let certificate = if !self.cert_is_system_verifiable && !self.certificate.is_empty() {
            Some(
                trusttunnel_deeplink::cert::pem_to_der(&self.certificate)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            )
        } else {
            None
        };

        // Parse protocol
        let upstream_protocol: Protocol = self
            .upstream_protocol
            .parse()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

        // Build deep-link config
        let config = DeepLinkConfig {
            hostname: Some(self.hostname.clone()),
            addresses: self.addresses.clone(),
            username: Some(self.username.clone()),
            password: Some(self.password.clone()),
            client_random_prefix: if self.client_random_prefix.is_empty() {
                None
            } else {
                Some(self.client_random_prefix.clone())
            },
            custom_sni: if self.custom_sni.is_empty() {
                None
            } else {
                Some(self.custom_sni.clone())
            },
            has_ipv6: self.has_ipv6,
            skip_verification: self.skip_verification,
            certificate,
            upstream_protocol,
            anti_dpi: self.anti_dpi,
            name: if self.name.is_empty() {
                None
            } else {
                Some(self.name.clone())
            },
            dns_upstreams: self.dns_upstreams.clone(),
            subscription_url: self.subscription_url.clone(),
        };

        trusttunnel_deeplink::encode(&config)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Generate a subscription-only deep-link URI (format v2) containing just
    /// the subscription URL. Clients must fetch the subscription before they
    /// can connect; there are no static fallback parameters.
    pub fn compose_deeplink_subscription_only(&self) -> std::io::Result<String> {
        let subscription_url = self.subscription_url.clone().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "subscription is not enabled on the endpoint",
            )
        })?;

        let config = trusttunnel_deeplink::DeepLinkConfig::builder()
            .subscription_url(Some(subscription_url))
            .build()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        trusttunnel_deeplink::encode(&config)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

static TEMPLATE: Lazy<String> = Lazy::new(|| {
    format!(
        r#"
# This file was automatically generated by endpoint and could be used in vpn client.

{}
hostname = ""

{}
addresses = []

{}
custom_sni = ""

{}
has_ipv6 = true

{}
username = ""

{}
password = ""

{}
client_random_prefix = ""

{}
skip_verification = false

{}
certificate = ""

{}
upstream_protocol = ""

{}
anti_dpi = false

{}
name = ""

{}
dns_upstreams = []
"#,
        ClientConfig::doc_hostname().to_toml_comment(),
        ClientConfig::doc_addresses().to_toml_comment(),
        ClientConfig::doc_custom_sni().to_toml_comment(),
        ClientConfig::doc_has_ipv6().to_toml_comment(),
        ClientConfig::doc_username().to_toml_comment(),
        ClientConfig::doc_password().to_toml_comment(),
        ClientConfig::doc_client_random_prefix().to_toml_comment(),
        ClientConfig::doc_skip_verification().to_toml_comment(),
        ClientConfig::doc_certificate().to_toml_comment(),
        ClientConfig::doc_upstream_protocol().to_toml_comment(),
        ClientConfig::doc_anti_dpi().to_toml_comment(),
        ClientConfig::doc_name().to_toml_comment(),
        ClientConfig::doc_dns_upstreams().to_toml_comment(),
    )
});
#[cfg(test)]
mod tests {
    use super::*;

    impl ClientConfig {
        fn test_config(certificate: String, cert_is_system_verifiable: bool) -> Self {
            ClientConfig {
                hostname: "vpn.example.com".into(),
                addresses: vec!["1.2.3.4:443".parse().unwrap()],
                custom_sni: String::new(),
                has_ipv6: true,
                username: "alice".into(),
                password: "secret".into(),
                client_random_prefix: String::new(),
                skip_verification: false,
                certificate,
                cert_is_system_verifiable,
                upstream_protocol: "http2".into(),
                anti_dpi: false,
                name: String::new(),
                dns_upstreams: vec![],
                subscription_url: None,
            }
        }
    }

    // Two-certificate PEM chain: leaf (CN=vpn.example.com) + CA (CN=Test CA)
    const TWO_CERT_PEM_CHAIN: &str = "\
-----BEGIN CERTIFICATE-----\n\
MIIC/DCCAeSgAwIBAgIUCI9VIilTMYZq4JfFnFjCuQsAiGIwDQYJKoZIhvcNAQEL\n\
BQAwEjEQMA4GA1UEAwwHVGVzdCBDQTAeFw0yNjAyMjYxMzEyMDBaFw0yNzAyMjYx\n\
MzEyMDBaMBoxGDAWBgNVBAMMD3Zwbi5leGFtcGxlLmNvbTCCASIwDQYJKoZIhvcN\n\
AQEBBQADggEPADCCAQoCggEBAKnrz9FwFq2xRpOu0D+2hFwymMaixPr556MuB4P1\n\
nLv8vqRQ3MBZn7p48QTywO5OAqIDL27hpigM1e2tc45UuAuaMYoz+Ryty3O75k9X\n\
sdYaVaupOLNWBtbjNntRzFgMpYwbz+lZYuaKqwdRmCJM71Af2jt7aPGSUXeMMR/A\n\
QZZNlRfQuA6NdmhzNsXjaA6xLDBYPk1nGYnFpMxOTlOD9jhM/lImrAMDBATEoMXO\n\
CyhEclgbJtYla6D5Q5Go3NlbMLPr6zOddoL5g7MkQmerODiWlLAlMPIvC33Bz9FU\n\
Dn5wVJ8G5gSFDjq66cL30a9Gq8lWStuy9d3WeXSY5WcBzoMCAwEAAaNCMEAwHQYD\n\
VR0OBBYEFB/yEYFRHwyDdA8/EaeiIi/padZgMB8GA1UdIwQYMBaAFGuqVmspjq2L\n\
h+FhwZJL3VYEm58DMA0GCSqGSIb3DQEBCwUAA4IBAQBqloNE2yxi/6x3KMOVS4bN\n\
+576mpwU+Kx3bDvAvEP8kNtnvOvLKYATaIHsWK+uHvVjYPf7Nw1InUg3GKnE86IH\n\
mr1PgUri9ECKucg9UkOyzdS2VdeWeL+ME2POpg3ARXici5vUngzcKPQmVBu27PSK\n\
dUgkNHQPSxWkBytrxLBi3dynL5qnyoOfzmXkl1odV5XPE77NtvoR4LD5z1/Tn4a1\n\
StvzAN22qiDLkP4MwOir5r21bShJt4otXyNXFZHA0gE19AjLxmknms8D2v3L4ytx\n\
UGXW9acA8MoG1D+TT6jQjGqupznNL/73xMRYazqFjaVCpmaaSYGP41AkLsHuiMti\n\
-----END CERTIFICATE-----\n\
-----BEGIN CERTIFICATE-----\n\
MIIDBTCCAe2gAwIBAgIUJQlOhwer2yHQbyhVtk86+1587qowDQYJKoZIhvcNAQEL\n\
BQAwEjEQMA4GA1UEAwwHVGVzdCBDQTAeFw0yNjAyMjYxMzEyMDBaFw0yNzAyMjYx\n\
MzEyMDBaMBIxEDAOBgNVBAMMB1Rlc3QgQ0EwggEiMA0GCSqGSIb3DQEBAQUAA4IB\n\
DwAwggEKAoIBAQCbWJQG4lT5uK571FUQqgZuPcfeCtuvI+WCIfxmGk58zI0wmBDS\n\
zaZroUVvcEV4qva+03hDENsKNTypDDlMrd83qzc3rEOLBezNrSQVlbiTNG7lYHU1\n\
3lw9//BlvNmjVBHcQ0643Q+XilG7sDSt3KuqoAT2CiLxm4A/xVN/uzfAoBZhFn5h\n\
oik448kqXXNh6PsofoZO3jTh+4JZuD++xvj+cVdKzH25UIWWCJxBrNqR9zXo8WO5\n\
UFcxxVWnHSqpS8dvpFGVj6B7kyjZZb7TSYYuEJoMplN3uR25nMHgrXse0mvatCRi\n\
uDygNx6Vzg2R7akQXD0bqBVyRmzKY/xAO7CLAgMBAAGjUzBRMB0GA1UdDgQWBBRr\n\
qlZrKY6ti4fhYcGSS91WBJufAzAfBgNVHSMEGDAWgBRrqlZrKY6ti4fhYcGSS91W\n\
BJufAzAPBgNVHRMBAf8EBTADAQH/MA0GCSqGSIb3DQEBCwUAA4IBAQCII03BWTUn\n\
nT2HJrh67ywq34UwWFqqJA0AQIetpS933waW01yr7YJxq3TAznVgsiXKkU/9bFvx\n\
9u4mnzMHy+LJeGw5TtveDmKz22Jr45KH0ug3kikqdPVqB+ur2Kx73ao0SXFCyeIi\n\
6E57QnwyAWmSxIKzjIDreMr0Y2tWRfwvgsRkxZZP3Ps+SQakz6yfYoSJesJxJ0o2\n\
OzTTMTfK4lR2f/QP4MGp8E0dImkfm9eLq6be8VoaNt2nx1MqiD2AxMF3w7FAXmCS\n\
jhjuhML7Zp8c0/3g+r/60sv/9x4DrPeXTYrGCK+qLgZ1qxpwIARNbl780fGnZCIf\n\
omxU7kknZApM\n\
-----END CERTIFICATE-----\n";

    #[test]
    fn test_compose_toml_self_signed_cert_chain() {
        let config = ClientConfig::test_config(TWO_CERT_PEM_CHAIN.to_string(), false);
        let toml_output = config.compose_toml();

        let doc: Document = toml_output.parse().unwrap();
        let cert_value = doc["certificate"].as_str().unwrap();

        assert!(
            cert_value.contains("-----BEGIN CERTIFICATE-----"),
            "TOML should contain certificate when not system-verifiable"
        );
        assert_eq!(
            cert_value.matches("-----BEGIN CERTIFICATE-----").count(),
            2,
            "TOML should contain both certs from the chain"
        );
    }

    #[test]
    fn test_compose_toml_system_verifiable_cert_omitted() {
        let config = ClientConfig::test_config(TWO_CERT_PEM_CHAIN.to_string(), true);
        let toml_output = config.compose_toml();

        let doc: Document = toml_output.parse().unwrap();
        let cert_value = doc["certificate"].as_str().unwrap();

        assert_eq!(
            cert_value, "",
            "TOML certificate should be empty when cert is system-verifiable"
        );
    }

    #[test]
    fn test_compose_deeplink_self_signed_cert_chain() {
        let config = ClientConfig::test_config(TWO_CERT_PEM_CHAIN.to_string(), false);
        let uri = config.compose_deeplink().unwrap();

        let decoded = trusttunnel_deeplink::decode(&uri).unwrap();
        let cert_der = decoded
            .certificate
            .expect("Deep-link should contain certificate when not system-verifiable");

        let pem = trusttunnel_deeplink::cert::der_to_pem(&cert_der).unwrap();
        assert_eq!(
            pem.matches("-----BEGIN CERTIFICATE-----").count(),
            2,
            "Deep-link DER should contain both certs from the chain"
        );
    }

    #[test]
    fn test_compose_deeplink_system_verifiable_cert_omitted() {
        let config = ClientConfig::test_config(TWO_CERT_PEM_CHAIN.to_string(), true);
        let uri = config.compose_deeplink().unwrap();

        let decoded = trusttunnel_deeplink::decode(&uri).unwrap();
        assert!(
            decoded.certificate.is_none(),
            "Deep-link should not contain certificate when cert is system-verifiable"
        );
    }

    #[test]
    fn percent_encode_reserved_chars_in_userinfo() {
        assert_eq!(percent_encode_userinfo("alice"), "alice");
        assert_eq!(percent_encode_userinfo("a:b@/"), "a%3Ab%40%2F");
    }

    #[test]
    fn build_subscription_url_appends_credentials() {
        let url = build_subscription_url("https://vpn.example.com/subscription", "alice", "s3cret");
        assert_eq!(url, "https://alice:s3cret@vpn.example.com/subscription");
    }

    #[test]
    fn build_subscription_url_strips_existing_userinfo_in_override() {
        let url = build_subscription_url("https://old:old@sub.example.com/sub", "alice", "p@ss");
        assert_eq!(url, "https://alice:p%40ss@sub.example.com/sub");
    }

    #[test]
    fn build_subscription_url_preserves_at_in_path_and_query() {
        // `@` after the authority is not userinfo and must be preserved,
        // matching what `validate_subscription_url_override` allows.
        let url = build_subscription_url("https://vpn.example.com/p@ath", "alice", "s3cret");
        assert_eq!(url, "https://alice:s3cret@vpn.example.com/p@ath");
        let url =
            build_subscription_url("https://vpn.example.com/sub?next=/p@ath", "alice", "s3cret");
        assert_eq!(url, "https://alice:s3cret@vpn.example.com/sub?next=/p@ath");
        // The authority ends at the first `?` or `#` even without a path.
        let url = build_subscription_url("https://vpn.example.com?next=@x", "alice", "s3cret");
        assert_eq!(url, "https://alice:s3cret@vpn.example.com?next=@x");
    }

    #[test]
    fn compose_toml_emits_subscription_url_when_set() {
        let mut config = ClientConfig::test_config(TWO_CERT_PEM_CHAIN.to_string(), false);
        config.subscription_url =
            Some("https://alice:s3cret@vpn.example.com/subscription".to_string());
        let toml_output = config.compose_toml();

        let doc: Document = toml_output.parse().unwrap();
        assert_eq!(
            doc["subscription_url"].as_str().unwrap(),
            "https://alice:s3cret@vpn.example.com/subscription"
        );
    }

    #[test]
    fn compose_deeplink_includes_subscription_url() {
        let mut config = ClientConfig::test_config(TWO_CERT_PEM_CHAIN.to_string(), false);
        let url = "https://alice:s3cret@vpn.example.com/subscription".to_string();
        config.subscription_url = Some(url.clone());
        let uri = config.compose_deeplink().unwrap();

        let decoded = trusttunnel_deeplink::decode(&uri).unwrap();
        assert_eq!(decoded.subscription_url.as_deref(), Some(url.as_str()));
        // Static fallback parameters are still present.
        assert_eq!(decoded.hostname.as_deref(), Some("vpn.example.com"));
        assert_eq!(decoded.username.as_deref(), Some("alice"));
    }

    #[test]
    fn compose_deeplink_without_subscription_url() {
        let config = ClientConfig::test_config(TWO_CERT_PEM_CHAIN.to_string(), false);
        let uri = config.compose_deeplink().unwrap();

        let decoded = trusttunnel_deeplink::decode(&uri).unwrap();
        assert_eq!(decoded.subscription_url, None);
    }

    #[test]
    fn compose_deeplink_subscription_only_minimal() {
        let mut config = ClientConfig::test_config(TWO_CERT_PEM_CHAIN.to_string(), false);
        let url = "https://alice:s3cret@vpn.example.com/subscription".to_string();
        config.subscription_url = Some(url.clone());
        let uri = config.compose_deeplink_subscription_only().unwrap();

        let decoded = trusttunnel_deeplink::decode(&uri).unwrap();
        assert_eq!(decoded.subscription_url.as_deref(), Some(url.as_str()));
        assert_eq!(decoded.hostname, None);
        assert!(decoded.addresses.is_empty());
        assert_eq!(decoded.username, None);
        assert_eq!(decoded.password, None);
        assert_eq!(decoded.certificate, None);
    }

    #[test]
    fn compose_deeplink_subscription_only_requires_url() {
        let config = ClientConfig::test_config(TWO_CERT_PEM_CHAIN.to_string(), false);
        assert!(config.compose_deeplink_subscription_only().is_err());
    }

    #[test]
    fn validate_override_accepts_plain_https() {
        assert!(validate_subscription_url_override("https://vpn.example.com/subscription").is_ok());
        assert!(validate_subscription_url_override("https://vpn.example.com").is_ok());
        assert!(validate_subscription_url_override("https://1.2.3.4:443/subscription").is_ok());
    }

    #[test]
    fn validate_override_rejects_non_https() {
        let err = validate_subscription_url_override("http://vpn.example.com/subscription");
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("https"));
    }

    #[test]
    fn validate_override_rejects_embedded_credentials() {
        let err =
            validate_subscription_url_override("https://alice:s3cret@vpn.example.com/subscription");
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("credential"));
    }

    #[test]
    fn validate_override_accepts_at_outside_authority() {
        // `@` in the path or query is not userinfo and is allowed.
        assert!(validate_subscription_url_override("https://vpn.example.com/p@ath").is_ok());
        assert!(validate_subscription_url_override("https://vpn.example.com/sub?next=@x").is_ok());
        // The authority ends at the first `?` or `#` even without a path.
        assert!(validate_subscription_url_override("https://vpn.example.com?next=@x").is_ok());
    }

    #[test]
    fn validate_override_rejects_empty_host() {
        assert!(validate_subscription_url_override("https://").is_err());
        assert!(validate_subscription_url_override("https:///subscription").is_err());
    }

    #[test]
    fn subscription_url_port_included_for_non_default_port() {
        assert_eq!(subscription_url_port(Some("1.2.3.4:8000")), Some(8000));
        assert_eq!(
            subscription_url_port(Some("vpn.example.com:8443")),
            Some(8443)
        );
        assert_eq!(
            subscription_url_port(Some("[2001:db8::1]:8000")),
            Some(8000)
        );
    }

    #[test]
    fn subscription_url_port_omitted_when_address_absent() {
        assert_eq!(subscription_url_port(None), None);
    }

    #[test]
    fn subscription_url_port_omitted_for_default_port() {
        assert_eq!(subscription_url_port(Some("1.2.3.4:443")), None);
        assert_eq!(subscription_url_port(Some("vpn.example.com:443")), None);
    }

    #[test]
    fn subscription_url_port_omitted_when_port_invalid() {
        assert_eq!(subscription_url_port(Some("vpn.example.com")), None);
        assert_eq!(
            subscription_url_port(Some("vpn.example.com:notaport")),
            None
        );
        assert_eq!(subscription_url_port(Some("1.2.3.4:0")), None);
        assert_eq!(subscription_url_port(Some("1.2.3.4:99999")), None);
    }
}
