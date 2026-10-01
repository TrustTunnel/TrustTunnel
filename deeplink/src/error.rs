use std::io;

/// Result type alias for deep-link operations.
pub type Result<T> = std::result::Result<T, DeepLinkError>;

/// Errors that can occur during deep-link encoding or decoding.
#[derive(Debug, thiserror::Error)]
pub enum DeepLinkError {
    #[error("Invalid base64url encoding: {0}")]
    InvalidBase64(String),

    #[error(
        "Truncated TLV entry: tag {tag:#04x} expects {expected} bytes but only {got} remaining"
    )]
    TruncatedTlv {
        tag: u8,
        expected: usize,
        got: usize,
    },

    #[error("Missing required field: {0}")]
    MissingRequiredField(&'static str),

    #[error("Invalid subscription URL '{0}': must use the https:// scheme")]
    InvalidSubscriptionUrl(String),

    #[error("Invalid protocol byte: {0:#04x} (expected 0x01 for http2 or 0x02 for http3)")]
    InvalidProtocol(u8),

    #[error("Unsupported deep link version: {found} (max supported: {max_supported})")]
    UnsupportedVersion { found: u64, max_supported: u64 },

    #[error("Truncated list entry: expected {expected} bytes but only {got} remaining")]
    TruncatedListEntry { expected: usize, got: usize },

    #[error("Varint value too large: {0} (max: 2^62-1)")]
    VarintOverflow(u64),

    #[error("Invalid certificate: {0}")]
    InvalidCertificate(String),

    #[error("Invalid UTF-8 string: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),

    #[error("Invalid address format: {0}")]
    InvalidAddress(String),

    #[error("Invalid boolean value: expected 0x00 or 0x01, got {0:#04x}")]
    InvalidBoolean(u8),

    #[error("Invalid URI scheme: expected 'tt://', got '{0}'")]
    InvalidScheme(String),

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Base64 decode error: {0}")]
    Base64Decode(#[from] base64::DecodeError),

    #[error("Address parse error: {0}")]
    AddrParse(#[from] std::net::AddrParseError),
}
