use crate::error::Result;
use crate::types::{DeepLinkConfig, Protocol, TlvTag, CURRENT_VERSION};
use crate::varint::encode_varint;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

static PREFIX: &str = "tt://?";

/// Encode a Tag-Length-Value entry.
fn encode_tlv(tag: TlvTag, value: &[u8]) -> Result<Vec<u8>> {
    let mut result = encode_varint(u64::from(tag.as_u8()))?;
    result.extend(encode_varint(value.len() as u64)?);
    result.extend_from_slice(value);
    Ok(result)
}

/// Encode a string field as TLV.
fn encode_string_field(tag: TlvTag, value: &str) -> Result<Vec<u8>> {
    encode_tlv(tag, value.as_bytes())
}

/// Encode a boolean field as TLV (1 byte: 0x01 for true, 0x00 for false).
fn encode_bool_field(tag: TlvTag, value: bool) -> Result<Vec<u8>> {
    encode_tlv(tag, if value { &[0x01] } else { &[0x00] })
}

/// Encode upstream protocol as TLV (1 byte: 0x01 for http2, 0x02 for http3).
fn encode_protocol_field(protocol: Protocol) -> Result<Vec<u8>> {
    encode_tlv(TlvTag::UpstreamProtocol, &[protocol.as_u8()])
}

/// Encode a String[] value: each element is a varint length followed by UTF-8 bytes.
fn encode_string_array(strings: &[String]) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    for s in strings {
        let bytes = s.as_bytes();
        buf.extend(encode_varint(bytes.len() as u64)?);
        buf.extend_from_slice(bytes);
    }
    Ok(buf)
}

/// Encode binary payload to base64url (URL-safe base64 without padding).
fn encode_base64url(payload: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(payload)
}

/// Encode a DeepLinkConfig into TLV binary payload.
pub fn encode_tlv_payload(config: &DeepLinkConfig) -> Result<Vec<u8>> {
    config.validate()?;

    let mut payload = Vec::new();

    // Version tag
    let version_bytes = encode_varint(CURRENT_VERSION)?;
    payload.extend(encode_tlv(TlvTag::Version, &version_bytes)?);

    // Static connection parameters: emitted when present (omitted in the
    // subscription-only variant). Order matches the Python reference
    // implementation.
    if let Some(ref hostname) = config.hostname {
        payload.extend(encode_string_field(TlvTag::Hostname, hostname)?);
    }
    if let Some(ref username) = config.username {
        payload.extend(encode_string_field(TlvTag::Username, username)?);
    }
    if let Some(ref password) = config.password {
        payload.extend(encode_string_field(TlvTag::Password, password)?);
    }

    for addr in &config.addresses {
        payload.extend(encode_string_field(TlvTag::Address, addr)?);
    }

    // client_random_prefix: include if present and non-empty
    if let Some(ref prefix) = config.client_random_prefix {
        if !prefix.is_empty() {
            payload.extend(encode_string_field(TlvTag::ClientRandomPrefix, prefix)?);
        }
    }

    // Optional fields (omit if default value or None) - order matches Python
    if let Some(custom_sni) = &config.custom_sni {
        payload.extend(encode_string_field(TlvTag::CustomSni, custom_sni)?);
    }

    // has_ipv6 default is true, so only encode if false
    if !config.has_ipv6 {
        payload.extend(encode_bool_field(TlvTag::HasIpv6, false)?);
    }

    // skip_verification default is false, so only encode if true
    if config.skip_verification {
        payload.extend(encode_bool_field(TlvTag::SkipVerification, true)?);
    }

    // anti_dpi default is false, so only encode if true
    if config.anti_dpi {
        payload.extend(encode_bool_field(TlvTag::AntiDpi, true)?);
    }

    // Certificate: include if present
    if let Some(cert_der) = &config.certificate {
        payload.extend(encode_tlv(TlvTag::Certificate, cert_der)?);
    }

    // upstream_protocol default is Http2, so only encode if Http3
    if config.upstream_protocol != Protocol::Http2 {
        payload.extend(encode_protocol_field(config.upstream_protocol)?);
    }

    // name (optional)
    if let Some(ref name) = config.name {
        payload.extend(encode_string_field(TlvTag::Name, name)?);
    }

    // dns_upstreams (optional, String[] encoding)
    if !config.dns_upstreams.is_empty() {
        let value = encode_string_array(&config.dns_upstreams)?;
        payload.extend(encode_tlv(TlvTag::DnsUpstreams, &value)?);
    }

    // subscription_url (optional, format v2)
    if let Some(ref url) = config.subscription_url {
        payload.extend(encode_string_field(TlvTag::SubscriptionUrl, url)?);
    }

    Ok(payload)
}

/// Encode a configuration into a deep-link URI (`tt://?...`).
///
/// # Errors
///
/// Returns `DeepLinkError` if encoding fails (e.g., missing required fields,
/// invalid data, varint overflow).
pub fn encode(config: &DeepLinkConfig) -> Result<String> {
    let payload = encode_tlv_payload(config)?;
    let encoded = encode_base64url(&payload);
    Ok(format!("{PREFIX}{}", encoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_tlv() {
        let result = encode_tlv(TlvTag::Hostname, b"example.com").unwrap();
        assert_eq!(result[0], 0x01); // tag
        assert_eq!(result[1], 11); // length
        assert_eq!(&result[2..], b"example.com");
    }

    #[test]
    fn test_encode_string_field() {
        let result = encode_string_field(TlvTag::Username, "alice").unwrap();
        assert_eq!(result[0], 0x05); // Username tag
        assert_eq!(result[1], 5); // length
        assert_eq!(&result[2..], b"alice");
    }

    #[test]
    fn test_encode_bool_field() {
        let result_true = encode_bool_field(TlvTag::HasIpv6, true).unwrap();
        assert_eq!(result_true[0], 0x04); // HasIpv6 tag
        assert_eq!(result_true[1], 1); // length
        assert_eq!(result_true[2], 0x01); // true

        let result_false = encode_bool_field(TlvTag::SkipVerification, false).unwrap();
        assert_eq!(result_false[0], 0x07); // SkipVerification tag
        assert_eq!(result_false[1], 1); // length
        assert_eq!(result_false[2], 0x00); // false
    }

    #[test]
    fn test_encode_protocol_field() {
        let result_http2 = encode_protocol_field(Protocol::Http2).unwrap();
        assert_eq!(result_http2[0], 0x09); // UpstreamProtocol tag
        assert_eq!(result_http2[1], 1); // length
        assert_eq!(result_http2[2], 0x01); // http2

        let result_http3 = encode_protocol_field(Protocol::Http3).unwrap();
        assert_eq!(result_http3[2], 0x02); // http3
    }

    #[test]
    fn test_encode_base64url() {
        let data = b"hello world";
        let encoded = encode_base64url(data);
        assert_eq!(encoded, "aGVsbG8gd29ybGQ");
        assert!(!encoded.contains('='));
    }

    #[test]
    fn test_encode_tlv_payload_minimal() {
        let config = DeepLinkConfig::builder()
            .hostname("vpn.example.com".to_string())
            .addresses(vec!["1.2.3.4:443".to_string()])
            .username("alice".to_string())
            .password("secret".to_string())
            .build()
            .unwrap();

        let payload = encode_tlv_payload(&config).unwrap();

        // Should contain required fields only (has_ipv6=true is default, so omitted)
        assert!(!payload.is_empty());
    }

    #[test]
    fn test_encode_tlv_payload_with_optional_fields() {
        let config = DeepLinkConfig::builder()
            .hostname("vpn.example.com".to_string())
            .addresses(vec!["1.2.3.4:443".to_string()])
            .username("alice".to_string())
            .password("secret".to_string())
            .custom_sni(Some("example.org".to_string()))
            .has_ipv6(false)
            .skip_verification(true)
            .upstream_protocol(Protocol::Http3)
            .anti_dpi(true)
            .build()
            .unwrap();

        let payload = encode_tlv_payload(&config).unwrap();

        // Should contain all fields
        assert!(!payload.is_empty());
    }

    #[test]
    fn test_encode_full_uri() {
        let config = DeepLinkConfig::builder()
            .hostname("vpn.example.com".to_string())
            .addresses(vec!["1.2.3.4:443".to_string()])
            .username("alice".to_string())
            .password("secret".to_string())
            .build()
            .unwrap();

        let uri = encode(&config).unwrap();

        assert!(uri.starts_with(PREFIX));
        assert!(!uri.contains('='));
    }

    #[test]
    fn test_encode_tlv_payload_subscription_only() {
        let url = "https://alice:s3cret@vpn.example.com/subscription";
        let config = DeepLinkConfig::builder()
            .subscription_url(Some(url.to_string()))
            .build()
            .unwrap();

        let payload = encode_tlv_payload(&config).unwrap();

        // Version-2 TLV (tag 0x00, len 1, value 2) comes first.
        assert_eq!(&payload[..3], &[0x00, 0x01, 0x02]);

        // The whole payload is exactly these two TLVs; anything appended
        // afterwards (e.g. a new default-emitted field) fails here.
        let mut tags = Vec::new();
        let mut offset = 0;
        while offset < payload.len() {
            let (tag, after_tag) = crate::varint::decode_varint(&payload, offset).unwrap();
            let (len, after_len) = crate::varint::decode_varint(&payload, after_tag).unwrap();
            tags.push(TlvTag::from_u8(tag as u8));
            offset = after_len + len as usize;
        }
        assert_eq!(tags, [Some(TlvTag::Version), Some(TlvTag::SubscriptionUrl)]);

        let decoded = crate::decode::decode_tlv_payload(&payload).unwrap();
        assert_eq!(decoded.subscription_url.as_deref(), Some(url));
        assert_eq!(decoded.hostname, None);
    }
}
