use proptest::prelude::*;
use trusttunnel_deeplink::{decode, encode, DeepLinkConfig, Protocol};

fn arbitrary_address_string() -> impl Strategy<Value = String> {
    prop_oneof![
        (any::<[u8; 4]>(), 1u16..=65535)
            .prop_map(|(ip, port)| { format!("{}.{}.{}.{}:{}", ip[0], ip[1], ip[2], ip[3], port) }),
        "[a-z]{3,15}\\.[a-z]{2,10}\\.[a-z]{2,5}:[0-9]{2,5}",
    ]
}

fn arbitrary_protocol() -> impl Strategy<Value = Protocol> {
    prop_oneof![Just(Protocol::Http2), Just(Protocol::Http3),]
}

fn arbitrary_hex_string() -> impl Strategy<Value = Option<String>> {
    prop::option::of("([0-9a-f]{2}){0,16}")
}

fn arbitrary_nonempty_hex_string() -> impl Strategy<Value = Option<String>> {
    prop::option::of("([0-9a-f]{2}){1,16}")
}

fn arbitrary_config() -> impl Strategy<Value = DeepLinkConfig> {
    (
        (
            "[a-z]{3,20}\\.[a-z]{3,10}\\.[a-z]{2,5}",
            prop::collection::vec(arbitrary_address_string(), 1..5),
            "[a-z0-9_]{3,20}",
            "[a-zA-Z0-9!@#$%]{8,30}",
            arbitrary_hex_string(),
            arbitrary_nonempty_hex_string(),
            prop::option::of("[a-z]{3,15}\\.[a-z]{2,10}\\.[a-z]{2,5}"),
            any::<bool>(),
            any::<bool>(),
            prop::option::of(prop::collection::vec(any::<u8>(), 0..100)),
            arbitrary_protocol(),
            any::<bool>(),
        ),
        prop::option::of("[a-z]{3,20}"),
        prop::collection::vec("[a-z0-9:/._-]{5,40}", 0..3),
    )
        .prop_map(
            |(
                (
                    hostname,
                    addresses,
                    username,
                    password,
                    client_random_prefix,
                    client_random_psk_key,
                    custom_sni,
                    has_ipv6,
                    skip_verification,
                    certificate,
                    upstream_protocol,
                    anti_dpi,
                ),
                name,
                dns_upstreams,
            )| {
                DeepLinkConfig {
                    hostname,
                    addresses,
                    username,
                    password,
                    client_random_prefix,
                    client_random_psk_key,
                    custom_sni,
                    has_ipv6,
                    skip_verification,
                    certificate,
                    upstream_protocol,
                    anti_dpi,
                    name,
                    dns_upstreams,
                }
            },
        )
}

proptest! {
    #[test]
    fn test_encode_decode_roundtrip(config in arbitrary_config()) {
        let uri = encode(&config).unwrap();
        let decoded = decode(&uri).unwrap();

        prop_assert_eq!(decoded.hostname, config.hostname);
        prop_assert_eq!(decoded.addresses, config.addresses);
        prop_assert_eq!(decoded.username, config.username);
        prop_assert_eq!(decoded.password, config.password);
        prop_assert_eq!(decoded.client_random_psk_key, config.client_random_psk_key);
        prop_assert_eq!(decoded.custom_sni, config.custom_sni);
        prop_assert_eq!(decoded.has_ipv6, config.has_ipv6);
        prop_assert_eq!(decoded.skip_verification, config.skip_verification);
        prop_assert_eq!(decoded.certificate, config.certificate);
        prop_assert_eq!(decoded.upstream_protocol, config.upstream_protocol);
        prop_assert_eq!(decoded.anti_dpi, config.anti_dpi);
        prop_assert_eq!(decoded.name, config.name);
        prop_assert_eq!(decoded.dns_upstreams, config.dns_upstreams);
    }

    #[test]
    fn test_uri_starts_with_scheme(config in arbitrary_config()) {
        let uri = encode(&config).unwrap();
        prop_assert!(uri.starts_with("tt://?"));
    }

    #[test]
    fn test_uri_no_padding(config in arbitrary_config()) {
        let uri = encode(&config).unwrap();
        prop_assert!(!uri.contains('='));
    }

    #[test]
    fn test_varint_roundtrip(value in 0u64..0x3FFFFFFFFFFFFFFF) {
        use trusttunnel_deeplink::varint::{encode_varint, decode_varint};

        let encoded = encode_varint(value).unwrap();
        let (decoded, _) = decode_varint(&encoded, 0).unwrap();
        prop_assert_eq!(decoded, value);
    }
}
