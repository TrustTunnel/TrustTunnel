use crate::error::{DeepLinkError, Result};
use std::fmt;
use std::str::FromStr;

/// TLV tag identifiers (per DEEP_LINK.md specification)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TlvTag {
    Version = 0x00,
    Hostname = 0x01,
    Address = 0x02,
    CustomSni = 0x03,
    HasIpv6 = 0x04,
    Username = 0x05,
    Password = 0x06,
    SkipVerification = 0x07,
    Certificate = 0x08,
    UpstreamProtocol = 0x09,
    AntiDpi = 0x0A,
    ClientRandomPrefix = 0x0B,
    Name = 0x0C,
    DnsUpstreams = 0x0D,
    ClientRandomPskKey = 0x0E,
}

impl TlvTag {
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x00 => Some(TlvTag::Version),
            0x01 => Some(TlvTag::Hostname),
            0x02 => Some(TlvTag::Address),
            0x03 => Some(TlvTag::CustomSni),
            0x04 => Some(TlvTag::HasIpv6),
            0x05 => Some(TlvTag::Username),
            0x06 => Some(TlvTag::Password),
            0x07 => Some(TlvTag::SkipVerification),
            0x08 => Some(TlvTag::Certificate),
            0x09 => Some(TlvTag::UpstreamProtocol),
            0x0A => Some(TlvTag::AntiDpi),
            0x0B => Some(TlvTag::ClientRandomPrefix),
            0x0C => Some(TlvTag::Name),
            0x0D => Some(TlvTag::DnsUpstreams),
            0x0E => Some(TlvTag::ClientRandomPskKey),
            _ => None,
        }
    }
}

/// Protocol type for upstream connection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Default)]
pub enum Protocol {
    #[default]
    Http2 = 0x01,
    Http3 = 0x02,
}

impl Protocol {
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0x01 => Ok(Protocol::Http2),
            0x02 => Ok(Protocol::Http3),
            _ => Err(DeepLinkError::InvalidProtocol(value)),
        }
    }
}

impl FromStr for Protocol {
    type Err = DeepLinkError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "http2" => Ok(Protocol::Http2),
            "http3" => Ok(Protocol::Http3),
            _ => Err(DeepLinkError::InvalidAddress(format!(
                "unknown upstream_protocol: {}",
                s
            ))),
        }
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Protocol::Http2 => write!(f, "http2"),
            Protocol::Http3 => write!(f, "http3"),
        }
    }
}

pub const CURRENT_VERSION: u64 = 1;

/// TrustTunnel deep-link configuration.
///
/// This struct represents all configuration fields that can be encoded into
/// or decoded from a `tt://?` deep-link URI.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeepLinkConfig {
    pub hostname: String,
    pub addresses: Vec<String>,
    pub username: String,
    pub password: String,
    pub client_random_prefix: Option<String>,
    pub client_random_psk_key: Option<String>,
    pub custom_sni: Option<String>,
    pub has_ipv6: bool,
    pub skip_verification: bool,
    pub certificate: Option<Vec<u8>>,
    pub upstream_protocol: Protocol,
    pub anti_dpi: bool,
    pub name: Option<String>,
    pub dns_upstreams: Vec<String>,
}

impl DeepLinkConfig {
    /// Create a new builder for constructing a DeepLinkConfig.
    pub fn builder() -> DeepLinkConfigBuilder {
        DeepLinkConfigBuilder::default()
    }

    /// Validate that all required fields are present and valid.
    pub fn validate(&self) -> Result<()> {
        if self.hostname.is_empty() {
            return Err(DeepLinkError::MissingRequiredField("hostname"));
        }
        if self.addresses.is_empty() {
            return Err(DeepLinkError::MissingRequiredField("addresses"));
        }
        if self.username.is_empty() {
            return Err(DeepLinkError::MissingRequiredField("username"));
        }
        if self.password.is_empty() {
            return Err(DeepLinkError::MissingRequiredField("password"));
        }
        Ok(())
    }
}

/// Builder for constructing a DeepLinkConfig.
#[derive(Debug, Default)]
pub struct DeepLinkConfigBuilder {
    hostname: Option<String>,
    addresses: Option<Vec<String>>,
    username: Option<String>,
    password: Option<String>,
    client_random_prefix: Option<String>,
    client_random_psk_key: Option<String>,
    custom_sni: Option<String>,
    has_ipv6: Option<bool>,
    skip_verification: Option<bool>,
    certificate: Option<Vec<u8>>,
    upstream_protocol: Option<Protocol>,
    anti_dpi: Option<bool>,
    name: Option<String>,
    dns_upstreams: Option<Vec<String>>,
}

impl DeepLinkConfigBuilder {
    pub fn hostname(mut self, hostname: String) -> Self {
        self.hostname = Some(hostname);
        self
    }

    pub fn addresses(mut self, addresses: Vec<String>) -> Self {
        self.addresses = Some(addresses);
        self
    }

    pub fn username(mut self, username: String) -> Self {
        self.username = Some(username);
        self
    }

    pub fn password(mut self, password: String) -> Self {
        self.password = Some(password);
        self
    }

    pub fn custom_sni(mut self, custom_sni: Option<String>) -> Self {
        self.custom_sni = custom_sni;
        self
    }

    pub fn has_ipv6(mut self, has_ipv6: bool) -> Self {
        self.has_ipv6 = Some(has_ipv6);
        self
    }

    pub fn skip_verification(mut self, skip_verification: bool) -> Self {
        self.skip_verification = Some(skip_verification);
        self
    }

    pub fn certificate(mut self, certificate: Option<Vec<u8>>) -> Self {
        self.certificate = certificate;
        self
    }

    pub fn upstream_protocol(mut self, upstream_protocol: Protocol) -> Self {
        self.upstream_protocol = Some(upstream_protocol);
        self
    }

    pub fn anti_dpi(mut self, anti_dpi: bool) -> Self {
        self.anti_dpi = Some(anti_dpi);
        self
    }

    pub fn client_random_prefix(mut self, client_random_prefix: Option<String>) -> Self {
        self.client_random_prefix = client_random_prefix;
        self
    }

    pub fn client_random_psk_key(mut self, client_random_psk_key: Option<String>) -> Self {
        self.client_random_psk_key = client_random_psk_key;
        self
    }

    pub fn name(mut self, name: Option<String>) -> Self {
        self.name = name;
        self
    }

    pub fn dns_upstreams(mut self, dns_upstreams: Vec<String>) -> Self {
        self.dns_upstreams = Some(dns_upstreams);
        self
    }

    pub fn build(self) -> Result<DeepLinkConfig> {
        // Validate client_random_prefix is valid hex if provided
        if let Some(ref prefix) = self.client_random_prefix {
            if !prefix.is_empty() {
                hex::decode(prefix).map_err(|e| {
                    DeepLinkError::InvalidAddress(format!(
                        "client_random_prefix must be valid hex: {}",
                        e
                    ))
                })?;
            }
        }

        // Validate client_random_psk_key is valid hex if provided
        if let Some(ref psk) = self.client_random_psk_key {
            if !psk.is_empty() {
                hex::decode(psk).map_err(|e| {
                    DeepLinkError::InvalidAddress(format!(
                        "client_random_psk_key must be valid hex: {}",
                        e
                    ))
                })?;
            }
        }

        let config = DeepLinkConfig {
            hostname: self
                .hostname
                .ok_or(DeepLinkError::MissingRequiredField("hostname"))?,
            addresses: self
                .addresses
                .ok_or(DeepLinkError::MissingRequiredField("addresses"))?,
            username: self
                .username
                .ok_or(DeepLinkError::MissingRequiredField("username"))?,
            password: self
                .password
                .ok_or(DeepLinkError::MissingRequiredField("password"))?,
            client_random_prefix: self.client_random_prefix,
            client_random_psk_key: self.client_random_psk_key,
            custom_sni: self.custom_sni,
            has_ipv6: self.has_ipv6.unwrap_or(true),
            skip_verification: self.skip_verification.unwrap_or(false),
            certificate: self.certificate,
            upstream_protocol: self.upstream_protocol.unwrap_or_default(),
            anti_dpi: self.anti_dpi.unwrap_or(false),
            name: self.name,
            dns_upstreams: self.dns_upstreams.unwrap_or_default(),
        };
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tlv_tag_conversions() {
        assert_eq!(TlvTag::Hostname.as_u8(), 0x01);
        assert_eq!(TlvTag::from_u8(0x01), Some(TlvTag::Hostname));
        assert_eq!(TlvTag::from_u8(0x00), Some(TlvTag::Version));
        assert_eq!(TlvTag::from_u8(0xFF), None);
    }

    #[test]
    fn test_protocol_conversions() {
        assert_eq!(Protocol::Http2.as_u8(), 0x01);
        assert_eq!(Protocol::from_u8(0x01).unwrap(), Protocol::Http2);
        assert!(Protocol::from_u8(0xFF).is_err());
    }

    #[test]
    fn test_protocol_from_str() {
        assert_eq!("http2".parse::<Protocol>().unwrap(), Protocol::Http2);
        assert_eq!("http3".parse::<Protocol>().unwrap(), Protocol::Http3);
        assert!("http1".parse::<Protocol>().is_err());
    }

    #[test]
    fn test_protocol_display() {
        assert_eq!(Protocol::Http2.to_string(), "http2");
        assert_eq!(Protocol::Http3.to_string(), "http3");
    }

    #[test]
    fn test_builder_success() {
        let config = DeepLinkConfig::builder()
            .hostname("vpn.example.com".to_string())
            .addresses(vec!["1.2.3.4:443".to_string()])
            .username("alice".to_string())
            .password("secret".to_string())
            .build()
            .unwrap();

        assert_eq!(config.hostname, "vpn.example.com");
        assert_eq!(config.addresses.len(), 1);
        assert!(config.has_ipv6);
        assert_eq!(config.upstream_protocol, Protocol::Http2);
    }

    #[test]
    fn test_builder_missing_required_field() {
        let result = DeepLinkConfig::builder()
            .hostname("vpn.example.com".to_string())
            .username("alice".to_string())
            .password("secret".to_string())
            .build();

        assert!(result.is_err());
    }

    #[test]
    fn test_builder_domain_address() {
        let config = DeepLinkConfig::builder()
            .hostname("vpn.example.com".to_string())
            .addresses(vec!["vpn.example.com:443".to_string()])
            .username("alice".to_string())
            .password("secret".to_string())
            .build()
            .unwrap();
        assert_eq!(config.addresses[0], "vpn.example.com:443");
    }

    #[test]
    fn test_builder_rejects_invalid_psk_hex() {
        let result = DeepLinkConfig::builder()
            .hostname("vpn.example.com".to_string())
            .addresses(vec!["1.2.3.4:443".to_string()])
            .username("alice".to_string())
            .password("secret".to_string())
            .client_random_psk_key(Some("not-hex".to_string()))
            .build();

        assert!(result.is_err());
    }

    #[test]
    fn test_builder_psk_key_empty_is_allowed() {
        let config = DeepLinkConfig::builder()
            .hostname("vpn.example.com".to_string())
            .addresses(vec!["1.2.3.4:443".to_string()])
            .username("alice".to_string())
            .password("secret".to_string())
            .client_random_psk_key(Some(String::new()))
            .build()
            .unwrap();
        assert_eq!(config.client_random_psk_key, Some(String::new()));
    }

    #[test]
    fn test_validate_empty_hostname() {
        let config = DeepLinkConfig {
            hostname: String::new(),
            addresses: vec!["1.2.3.4:443".to_string()],
            username: "alice".to_string(),
            password: "secret".to_string(),
            custom_sni: None,
            has_ipv6: true,
            skip_verification: false,
            certificate: None,
            upstream_protocol: Protocol::Http2,
            anti_dpi: false,
            client_random_prefix: None,
            client_random_psk_key: None,
            name: None,
            dns_upstreams: vec![],
        };

        assert!(config.validate().is_err());
    }
}
