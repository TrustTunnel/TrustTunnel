use crate::error::{DeepLinkError, Result};
use crate::types::{DeepLinkConfig, Protocol, TlvTag};
use crate::varint::decode_varint;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

/// Decode a string from UTF-8 bytes.
fn decode_string(data: &[u8]) -> Result<String> {
    String::from_utf8(data.to_vec()).map_err(DeepLinkError::InvalidUtf8)
}

/// Decode a boolean from a single byte (0x00 = false, 0x01 = true).
fn decode_bool(data: &[u8]) -> Result<bool> {
    if data.len() != 1 {
        return Err(DeepLinkError::InvalidBoolean(
            data.first().copied().unwrap_or(0xFF),
        ));
    }
    match data[0] {
        0x00 => Ok(false),
        0x01 => Ok(true),
        byte => Err(DeepLinkError::InvalidBoolean(byte)),
    }
}

/// Decode a protocol from a single byte.
fn decode_protocol(data: &[u8]) -> Result<Protocol> {
    if data.len() != 1 {
        return Err(DeepLinkError::InvalidProtocol(
            data.first().copied().unwrap_or(0xFF),
        ));
    }
    Protocol::from_u8(data[0])
}

/// TLV parser with stateful offset tracking.
struct TlvParser<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> TlvParser<'a> {
    fn new(data: &'a [u8]) -> Self {
        TlvParser { data, offset: 0 }
    }

    /// Parse the next TLV field, returning (tag, value_bytes).
    /// Returns None when end of data is reached.
    fn next_field(&mut self) -> Option<Result<(Option<TlvTag>, Vec<u8>)>> {
        if self.offset >= self.data.len() {
            return None;
        }

        // Decode tag
        let (tag_u64, new_offset) = match decode_varint(self.data, self.offset) {
            Ok(result) => result,
            Err(e) => return Some(Err(e.into())),
        };
        self.offset = new_offset;

        let tag = TlvTag::from_u8(tag_u64 as u8);

        // Decode length
        let (length, new_offset) = match decode_varint(self.data, self.offset) {
            Ok(result) => result,
            Err(e) => return Some(Err(e.into())),
        };
        self.offset = new_offset;

        let length = length as usize;

        // Check if we have enough data
        if self.offset + length > self.data.len() {
            return Some(Err(DeepLinkError::TruncatedTlv {
                tag: tag_u64 as u8,
                expected: length,
                got: self.data.len() - self.offset,
            }));
        }

        // Extract value
        let value = self.data[self.offset..self.offset + length].to_vec();
        self.offset += length;

        Some(Ok((tag, value)))
    }
}

/// Decode a TLV binary payload into a DeepLinkConfig.
pub fn decode_tlv_payload(payload: &[u8]) -> Result<DeepLinkConfig> {
    let mut parser = TlvParser::new(payload);

    let mut hostname: Option<String> = None;
    let mut addresses: Vec<String> = Vec::new();
    let mut username: Option<String> = None;
    let mut password: Option<String> = None;
    let mut custom_sni: Option<String> = None;
    let mut has_ipv6: bool = true; // default
    let mut skip_verification: bool = false; // default
    let mut certificate: Option<Vec<u8>> = None;
    let mut upstream_protocol: Protocol = Protocol::Http2; // default
    let mut anti_dpi: bool = false; // default
    let mut client_random_prefix: Option<String> = None;

    while let Some(field_result) = parser.next_field() {
        let (tag_opt, value) = field_result?;

        // Unknown tags are ignored per spec (forward compatibility)
        let tag = match tag_opt {
            Some(t) => t,
            None => continue,
        };

        match tag {
            TlvTag::Hostname => {
                hostname = Some(decode_string(&value)?);
            }
            TlvTag::Address => {
                addresses.push(decode_string(&value)?);
            }
            TlvTag::CustomSni => {
                custom_sni = Some(decode_string(&value)?);
            }
            TlvTag::HasIpv6 => {
                has_ipv6 = decode_bool(&value)?;
            }
            TlvTag::Username => {
                username = Some(decode_string(&value)?);
            }
            TlvTag::Password => {
                password = Some(decode_string(&value)?);
            }
            TlvTag::SkipVerification => {
                skip_verification = decode_bool(&value)?;
            }
            TlvTag::Certificate => {
                certificate = Some(value);
            }
            TlvTag::UpstreamProtocol => {
                upstream_protocol = decode_protocol(&value)?;
            }
            TlvTag::AntiDpi => {
                anti_dpi = decode_bool(&value)?;
            }
            TlvTag::ClientRandomPrefix => {
                let prefix = decode_string(&value)?;
                // Validate hex format
                let (prefix_part, mask_part) = prefix.split_once('/').unwrap_or((&prefix, ""));
                hex::decode(prefix_part).map_err(|e| {
                    DeepLinkError::InvalidAddress(format!(
                        "client_random_prefix must be valid hex: {}",
                        e
                    ))
                })?;
                hex::decode(mask_part).map_err(|e| {
                    DeepLinkError::InvalidAddress(format!(
                        "client_random_prefix mask must be valid hex: {}",
                        e
                    ))
                })?;
                client_random_prefix = Some(prefix);
            }
        }
    }

    // Validate required fields
    let hostname = hostname.ok_or(DeepLinkError::MissingRequiredField("hostname"))?;
    if addresses.is_empty() {
        return Err(DeepLinkError::MissingRequiredField("addresses"));
    }
    let username = username.ok_or(DeepLinkError::MissingRequiredField("username"))?;
    let password = password.ok_or(DeepLinkError::MissingRequiredField("password"))?;

    let config = DeepLinkConfig {
        hostname,
        addresses,
        username,
        password,
        client_random_prefix,
        custom_sni,
        has_ipv6,
        skip_verification,
        certificate,
        upstream_protocol,
        anti_dpi,
    };

    config.validate()?;
    Ok(config)
}

/// Decode a deep-link URI into a configuration.
///
/// # Errors
///
/// Returns `DeepLinkError` if decoding fails (e.g., invalid URI format,
/// malformed TLV data, missing required fields).
pub fn decode(uri: &str) -> Result<DeepLinkConfig> {
    // Validate and strip scheme
    if !uri.starts_with("tt://") {
        return Err(DeepLinkError::InvalidScheme(uri.chars().take(20).collect()));
    }

    let encoded = &uri[5..]; // Strip "tt://"

    // Decode base64url
    let payload = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|e| DeepLinkError::InvalidBase64(e.to_string()))?;

    // Parse TLV payload
    decode_tlv_payload(&payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_string() {
        assert_eq!(decode_string(b"hello").unwrap(), "hello");
        assert!(decode_string(&[0xFF, 0xFE]).is_err()); // Invalid UTF-8
    }

    #[test]
    fn test_decode_bool() {
        assert!(!decode_bool(&[0x00]).unwrap());
        assert!(decode_bool(&[0x01]).unwrap());
        assert!(decode_bool(&[0x02]).is_err());
        assert!(decode_bool(&[0x00, 0x01]).is_err()); // Wrong length
    }

    #[test]
    fn test_decode_protocol() {
        assert_eq!(decode_protocol(&[0x01]).unwrap(), Protocol::Http2);
        assert_eq!(decode_protocol(&[0x02]).unwrap(), Protocol::Http3);
        assert!(decode_protocol(&[0x03]).is_err());
    }

    #[test]
    fn test_decode_address_ip() {
        let addr = decode_string(b"1.2.3.4:443").unwrap();
        assert_eq!(addr, "1.2.3.4:443");
    }

    #[test]
    fn test_decode_address_domain() {
        let addr = decode_string(b"vpn.example.com:443").unwrap();
        assert_eq!(addr, "vpn.example.com:443");
    }

    #[test]
    fn test_tlv_parser() {
        // Create a simple TLV: tag=0x01, length=5, value="hello"
        let data = vec![0x01, 0x05, b'h', b'e', b'l', b'l', b'o'];
        let mut parser = TlvParser::new(&data);

        let (tag, value) = parser.next_field().unwrap().unwrap();
        assert_eq!(tag, Some(TlvTag::Hostname));
        assert_eq!(value, b"hello");

        assert!(parser.next_field().is_none());
    }

    #[test]
    fn test_tlv_parser_unknown_tag() {
        // Unknown tag 0x0C (12) should be parsed but returned as None
        // (0x0C is not a known tag, and fits in 1 byte since it's < 0x40)
        let data = vec![0x0C, 0x03, 0x01, 0x02, 0x03];
        let mut parser = TlvParser::new(&data);

        let (tag, value) = parser.next_field().unwrap().unwrap();
        assert_eq!(tag, None); // Unknown tag
        assert_eq!(value, vec![0x01, 0x02, 0x03]);
    }

    #[test]
    fn test_tlv_parser_truncated() {
        // Tag=0x01, length=10, but only 3 bytes of value
        let data = vec![0x01, 0x0A, 0x01, 0x02, 0x03];
        let mut parser = TlvParser::new(&data);

        let result = parser.next_field().unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_invalid_scheme() {
        let result = decode("http://example.com");
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_client_random_with_mask() {
        let data = vec![
            0x0B, 0x09, b'5', b'8', b'4', b'1', b'/', b'7', b'a', b'4', b'3',
        ];
        let mut parser = TlvParser::new(&data);

        let (tag, value) = parser.next_field().unwrap().unwrap();
        assert_eq!(tag, Some(TlvTag::ClientRandomPrefix));
        assert_eq!(value, b"5841/7a43");
    }
}
