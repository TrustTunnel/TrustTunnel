use trusttunnel_deeplink::{decode, encode, DeepLinkConfig, DeepLinkError, Protocol};

#[test]
fn test_roundtrip_minimal_config() {
    let original = DeepLinkConfig::builder()
        .hostname("vpn.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("alice".to_string())
        .password("secret123".to_string())
        .build()
        .unwrap();

    let uri = encode(&original).unwrap();
    assert!(uri.starts_with("tt://?"));

    let decoded = decode(&uri).unwrap();

    assert_eq!(decoded.hostname, original.hostname);
    assert_eq!(decoded.addresses, original.addresses);
    assert_eq!(decoded.username, original.username);
    assert_eq!(decoded.password, original.password);
    assert_eq!(decoded.has_ipv6, original.has_ipv6);
    assert_eq!(decoded.upstream_protocol, original.upstream_protocol);
    assert_eq!(decoded.anti_dpi, original.anti_dpi);
}

#[test]
fn test_roundtrip_maximal_config() {
    let original = DeepLinkConfig::builder()
        .hostname("secure.vpn.example.com".to_string())
        .addresses(vec![
            "192.168.1.1:8443".to_string(),
            "10.0.0.1:443".to_string(),
        ])
        .username("premium_user".to_string())
        .password("very_secret_password_123".to_string())
        .custom_sni(Some("cdn.example.org".to_string()))
        .has_ipv6(false)
        .skip_verification(true)
        .certificate(Some(vec![0x30, 0x82, 0x01, 0x23]))
        .upstream_protocol(Protocol::Http3)
        .anti_dpi(true)
        .build()
        .unwrap();

    let uri = encode(&original).unwrap();
    let decoded = decode(&uri).unwrap();

    assert_eq!(decoded.hostname, original.hostname);
    assert_eq!(decoded.addresses, original.addresses);
    assert_eq!(decoded.username, original.username);
    assert_eq!(decoded.password, original.password);
    assert_eq!(decoded.custom_sni, original.custom_sni);
    assert_eq!(decoded.has_ipv6, original.has_ipv6);
    assert_eq!(decoded.skip_verification, original.skip_verification);
    assert_eq!(decoded.certificate, original.certificate);
    assert_eq!(decoded.upstream_protocol, original.upstream_protocol);
    assert_eq!(decoded.anti_dpi, original.anti_dpi);
}

#[test]
fn test_roundtrip_with_certificate() {
    let cert_der = vec![
        0x30, 0x82, 0x03, 0x52, 0x30, 0x82, 0x02, 0x3A, 0xA0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x09,
        0x00,
    ];

    let original = DeepLinkConfig::builder()
        .hostname("vpn.secure.com".to_string())
        .addresses(vec!["203.0.113.1:443".to_string()])
        .username("user".to_string())
        .password("pass".to_string())
        .certificate(Some(cert_der.clone()))
        .build()
        .unwrap();

    let uri = encode(&original).unwrap();
    let decoded = decode(&uri).unwrap();

    assert_eq!(decoded.certificate, Some(cert_der));
}

#[test]
fn test_roundtrip_without_certificate() {
    let original = DeepLinkConfig::builder()
        .hostname("vpn.trusted.com".to_string())
        .addresses(vec!["198.51.100.1:443".to_string()])
        .username("user".to_string())
        .password("pass".to_string())
        .certificate(None)
        .build()
        .unwrap();

    let uri = encode(&original).unwrap();
    let decoded = decode(&uri).unwrap();

    assert_eq!(decoded.certificate, None);
}

#[test]
fn test_roundtrip_multiple_addresses() {
    let original = DeepLinkConfig::builder()
        .hostname("multi.vpn.com".to_string())
        .addresses(vec![
            "1.1.1.1:443".to_string(),
            "8.8.8.8:8443".to_string(),
            "9.9.9.9:9443".to_string(),
        ])
        .username("multiaddr".to_string())
        .password("test123".to_string())
        .build()
        .unwrap();

    let uri = encode(&original).unwrap();
    let decoded = decode(&uri).unwrap();

    assert_eq!(decoded.addresses.len(), 3);
    assert_eq!(decoded.addresses, original.addresses);
}

#[test]
fn test_roundtrip_long_values() {
    let long_password = "a".repeat(200);
    let long_hostname = format!("{}.vpn.example.com", "sub".repeat(50));

    let original = DeepLinkConfig::builder()
        .hostname(long_hostname.clone())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("user".to_string())
        .password(long_password.clone())
        .build()
        .unwrap();

    let uri = encode(&original).unwrap();
    let decoded = decode(&uri).unwrap();

    assert_eq!(decoded.hostname.as_deref(), Some(long_hostname.as_str()));
    assert_eq!(decoded.password.as_deref(), Some(long_password.as_str()));
}

#[test]
fn test_roundtrip_special_characters() {
    let original = DeepLinkConfig::builder()
        .hostname("vpn.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("user@example.com".to_string())
        .password("p@ss!w0rd#123".to_string())
        .custom_sni(Some("cdn-123.example.org".to_string()))
        .build()
        .unwrap();

    let uri = encode(&original).unwrap();
    let decoded = decode(&uri).unwrap();

    assert_eq!(decoded.username, original.username);
    assert_eq!(decoded.password, original.password);
    assert_eq!(decoded.custom_sni, original.custom_sni);
}

#[test]
fn test_roundtrip_ipv6_addresses() {
    let original = DeepLinkConfig::builder()
        .hostname("vpn6.example.com".to_string())
        .addresses(vec![
            "[2001:db8::1]:443".to_string(),
            "[::1]:8443".to_string(),
        ])
        .username("ipv6user".to_string())
        .password("ipv6pass".to_string())
        .build()
        .unwrap();

    let uri = encode(&original).unwrap();
    let decoded = decode(&uri).unwrap();

    assert_eq!(decoded.addresses, original.addresses);
}

#[test]
fn test_roundtrip_default_values_omitted() {
    let config = DeepLinkConfig::builder()
        .hostname("vpn.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("user".to_string())
        .password("pass".to_string())
        .has_ipv6(true) // default value
        .skip_verification(false) // default value
        .upstream_protocol(Protocol::Http2) // default value
        .anti_dpi(false) // default value
        .build()
        .unwrap();

    let uri = encode(&config).unwrap();
    let decoded = decode(&uri).unwrap();

    // All defaults should be preserved
    assert!(decoded.has_ipv6);
    assert!(!decoded.skip_verification);
    assert_eq!(decoded.upstream_protocol, Protocol::Http2);
    assert!(!decoded.anti_dpi);
}

#[test]
fn test_roundtrip_non_default_values() {
    let config = DeepLinkConfig::builder()
        .hostname("vpn.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("user".to_string())
        .password("pass".to_string())
        .has_ipv6(false) // non-default
        .skip_verification(true) // non-default
        .upstream_protocol(Protocol::Http3) // non-default
        .anti_dpi(true) // non-default
        .build()
        .unwrap();

    let uri = encode(&config).unwrap();
    let decoded = decode(&uri).unwrap();

    // All non-defaults should be preserved
    assert!(!decoded.has_ipv6);
    assert!(decoded.skip_verification);
    assert_eq!(decoded.upstream_protocol, Protocol::Http3);
    assert!(decoded.anti_dpi);
}

#[test]
fn test_roundtrip_with_client_random_prefix() {
    let config = DeepLinkConfig::builder()
        .hostname("crp.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("testuser".to_string())
        .password("testpass".to_string())
        .client_random_prefix(Some("aabbcc".to_string()))
        .build()
        .unwrap();

    let uri = encode(&config).unwrap();
    let decoded = decode(&uri).unwrap();

    assert_eq!(decoded.hostname.as_deref(), Some("crp.example.com"));
    assert_eq!(decoded.username.as_deref(), Some("testuser"));
    assert_eq!(decoded.password.as_deref(), Some("testpass"));
    assert_eq!(decoded.client_random_prefix, Some("aabbcc".to_string()));
}

#[test]
fn test_roundtrip_without_client_random_prefix() {
    let config = DeepLinkConfig::builder()
        .hostname("nocrp.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("testuser".to_string())
        .password("testpass".to_string())
        .client_random_prefix(None)
        .build()
        .unwrap();

    let uri = encode(&config).unwrap();
    let decoded = decode(&uri).unwrap();

    assert_eq!(decoded.hostname.as_deref(), Some("nocrp.example.com"));
    assert_eq!(decoded.client_random_prefix, None);
}

#[test]
fn test_invalid_hex_client_random_prefix() {
    let result = DeepLinkConfig::builder()
        .hostname("test.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("testuser".to_string())
        .password("testpass".to_string())
        .client_random_prefix(Some("notvalidhex".to_string()))
        .build();

    assert!(result.is_err());
}

#[test]
fn test_roundtrip_with_name() {
    let config = DeepLinkConfig::builder()
        .hostname("vpn.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("user".to_string())
        .password("pass".to_string())
        .name(Some("My VPN Server".to_string()))
        .build()
        .unwrap();

    let uri = encode(&config).unwrap();
    let decoded = decode(&uri).unwrap();

    assert_eq!(decoded.name, Some("My VPN Server".to_string()));
}

#[test]
fn test_roundtrip_with_dns_upstreams() {
    let config = DeepLinkConfig::builder()
        .hostname("vpn.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("user".to_string())
        .password("pass".to_string())
        .dns_upstreams(vec![
            "1.1.1.1".to_string(),
            "tls://dns.example.com".to_string(),
            "https://dns.example.com/dns-query".to_string(),
        ])
        .build()
        .unwrap();

    let uri = encode(&config).unwrap();
    let decoded = decode(&uri).unwrap();

    assert_eq!(
        decoded.dns_upstreams,
        vec![
            "1.1.1.1",
            "tls://dns.example.com",
            "https://dns.example.com/dns-query"
        ]
    );
}

#[test]
fn test_roundtrip_without_new_optional_fields() {
    let config = DeepLinkConfig::builder()
        .hostname("vpn.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("user".to_string())
        .password("pass".to_string())
        .build()
        .unwrap();

    let uri = encode(&config).unwrap();
    let decoded = decode(&uri).unwrap();

    assert_eq!(decoded.name, None);
    assert!(decoded.dns_upstreams.is_empty());
}

#[test]
fn test_unsupported_version_rejected() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use trusttunnel_deeplink::encode_varint;

    // Build a payload with version=99 (unsupported)
    let mut payload = Vec::new();
    // Tag 0x00 (Version)
    payload.extend(encode_varint(0x00).unwrap());
    let version_bytes = encode_varint(99).unwrap();
    payload.extend(encode_varint(version_bytes.len() as u64).unwrap());
    payload.extend(&version_bytes);
    // Tag 0x01 (Hostname)
    payload.extend(encode_varint(0x01).unwrap());
    payload.extend(encode_varint(3).unwrap());
    payload.extend(b"vpn");

    let encoded = URL_SAFE_NO_PAD.encode(&payload);
    let uri = format!("tt://?{}", encoded);
    let result = decode(&uri);

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        DeepLinkError::UnsupportedVersion {
            found: 99,
            max_supported: 2
        }
    ));
}

#[test]
fn test_roundtrip_full_config_with_subscription_url() {
    let url = "https://alice:s3cret@vpn.example.com/subscription".to_string();
    let original = DeepLinkConfig::builder()
        .hostname("vpn.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("alice".to_string())
        .password("s3cret".to_string())
        .name(Some("Acme VPN".to_string()))
        .dns_upstreams(vec!["tls://1.1.1.1".to_string()])
        .subscription_url(Some(url.clone()))
        .build()
        .unwrap();

    let uri = encode(&original).unwrap();
    let decoded = decode(&uri).unwrap();

    // Subscription URL and all static fallback fields survive the roundtrip.
    assert_eq!(decoded.subscription_url, Some(url));
    assert_eq!(decoded, original);
}

#[test]
fn test_roundtrip_subscription_only() {
    let url = "https://alice:s3cret@vpn.example.com/subscription".to_string();
    let original = DeepLinkConfig::builder()
        .subscription_url(Some(url.clone()))
        .build()
        .unwrap();

    let uri = encode(&original).unwrap();
    let decoded = decode(&uri).unwrap();

    assert_eq!(decoded.subscription_url, Some(url));
    assert_eq!(decoded.hostname, None);
    assert!(decoded.addresses.is_empty());
    assert_eq!(decoded.username, None);
    assert_eq!(decoded.password, None);
    assert_eq!(decoded.name, None);
    assert!(decoded.dns_upstreams.is_empty());
    // Defaults still apply.
    assert!(decoded.has_ipv6);
    assert_eq!(decoded.upstream_protocol, Protocol::Http2);
}

#[test]
fn test_build_rejects_http_subscription_url() {
    let result = DeepLinkConfig::builder()
        .hostname("vpn.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("alice".to_string())
        .password("s3cret".to_string())
        .subscription_url(Some("http://vpn.example.com/subscription".to_string()))
        .build();

    assert!(matches!(
        result.unwrap_err(),
        DeepLinkError::InvalidSubscriptionUrl(_)
    ));
}
