#!/usr/bin/env python3
"""Convert a tt://? deep link URI back to a TrustTunnel endpoint TOML config.

Usage:
    python3 deeplink_to_config.py <tt://?...>

See DEEP_LINK.md for the specification.
"""

from __future__ import annotations

import base64
import sys

# ---------------------------------------------------------------------------
# TLS variable-length integer decoding (RFC 9000 §16)
# ---------------------------------------------------------------------------

def decode_varint(data: bytes, offset: int) -> tuple[int, int]:
    """Decode a TLS/QUIC variable-length integer at *offset*.

    Returns (value, new_offset).
    """
    if offset >= len(data):
        raise ValueError("unexpected end of data while reading varint")
    first = data[offset]
    prefix = first >> 6
    if prefix == 0:
        return first & 0x3F, offset + 1
    if prefix == 1:
        if offset + 2 > len(data):
            raise ValueError("truncated 2-byte varint")
        return int.from_bytes(data[offset:offset + 2], "big") & 0x3FFF, offset + 2
    if prefix == 2:
        if offset + 4 > len(data):
            raise ValueError("truncated 4-byte varint")
        return int.from_bytes(data[offset:offset + 4], "big") & 0x3FFFFFFF, offset + 4
    # prefix == 3
    if offset + 8 > len(data):
        raise ValueError("truncated 8-byte varint")
    return int.from_bytes(data[offset:offset + 8], "big") & 0x3FFFFFFFFFFFFFFF, offset + 8

# ---------------------------------------------------------------------------
# Tag constants (must match config_to_deeplink.py)
# ---------------------------------------------------------------------------

TAG_HOSTNAME           = 0x01
TAG_ADDRESS            = 0x02
TAG_CUSTOM_SNI         = 0x03
TAG_HAS_IPV6           = 0x04
TAG_USERNAME           = 0x05
TAG_PASSWORD           = 0x06
TAG_SKIP_VERIFICATION  = 0x07
TAG_CERTIFICATE        = 0x08
TAG_UPSTREAM_PROTOCOL  = 0x09
TAG_ANTI_DPI           = 0x0A
TAG_CLIENT_RANDOM_PREFIX = 0x0B
TAG_NAME                   = 0x0C
TAG_DNS_UPSTREAMS          = 0x0D
TAG_SUBSCRIPTION_URL       = 0x0E

CURRENT_VERSION = 2

PROTOCOL_RMAP = {0x01: "http2", 0x02: "http3"}

DEFAULTS = {
    "has_ipv6": True,
    "skip_verification": False,
    "upstream_protocol": "http2",
    "anti_dpi": False,
}

# ---------------------------------------------------------------------------
# DER → PEM conversion
# ---------------------------------------------------------------------------

def _read_asn1_length(data: bytes, offset: int) -> tuple[int, int]:
    """Read an ASN.1 length at *offset*. Returns (length, new_offset)."""
    if offset >= len(data):
        raise ValueError("unexpected end of data in ASN.1 length")
    first = data[offset]
    if first < 0x80:
        return first, offset + 1
    num_bytes = first & 0x7F
    if num_bytes == 0 or offset + 1 + num_bytes > len(data):
        raise ValueError("invalid ASN.1 length encoding")
    length = int.from_bytes(data[offset + 1:offset + 1 + num_bytes], "big")
    return length, offset + 1 + num_bytes


def _split_der_certs(data: bytes) -> list[bytes]:
    """Split concatenated DER certificates into individual blobs."""
    certs: list[bytes] = []
    offset = 0
    while offset < len(data):
        if data[offset] != 0x30:
            raise ValueError(f"expected ASN.1 SEQUENCE (0x30) at offset {offset}, "
                             f"got 0x{data[offset]:02X}")
        body_len, hdr_end = _read_asn1_length(data, offset + 1)
        cert_end = hdr_end + body_len
        if cert_end > len(data):
            raise ValueError("truncated DER certificate")
        certs.append(data[offset:cert_end])
        offset = cert_end
    return certs


def der_to_pem(data: bytes) -> str:
    """Convert concatenated DER certificates to a PEM string."""
    certs = _split_der_certs(data)
    pem_blocks: list[str] = []
    for der in certs:
        b64 = base64.b64encode(der).decode("ascii")
        lines = [b64[i:i + 64] for i in range(0, len(b64), 64)]
        pem_blocks.append(
            "-----BEGIN CERTIFICATE-----\n"
            + "\n".join(lines)
            + "\n-----END CERTIFICATE-----"
        )
    return "\n".join(pem_blocks) + "\n"

# ---------------------------------------------------------------------------
# TLV parser
# ---------------------------------------------------------------------------

def parse_tlv(data: bytes) -> list[tuple[int, bytes]]:
    """Parse a sequence of TLV entries from *data*."""
    entries: list[tuple[int, bytes]] = []
    offset = 0
    while offset < len(data):
        tag, offset = decode_varint(data, offset)
        length, offset = decode_varint(data, offset)
        if offset + length > len(data):
            raise ValueError(f"TLV value truncated: tag=0x{tag:02X}, "
                             f"expected {length} bytes, got {len(data) - offset}")
        value = data[offset:offset + length]
        offset += length
        entries.append((tag, value))
    return entries

# ---------------------------------------------------------------------------
# Decoder
# ---------------------------------------------------------------------------

def _decode_string_array(data: bytes) -> list[str]:
    """Decode a String[] value: sequence of varint-length-prefixed UTF-8 strings."""
    result: list[str] = []
    offset = 0
    while offset < len(data):
        length, offset = decode_varint(data, offset)
        if offset + length > len(data):
            raise ValueError("truncated string in String[] value")
        result.append(data[offset:offset + length].decode())
        offset += length
    return result


def decode_config(data: bytes) -> dict:
    """Decode TLV binary payload into a config dict."""
    entries = parse_tlv(data)
    cfg: dict = {}
    addresses: list[str] = []

    for tag, value in entries:
        if tag == 0x00:
            version, _ = decode_varint(value, 0)
            if version > CURRENT_VERSION:
                raise ValueError(
                    f"unsupported deep link version: {version} "
                    f"(max supported: {CURRENT_VERSION})"
                )
        elif tag == TAG_HOSTNAME:
            cfg["hostname"] = value.decode()
        elif tag == TAG_ADDRESS:
            addresses.append(value.decode())
        elif tag == TAG_CUSTOM_SNI:
            cfg["custom_sni"] = value.decode()
        elif tag == TAG_HAS_IPV6:
            cfg["has_ipv6"] = value[0] != 0
        elif tag == TAG_USERNAME:
            cfg["username"] = value.decode()
        elif tag == TAG_PASSWORD:
            cfg["password"] = value.decode()
        elif tag == TAG_SKIP_VERIFICATION:
            cfg["skip_verification"] = value[0] != 0
        elif tag == TAG_CERTIFICATE:
            cfg["certificate"] = der_to_pem(value)
        elif tag == TAG_UPSTREAM_PROTOCOL:
            proto_byte = value[0]
            if proto_byte not in PROTOCOL_RMAP:
                raise ValueError(f"unknown upstream_protocol byte: 0x{proto_byte:02X}")
            cfg["upstream_protocol"] = PROTOCOL_RMAP[proto_byte]
        elif tag == TAG_ANTI_DPI:
            cfg["anti_dpi"] = value[0] != 0
        elif tag == TAG_CLIENT_RANDOM_PREFIX:
            cfg["client_random_prefix"] = value.decode()
        elif tag == TAG_NAME:
            cfg["name"] = value.decode()
        elif tag == TAG_DNS_UPSTREAMS:
            cfg["dns_upstreams"] = _decode_string_array(value)
        elif tag == TAG_SUBSCRIPTION_URL:
            cfg["subscription_url"] = value.decode()
        # Unknown tags are silently ignored per spec.

    if "subscription_url" in cfg and not cfg["subscription_url"].startswith("https://"):
        raise ValueError(
            f"subscription_url must be an https:// URL: {cfg['subscription_url']}"
        )

    if addresses:
        cfg["addresses"] = addresses

    # Apply defaults for omitted optional fields.
    for key, default in DEFAULTS.items():
        cfg.setdefault(key, default)

    # Certificate defaults to empty string when not present (verified by system CAs).
    cfg.setdefault("certificate", "")

    return cfg

# ---------------------------------------------------------------------------
# TOML emitter (minimal, no external dependency)
# ---------------------------------------------------------------------------

def _toml_value(v: object) -> str:
    """Format a single TOML value."""
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, str):
        if "\n" in v:
            return f'"""\n{v}"""'
        return f'"{v}"'
    if isinstance(v, list):
        inner = ", ".join(f'"{item}"' for item in v)
        return f"[{inner}]"
    return str(v)


# Ordered list of (key, comment) for pretty output.
_FIELD_ORDER: list[tuple[str, str]] = [
    ("hostname",          "Endpoint host name, used for TLS session establishment"),
    ("addresses",         "Endpoint addresses."),
    ("custom_sni",        "Custom SNI"),
    ("has_ipv6",          "Whether IPv6 traffic can be routed through the endpoint"),
    ("username",          "Username for authorization"),
    ("password",          "Password for authorization"),
    ("client_random_prefix", "TLS client random hex prefix for connection filtering"),
    ("skip_verification", "Skip the endpoint certificate verification?\n"
                          "# That is, any certificate is accepted with this one set to true."),
    ("certificate",       "Endpoint certificate in PEM format.\n"
                          "# If not specified, the endpoint certificate is verified "
                          "using the system storage."),
    ("upstream_protocol", "Protocol to be used to communicate with the endpoint [http2, http3]"),
    ("anti_dpi",          "Is anti-DPI measures should be enabled"),
    ("name",              "Human-readable server display name"),
    ("dns_upstreams",     "DNS upstreams to use when connected to this endpoint"),
    ("subscription_url",  "Subscription URL (HTTPS, credentials embedded)"),
]


def config_to_toml(cfg: dict) -> str:
    """Render *cfg* as a TOML string matching the canonical endpoint format."""
    lines: list[str] = [
        "# This file was automatically generated by endpoint "
        "and could be used in vpn client.",
        "",
    ]
    for key, comment in _FIELD_ORDER:
        if key not in cfg:
            continue
        for cline in comment.split("\n"):
            lines.append(f"# {cline}" if not cline.startswith("#") else cline)
        lines.append(f"{key} = {_toml_value(cfg[key])}")
        lines.append("")
    return "\n".join(lines) + "\n"

# ---------------------------------------------------------------------------
# Deep link → config
# ---------------------------------------------------------------------------

def deeplink_to_config(uri: str) -> dict:
    """Parse a tt:// deep link URI and return a config dict."""
    prefix_new = "tt://?"
    prefix_old = "tt://"
    if uri.startswith(prefix_new):
        encoded = uri[len(prefix_new):]
    elif uri.startswith(prefix_old):
        encoded = uri[len(prefix_old):]
    else:
        raise ValueError(f"URI must start with {prefix_old!r}")
    # Restore padding for base64 decoding.
    padding = (4 - len(encoded) % 4) % 4
    payload = base64.urlsafe_b64decode(encoded + "=" * padding)
    return decode_config(payload)


def main() -> None:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <tt://?...>", file=sys.stderr)
        sys.exit(1)

    uri = sys.argv[1]
    cfg = deeplink_to_config(uri)

    # Validate required fields (optional when subscription_url is present).
    if "subscription_url" not in cfg:
        for field in ("hostname", "addresses", "username", "password"):
            if field not in cfg:
                sys.exit(f"error: missing required field: {field}")

    print(config_to_toml(cfg), end="")


if __name__ == "__main__":
    main()
