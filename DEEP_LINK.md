# TrustTunnel Deep Link Specification

This document describes the deep link URI scheme used to share TrustTunnel
endpoint configurations between devices and applications.

Status: draft.

---

## URI Format

```uri
tt://<base64url-encoded payload>
```

- **Scheme**: `tt`
- **Payload**: The endpoint configuration is serialized into a binary format,
  then encoded using [Base64url](https://datatracker.ietf.org/doc/html/rfc4648#section-5)
  (URL-safe Base64 without padding).

### Why Base64url?

Standard Base64 uses `+` and `/` characters that require percent-encoding in
URIs. Base64url replaces them with `-` and `_`, making the result safe to embed
directly in a URI without escaping. Padding (`=`) is omitted.

---

## Binary Payload Format

The payload is a compact binary encoding of the endpoint configuration fields
exported by `trusttunnel_endpoint`.

### Wire Layout

Each field is encoded as a **Tag–Length–Value (TLV)** entry:

| Component | Encoding | Description |
| --------- | -------- | ----------- |
| **Tag** | TLS varint | Field identifier (see table below) |
| **Length** | TLS varint | Byte length of the value that follows |
| **Value** | *Length* bytes | Field-specific payload |

A parser MUST ignore unknown tags to allow forward-compatible extensions.

### TLS Variable-Length Integer Encoding

Tag and Length use the variable-length integer encoding defined in
[RFC 9000 §16](https://www.rfc-editor.org/rfc/rfc9000.html#section-16)
(QUIC / TLS 1.3). The two most-significant bits of the first byte encode the
length of the integer:

| 2-MSB | Integer size | Usable bits | Max value |
| ----- | ------------ | ----------- | --------- |
| `00` | 1 byte | 6 | 63 |
| `01` | 2 bytes | 14 | 16 383 |
| `10` | 4 bytes | 30 | 1 073 741 823 |
| `11` | 8 bytes | 62 | 4 611 686 018 427 387 903 |

Multi-byte varints are in **network byte order** (big-endian). In practice,
current tags fit in a single byte (`00` prefix) and lengths under 16 384 fit
in one or two bytes.

### Field Tags

| Tag    | Field                  | Value encoding                                                                                       | Required             |
|--------|------------------------|------------------------------------------------------------------------------------------------------|----------------------|
| `0x01` | `hostname`             | UTF-8 string                                                                                         | yes                  |
| `0x02` | `addresses`            | UTF-8, one `address:port` per entry; multiple entries are encoded as separate TLVs with the same tag | yes                  |
| `0x03` | `custom_sni`           | UTF-8 string                                                                                         | no                   |
| `0x04` | `has_ipv6`             | 1 byte: `0x01` = true, `0x00` = false                                                                | no (default `true`)  |
| `0x05` | `username`             | UTF-8 string                                                                                         | yes                  |
| `0x06` | `password`             | UTF-8 string                                                                                         | yes                  |
| `0x0B` | `client_random_prefix` | UTF-8 hex-encoded string in the following format: `prefix[/mask]`                                    | no                   |
| `0x07` | `skip_verification`    | 1 byte: `0x01` = true, `0x00` = false                                                                | no (default `false`) |
| `0x08` | `certificate`          | Concatenated DER-encoded certificates (raw binary); omit if the chain is verified by system CAs      | no                   |
| `0x09` | `upstream_protocol`    | 1 byte: `0x01` = `http2`, `0x02` = `http3`                                                           | no (default `http2`) |
| `0x0A` | `anti_dpi`             | 1 byte: `0x01` = true, `0x00` = false                                                                | no (default `false`) |

### Encoding Rules

1. Fields MAY appear in any order.
2. Tag `0x02` (`addresses`) MAY appear more than once; each occurrence adds one
   address to the list. All other tags MUST appear at most once; if duplicated,
   the last occurrence wins.
3. Boolean fields that match their default value MAY be omitted to save space.
4. A parser MUST reject a payload that is missing any required field.

---

## Example

Given the following exported endpoint configuration:

```toml
hostname = "vpn.example.com"
addresses = ["1.2.3.4:443"]
custom_sni = "example.org"
has_ipv6 = true
username = "premium"
password = "s3cretPass"
skip_verification = false
certificate = """
-----BEGIN CERTIFICATE-----
MIIDijCCAxGgAwIBAgISBcSirIQr2Y8pK6reoWtJhyXZMAoGCCqGSM49BAMDMDIx
...
-----END CERTIFICATE-----
"""
upstream_protocol = "http2"
anti_dpi = false
```

### Encoding Steps

1. **Serialize** each field into TLV entries:

   ```text
   Tag=0x01  Len=15  Value="vpn.example.com"
   Tag=0x02  Len=11  Value="1.2.3.4:443"
   Tag=0x03  Len=11  Value="example.org"
   Tag=0x04  Len=1   Value=0x01              (has_ipv6 = true)
   Tag=0x05  Len=7   Value="premium"
   Tag=0x06  Len=10  Value="s3cretPass"
   Tag=0x08  Len=N   Value=<concatenated DER bytes of the certificate chain>
   Tag=0x09  Len=1   Value=0x01              (http2)
   ```

   Fields at their default value (`skip_verification = false`,
   `anti_dpi = false`) are omitted.

2. **Concatenate** all TLV entries into a single byte buffer.

3. **Base64url-encode** the buffer (no padding).

4. **Construct** the URI:

   ```uri
   tt://AQAL...  (full Base64url string)
   ```

---

## Versioning

The current encoding is **version 0** (implicit). If a breaking change to the
binary format is needed in the future, a reserved tag `0x00` will be used as a
version indicator:

| Tag | Field | Value encoding |
| --- | ----- | -------------- |
| `0x00` | `version` | 1 byte: format version number |

If the `0x00` tag is absent, parsers MUST assume version 0.

---

## Platform Integration

### Mobile (iOS / Android)

Register the `tt` scheme in the application manifest. When the OS dispatches a
deep link:

1. Strip the `tt://` prefix.
2. Base64url-decode the remainder.
3. Parse the TLV binary payload.
4. Populate the endpoint configuration and present it to the user for
   confirmation before connecting.

### Desktop (macOS / Windows / Linux)

The `tt://` URI can be passed as a command-line argument or handled via OS URI
scheme registration. The TrustTunnel client or setup wizard parses the payload
using the same decode logic.

### QR Codes

The `tt://` URI is short enough to be embedded in a QR code for easy scanning,
enabling zero-typing configuration sharing.

---

## Security Considerations

- **Credentials in the URI**: The deep link contains the `username` and
  `password` in cleartext (after decoding). Treat deep link URIs with the same
  care as passwords. Do not log or persist them unnecessarily.
- **Certificate pinning**: When `certificate` is present and
  `skip_verification` is `false`, the client MUST verify the endpoint
  certificate against the provided PEM chain.
- **User confirmation**: Clients SHOULD display the decoded configuration to the
  user and require explicit confirmation before establishing a connection.
- **URI length**: Very large PEM certificate chains may produce long URIs.
  Implementations should handle URIs up to at least 8 KiB. For QR code use,
  consider whether the certificate field can be omitted when the endpoint uses a
  publicly trusted CA.
