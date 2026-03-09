use crate::{
    authentication, authentication::registry_based, cert_verification::CertificateVerifier,
    settings::TlsHostsSettings, utils::ToTomlComment,
};
#[cfg(feature = "rt_doc")]
use macros::{Getter, RuntimeDoc};
use once_cell::sync::Lazy;
use std::net::SocketAddr;
use toml_edit::{value, Document};

pub fn build(
    client: &String,
    addresses: Vec<SocketAddr>,
    username: &[registry_based::Client],
    hostsettings: &TlsHostsSettings,
    custom_sni: Option<String>,
    client_random_prefix: Option<String>,
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
        tunnel_token: authentication::tunnel_token_from_credentials(&user.username, &user.password),
        skip_verification: false,
        certificate,
        cert_is_system_verifiable,
        upstream_protocol: "http2".into(),
        anti_dpi: false,
    }
}

#[cfg_attr(feature = "rt_doc", derive(Getter, RuntimeDoc))]
pub struct ClientConfig {
    /// Endpoint host name, used for TLS session establishment
    hostname: String,
    /// Endpoint addresses in `IP:port` or `hostname:port` format
    addresses: Vec<SocketAddr>,
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
    /// Tunnel token for `X-Tunnel-Token` header (SHA-256 of `username:password`, hex)
    tunnel_token: String,
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
}

impl ClientConfig {
    pub fn compose_toml(&self) -> String {
        let mut doc: Document = TEMPLATE.parse().unwrap();
        doc["hostname"] = value(&self.hostname);
        let vec = toml_edit::Array::from_iter(self.addresses.iter().map(|x| x.to_string()));
        doc["addresses"] = value(vec);
        doc["custom_sni"] = value(&self.custom_sni);
        doc["has_ipv6"] = value(self.has_ipv6);
        doc["username"] = value(&self.username);
        doc["password"] = value(&self.password);
        doc["client_random_prefix"] = value(&self.client_random_prefix);
        doc["tunnel_token"] = value(&self.tunnel_token);
        doc["skip_verification"] = value(self.skip_verification);
        if self.cert_is_system_verifiable {
            doc["certificate"] = value("");
        } else {
            doc["certificate"] = value(&self.certificate);
        }
        doc["upstream_protocol"] = value(&self.upstream_protocol);
        doc["anti_dpi"] = value(self.anti_dpi);
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
            hostname: self.hostname.clone(),
            addresses: self
                .addresses
                .iter()
                .map(ToString::to_string)
                .collect(),
            username: self.username.clone(),
            password: self.password.clone(),
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
        };

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
tunnel_token = ""

{}
skip_verification = false

{}
certificate = ""

{}
upstream_protocol = ""

{}
anti_dpi = false
"#,
        ClientConfig::doc_hostname().to_toml_comment(),
        ClientConfig::doc_addresses().to_toml_comment(),
        ClientConfig::doc_custom_sni().to_toml_comment(),
        ClientConfig::doc_has_ipv6().to_toml_comment(),
        ClientConfig::doc_username().to_toml_comment(),
        ClientConfig::doc_password().to_toml_comment(),
        ClientConfig::doc_client_random_prefix().to_toml_comment(),
        ClientConfig::doc_tunnel_token().to_toml_comment(),
        ClientConfig::doc_skip_verification().to_toml_comment(),
        ClientConfig::doc_certificate().to_toml_comment(),
        ClientConfig::doc_upstream_protocol().to_toml_comment(),
        ClientConfig::doc_anti_dpi().to_toml_comment(),
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
                tunnel_token: String::new(),
                skip_verification: false,
                certificate,
                cert_is_system_verifiable,
                upstream_protocol: "http2".into(),
                anti_dpi: false,
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
}
