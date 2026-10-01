#!/usr/bin/env python3
"""Convert a TrustTunnel endpoint TOML config file to a tt://? deep link URI.

Usage:
    python3 config_to_deeplink.py <config.toml>

See DEEP_LINK.md for the specification.
"""

from __future__ import annotations

import base64
import re
import sys

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:
    try:
        import tomli as tomllib  # pip install tomli
    except ModuleNotFoundError:
        sys.exit("error: Python < 3.11 requires the 'tomli' package: pip install tomli")

# ---------------------------------------------------------------------------
# TLS variable-length integer encoding (RFC 9000 §16)
# ---------------------------------------------------------------------------

def encode_varint(value: int) -> bytes:
    """Encode an integer using TLS/QUIC variable-length encoding."""
    if value < 0:
        raise ValueError("varint value must be non-negative")
    if value <= 0x3F:
        return value.to_bytes(1, "big")
    if value <= 0x3FFF:
        return (value | 0x4000).to_bytes(2, "big")
    if value <= 0x3FFFFFFF:
        return (value | 0x80000000).to_bytes(4, "big")
    if value <= 0x3FFFFFFFFFFFFFFF:
        return (value | 0xC000000000000000).to_bytes(8, "big")
    raise ValueError(f"varint value too large: {value}")

# ---------------------------------------------------------------------------
# PEM → DER conversion
# ---------------------------------------------------------------------------

_PEM_RE = re.compile(
    r"-----BEGIN [A-Z0-9 ]+-----\s*\n"
    r"([\sA-Za-z0-9+/=]+?)"
    r"\n-----END [A-Z0-9 ]+-----",
)


def pem_to_der(pem: str) -> bytes:
    """Convert a PEM string (one or more blocks) to concatenated DER bytes."""
    blocks = _PEM_RE.findall(pem)
    if not blocks:
        raise ValueError("no PEM blocks found in certificate field")
    der = bytearray()
    for b64 in blocks:
        der += base64.b64decode(b64)
    return bytes(der)

# ---------------------------------------------------------------------------
# TLV helpers
# ---------------------------------------------------------------------------

def tlv(tag: int, value: bytes) -> bytes:
    """Build a single Tag-Length-Value entry."""
    return encode_varint(tag) + encode_varint(len(value)) + value

# ---------------------------------------------------------------------------
# Field encoders
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
TAG_NAME                 = 0x0C
TAG_DNS_UPSTREAMS        = 0x0D
TAG_SUBSCRIPTION_URL     = 0x0E

CURRENT_VERSION = 2

PROTOCOL_MAP = {"http2": 0x01, "http3": 0x02}

DEFAULTS = {
    "has_ipv6": True,
    "skip_verification": False,
    "upstream_protocol": "http2",
    "anti_dpi": False,
}


def encode_string_array(strings: list[str]) -> bytes:
    """Encode a list of strings as a String[] value (varint-length-prefixed elements)."""
    buf = bytearray()
    for s in strings:
        encoded = s.encode()
        buf += encode_varint(len(encoded))
        buf += encoded
    return bytes(buf)


def encode_config(cfg: dict) -> bytes:
    """Encode a parsed TOML config dict into the TLV binary payload."""
    buf = bytearray()

    subscription_url = cfg.get("subscription_url")
    if subscription_url is not None and not subscription_url.startswith("https://"):
        raise ValueError("subscription_url must be an https:// URL")

    # Version tag
    buf += tlv(0x00, encode_varint(CURRENT_VERSION))

    # Static connection parameters (required unless subscription_url is present)
    for tag, key in [
        (TAG_HOSTNAME, "hostname"),
        (TAG_USERNAME, "username"),
        (TAG_PASSWORD, "password"),
    ]:
        if key not in cfg:
            if subscription_url is None:
                raise KeyError(f"missing required field: {key}")
            continue
        buf += tlv(tag, cfg[key].encode())

    # addresses (may repeat; required unless subscription_url is present)
    addresses = cfg.get("addresses")
    if not addresses:
        if subscription_url is None:
            raise KeyError("missing required field: addresses")
    else:
        for addr in addresses:
            buf += tlv(TAG_ADDRESS, addr.encode())

    # client_random_prefix (optional hex-encoded string)
    if "client_random_prefix" in cfg and cfg["client_random_prefix"]:
        buf += tlv(TAG_CLIENT_RANDOM_PREFIX, cfg["client_random_prefix"].encode())

    # Optional string fields
    if "custom_sni" in cfg:
        buf += tlv(TAG_CUSTOM_SNI, cfg["custom_sni"].encode())

    # Optional boolean fields (omit if equal to default)
    for tag, key in [
        (TAG_HAS_IPV6, "has_ipv6"),
        (TAG_SKIP_VERIFICATION, "skip_verification"),
        (TAG_ANTI_DPI, "anti_dpi"),
    ]:
        if key in cfg and cfg[key] != DEFAULTS.get(key):
            buf += tlv(tag, b"\x01" if cfg[key] else b"\x00")

    # certificate (PEM → concatenated DER)
    if "certificate" in cfg and cfg["certificate"]:
        buf += tlv(TAG_CERTIFICATE, pem_to_der(cfg["certificate"]))

    # upstream_protocol (omit if default)
    proto = cfg.get("upstream_protocol")
    if proto and proto != DEFAULTS["upstream_protocol"]:
        if proto not in PROTOCOL_MAP:
            raise ValueError(f"unknown upstream_protocol: {proto}")
        buf += tlv(TAG_UPSTREAM_PROTOCOL, bytes([PROTOCOL_MAP[proto]]))

    # name (optional)
    if "name" in cfg and cfg["name"]:
        buf += tlv(TAG_NAME, cfg["name"].encode())

    # dns_upstreams (optional, String[] encoding)
    dns = cfg.get("dns_upstreams")
    if dns:
        buf += tlv(TAG_DNS_UPSTREAMS, encode_string_array(dns))

    # subscription_url (optional, format v2)
    if subscription_url:
        buf += tlv(TAG_SUBSCRIPTION_URL, subscription_url.encode())

    return bytes(buf)


def config_to_deeplink(cfg: dict) -> str:
    """Convert a parsed TOML config dict to a tt://? deep link URI."""
    payload = encode_config(cfg)
    encoded = base64.urlsafe_b64encode(payload).rstrip(b"=").decode("ascii")
    return f"tt://?{encoded}"


def main() -> None:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <config.toml>", file=sys.stderr)
        sys.exit(1)

    path = sys.argv[1]
    with open(path, "rb") as f:
        cfg = tomllib.load(f)

    uri = config_to_deeplink(cfg)
    print(uri)


if __name__ == "__main__":
    main()
