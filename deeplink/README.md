# TrustTunnel Deep-Link Library

A standalone Rust library for encoding and decoding TrustTunnel configuration deep-links using the `tt` URI scheme.

## Features

- **Complete TLV encoding/decoding** - Implements the full [TrustTunnel deep-link specification](../DEEP_LINK.md)
- **Error handling** - Comprehensive error types with helpful messages
- **Base64url encoding** - URL-safe, compact representation
- **Certificate support** - PEM/DER conversion utilities
- **Property-based testing** - Verified with proptest for correctness

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
trusttunnel-deeplink = { git = "https://github.com/TrustTunnel/TrustTunnel/" }
```

## Quick Start

### Encoding a Configuration

```rust
use trusttunnel_deeplink::{encode, DeepLinkConfig};
use std::net::SocketAddr;

let config = DeepLinkConfig::builder()
    .hostname("vpn.example.com".to_string())
    .addresses(vec!["1.2.3.4:443".parse::<SocketAddr>().unwrap()])
    .username("alice".to_string())
    .password("secret123".to_string())
    .build()
    .unwrap();

let uri = encode(&config).unwrap();
println!("Deep-link: {}", uri);
// Output: tt://?AQ92cG4uZXhhbXBsZS5jb20CAzEuMi4zLjQ6NDQzBQVhbGljZQYJc2VjcmV0MTIz
```

### Decoding a Deep-Link

```rust
use trusttunnel_deeplink::decode;

let uri = "tt://?AQ92cG4uZXhhbXBsZS5jb20CAzEuMi4zLjQ6NDQzBQVhbGljZQYJc2VjcmV0MTIz";
let config = decode(uri).unwrap();

println!("Hostname: {:?}", config.hostname);
println!("Username: {:?}", config.username);
```

## Configuration Fields

The `DeepLinkConfig` struct supports the following fields:

| Field | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `hostname` | `Option<String>` | Yes * | None | Server hostname |
| `addresses` | `Vec<String>` | Yes * | `[]` | Server addresses (`host:port`) |
| `username` | `Option<String>` | Yes * | None | Authentication username |
| `password` | `Option<String>` | Yes * | None | Authentication password |
| `custom_sni` | `Option<String>` | No | None | Custom SNI for TLS |
| `has_ipv6` | `bool` | No | `true` | IPv6 support enabled |
| `skip_verification` | `bool` | No | `false` | Skip certificate verification |
| `certificate` | `Option<Vec<u8>>` | No | None | DER-encoded certificate chain |
| `upstream_protocol` | `Protocol` | No | `Http2` | Upstream protocol (HTTP/2 or HTTP/3) |
| `anti_dpi` | `bool` | No | `false` | Anti-DPI measures enabled |
| `client_random_prefix` | `Option<String>` | No | None | TLS client random hex prefix |
| `name` | `Option<String>` | No | None | Human-readable server name |
| `dns_upstreams` | `Vec<String>` | No | `[]` | DNS upstream addresses |
| `subscription_url` | `Option<String>` | No | None | HTTPS subscription URL (v2) |

\* Required unless `subscription_url` is present (format version 2); when it
is, the static connection parameters are optional fallback.

## Advanced Usage

### Working with Certificates

```rust
use trusttunnel_deeplink::cert::{pem_to_der, der_to_pem};

// Convert PEM certificate to DER
let pem = "-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----";
let der = pem_to_der(pem).unwrap();

// Convert DER back to PEM
let pem_again = der_to_pem(&der).unwrap();
```

### Builder Pattern

```rust
use trusttunnel_deeplink::{DeepLinkConfig, Protocol};

let config = DeepLinkConfig::builder()
    .hostname("vpn.example.com".to_string())
    .addresses(vec!["1.2.3.4:443".parse().unwrap()])
    .username("user".to_string())
    .password("pass".to_string())
    .custom_sni(Some("cdn.example.org".to_string()))
    .has_ipv6(false)
    .upstream_protocol(Protocol::Http3)
    .anti_dpi(true)
    .build()
    .unwrap();
```

## Error Handling

The library uses a custom `DeepLinkError` type with specific error variants:

```rust
use trusttunnel_deeplink::{decode, DeepLinkError};

match decode("invalid://uri") {
    Ok(config) => println!("Success!"),
    Err(DeepLinkError::InvalidScheme(scheme)) => {
        println!("Invalid URI scheme: {}", scheme);
    }
    Err(DeepLinkError::MissingRequiredField(field)) => {
        println!("Missing required field: {}", field);
    }
    Err(e) => println!("Other error: {}", e),
}
```

## Testing

Run the test suite:

```bash
# Unit tests
cargo test -p trusttunnel-deeplink

# Integration tests (roundtrip)
cargo test -p trusttunnel-deeplink --test roundtrip

# Python compatibility tests
cargo test -p trusttunnel-deeplink --test python_compat

# Property-based tests
cargo test -p trusttunnel-deeplink --test proptest

```
