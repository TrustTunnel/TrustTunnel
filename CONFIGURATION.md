# TrustTunnel Endpoint Configuration

This document describes all available configuration settings and configuration files for the TrustTunnel VPN endpoint.

## Table of Contents

- [Overview](#overview)
- [Command Line Arguments](#command-line-arguments)
- [Configuration Files](#configuration-files)
    - [Main Settings File (vpn.toml)](#main-settings-file-vpntoml)
    - [TLS Hosts Settings File (hosts.toml)](#tls-hosts-settings-file-hoststoml)
    - [Credentials File (credentials.toml)](#credentials-file-credentialstoml)
    - [Rules File (rules.toml)](#rules-file-rulestoml)
- [Settings Reference](#settings-reference)
    - [Core Settings](#core-settings)
    - [Listen Protocol Settings](#listen-protocol-settings)
    - [Forward Protocol Settings](#forward-protocol-settings)
    - [Reverse Proxy Settings](#reverse-proxy-settings)
    - [ICMP Settings](#icmp-settings)
    - [Metrics Settings](#metrics-settings)
- [TLS Hosts Reference](#tls-hosts-reference)
- [Rules Reference](#rules-reference)
- [Runtime Configuration](#runtime-configuration)

---

## Overview

The TrustTunnel endpoint uses TOML-formatted configuration files. The main
configuration is split into:

1. **Main settings file** - Core endpoint configuration (timeouts, protocols, etc.)
2. **TLS hosts settings file** - TLS certificate and hostname configuration
3. **Credentials file** - Client authentication credentials
4. **Rules file** - Connection filtering rules

The `setup_wizard` tool can generate these files interactively.

---

## Command Line Arguments

The endpoint binary accepts the following command line arguments:

| Argument | Short | Description | Default |
| -------- | ------- | ----------- | ------- |
| `--version` | `-v` | Print version and exit | - |
| `--loglvl` | `-l` | Logging level (`info`, `debug`, `trace`) | `info` |
| `--logfile` | - | File path for storing logs (stdout if not specified) | stdout |
| `--sentry_dsn` | - | Sentry DSN for error reporting | - |
| `--jobs` | - | Number of worker threads (defaults to CPU count) | CPU count |
| `<settings>` | - | **Required.** Path to main settings file | - |
| `<tls_hosts_settings>` | - | **Required.** Path to TLS hosts settings file | - |
| `--client_config` | `-c` | Print endpoint config for specified client and exit | - |
| `--address` | `-a` | Endpoint address to add to client config (requires `-c`) | - |

### Examples

```bash
# Start the endpoint
./trusttunnel_endpoint vpn.toml hosts.toml

# Start with debug logging
./trusttunnel_endpoint vpn.toml hosts.toml -l debug

# Start with file logging
./trusttunnel_endpoint vpn.toml hosts.toml --logfile /var/log/trusttunnel.log

# Export client configuration
./trusttunnel_endpoint vpn.toml hosts.toml -c username -a 203.0.113.1

# Export client configuration with explicit port
./trusttunnel_endpoint vpn.toml hosts.toml -c username -a 203.0.113.1:443
```

---

## Configuration Files

### Main Settings File (vpn.toml)

The main settings file contains core endpoint configuration. Example:

```toml
# The address to listen on
listen_address = "0.0.0.0:443"

# Whether IPv6 connections can be routed
ipv6_available = true

# Whether connections to private network of the endpoint are allowed
allow_private_network_connections = false

# Timeout of an incoming TLS handshake (seconds)
tls_handshake_timeout_secs = 10

# Timeout of a client listener (seconds)
client_listener_timeout_secs = 600

# Timeout of outgoing connection establishment (seconds)
connection_establishment_timeout_secs = 30

# Idle timeout of tunneled TCP connections (seconds)
tcp_connections_timeout_secs = 604800

# Timeout of tunneled UDP "connections" (seconds)
udp_connections_timeout_secs = 300

# Path to credentials file
credentials_file = "credentials.toml"

[auth]
mode = "credentials" # credentials | jwt | mixed
cache_ttl_seconds = 5 # cache TTL for Basic/JWT auth checks
revocation_sync_seconds = 15 # periodic auth cache flush for revocation sync

[auth.jwt]
algorithm = "RS256" # RS256 | HS256
issuer = "https://lk.securesoft.dev" # optional
audience = "trusttunnel" # optional
leeway_seconds = 30
username_claim = "sub"
device_id_claim = "device_id" # optional
public_key_path = "jwt/public.pem" # required for RS256
# hmac_secret_env = "TRUSTTUNNEL_JWT_SECRET" # required for HS256

# Path to rules file (optional)
rules_file = "rules.toml"

# Listen protocol settings
[listen_protocols]

[listen_protocols.http1]
upload_buffer_size = 32768

[listen_protocols.http2]
initial_connection_window_size = 8388608
initial_stream_window_size = 131072
max_concurrent_streams = 1000
max_frame_size = 16384
header_table_size = 65536

[listen_protocols.quic]
recv_udp_payload_size = 1350
send_udp_payload_size = 1350
initial_max_data = 104857600
initial_max_stream_data_bidi_local = 1048576
initial_max_stream_data_bidi_remote = 1048576
initial_max_stream_data_uni = 1048576
initial_max_streams_bidi = 4096
initial_max_streams_uni = 4096
max_connection_window = 25165824
max_stream_window = 16777216
disable_active_migration = true
enable_early_data = true
message_queue_capacity = 4096

# Forward protocol (optional, defaults to direct)
[forward_protocol]
direct = {}

# Reverse proxy settings (optional)
# [reverse_proxy]
# server_address = "127.0.0.1:8080"
# path_mask = "/api"
# h3_backward_compatibility = false

# ICMP settings (optional, requires superuser)
# [icmp]
# interface_name = "eth0"
# request_timeout_secs = 3
# recv_message_queue_capacity = 256

# Metrics settings (optional)
# [metrics]
# enabled = true
# jwt_error_enabled = true
# address = "127.0.0.1:1987"
# request_timeout_secs = 3
```

### TLS Hosts Settings File (hosts.toml)

Configures TLS certificates and hostnames. Example:

```toml
# Main TLS hosts for traffic tunneling
[[main_hosts]]
hostname = "vpn.example.com"
cert_chain_path = "certs/cert.pem"
private_key_path = "certs/key.pem"

# Ping hosts for HTTPS health checks (optional)
[[ping_hosts]]
hostname = "ping.vpn.example.com"
cert_chain_path = "certs/cert.pem"
private_key_path = "certs/key.pem"

# Speed test hosts (optional)
[[speedtest_hosts]]
hostname = "speed.vpn.example.com"
cert_chain_path = "certs/cert.pem"
private_key_path = "certs/key.pem"

# Reverse proxy hosts (optional, requires reverse_proxy in main settings)
# [[reverse_proxy_hosts]]
# hostname = "api.example.com"
# cert_chain_path = "certs/cert.pem"
# private_key_path = "certs/key.pem"
```

### Credentials File (credentials.toml)

Contains client authentication credentials. Example:

```toml
[[client]]
username = "user1"
password = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
device_user = "device-user-1" # optional login alias for Basic
device_id = "dev-1" # optional LK device id for JWT binding

[[client]]
username = "user2"
password = "sha256:60303ae22b9988617223f230e363dc4a63e7b445f31f7f8132d0ebf7e0fd4cae"
```

### Rules File (rules.toml)

Defines connection filtering rules. Example:

```toml
# Rules are evaluated in order, first matching rule's action is applied.
# If no rules match, the connection is allowed by default.

# Deny connections from specific IP range
[[rule]]
cidr = "192.168.1.0/24"
action = "deny"

# Allow connections with specific TLS client random prefix
[[rule]]
client_random_prefix = "aabbcc"
action = "allow"

# Deny connections matching both IP and client random with mask
[[rule]]
cidr = "10.0.0.0/8"
client_random_prefix = "a0b0/f0f0"
action = "deny"
```

---

## Settings Reference

### Core Settings

| Setting | Type | Default | Description |
| ------- | ---- | ------- | ----------- |
| `listen_address` | String | `0.0.0.0:443` | Address and port to listen on |
| `ipv6_available` | Boolean | `true` | Whether IPv6 connections can be routed |
| `allow_private_network_connections` | Boolean | `false` | Allow connections to endpoint's private network |
| `tls_handshake_timeout_secs` | Integer | `10` | TLS handshake timeout in seconds |
| `client_listener_timeout_secs` | Integer | `600` | Client listener timeout in seconds (10 minutes) |
| `connection_establishment_timeout_secs` | Integer | `30` | Outgoing connection timeout in seconds |
| `tcp_connections_timeout_secs` | Integer | `604800` | Idle TCP connection timeout (1 week) |
| `udp_connections_timeout_secs` | Integer | `300` | UDP connection timeout (5 minutes) |
| `credentials_file` | String | - | Path to credentials file |
| `rules_file` | String | - | Path to rules file (optional) |
| `auth.mode` | String | `credentials` | Auth mode: `credentials`, `jwt`, or `mixed` |
| `auth.cache_ttl_seconds` | Integer | `5` | TTL for cached Basic/JWT validation results to reduce LK/DB load |
| `auth.revocation_sync_seconds` | Integer | `15` | Periodic cache flush interval for revocation synchronization |

### JWT Authentication Settings (`[auth.jwt]`)

| Setting | Type | Default | Description |
| ------- | ---- | ------- | ----------- |
| `algorithm` | String | - | JWT algorithm: `RS256` or `HS256` |
| `issuer` | String | - | Optional `iss` claim check |
| `audience` | String | - | Optional `aud` claim check |
| `leeway_seconds` | Integer | `30` | Allowed clock skew for `exp` |
| `username_claim` | String | `sub` | JWT claim used as LK username and matched with `credentials.toml` |
| `device_id_claim` | String | - | Optional JWT claim matched against `client.device_id` in `credentials.toml` |
| `public_key_path` | String | - | PEM public key path for RS256 |
| `hmac_secret_env` | String | - | Env var containing HMAC secret for HS256 |

In JWT mode, client sends Basic auth and uses JWT as password:

`proxy-authorization: Basic base64("username:JWT_TOKEN")`

In Mixed mode both methods are active at the same time:
- first `Authorization: Bearer <token>` is validated;
- if Bearer is missing/invalid and Basic is provided, server validates Basic credentials.

JWT claims are validated against `credentials.toml`:
- `username_claim` must match LK username (`client.username`);
- if `device_id_claim` is configured, it must match `client.device_id`.

### Migration and backward compatibility

- Users on JWT-only mode do not need any changes.
- To enable Basic + JWT simultaneously:
  1. create `credentials.toml`;
  2. set `auth.mode = "mixed"`;
  3. configure `[auth.jwt]`.

### Listen Protocol Settings

Configure which protocols the endpoint accepts. At least one protocol must be enabled.

#### HTTP/1.1 Settings (`[listen_protocols.http1]`)

| Setting | Type | Default | Description |
| ------- | ---- | ------- | ----------- |
| `upload_buffer_size` | Integer | `32768` | Buffer size for outgoing traffic (bytes) |

#### HTTP/2 Settings (`[listen_protocols.http2]`)

| Setting | Type | Default | Description |
| ------- | ---- | ------- | ----------- |
| `initial_connection_window_size` | Integer | `8388608` | Connection-level flow control window (8 MB) |
| `initial_stream_window_size` | Integer | `131072` | Stream-level flow control window (128 KB) |
| `max_concurrent_streams` | Integer | `1000` | Maximum concurrent streams |
| `max_frame_size` | Integer | `16384` | Maximum HTTP/2 frame payload size |
| `header_table_size` | Integer | `65536` | Maximum header frame size |

#### QUIC/HTTP/3 Settings (`[listen_protocols.quic]`)

| Setting | Type | Default | Description |
| ------- | ---- | ------- | ----------- |
| `recv_udp_payload_size` | Integer | `1350` | Maximum received UDP payload size |
| `send_udp_payload_size` | Integer | `1350` | Maximum sent UDP payload size |
| `initial_max_data` | Integer | `104857600` | Initial max connection data (100 MB) |
| `initial_max_stream_data_bidi_local` | Integer | `1048576` | Local bidirectional stream flow control (1 MB) |
| `initial_max_stream_data_bidi_remote` | Integer | `1048576` | Remote bidirectional stream flow control (1 MB) |
| `initial_max_stream_data_uni` | Integer | `1048576` | Unidirectional stream flow control (1 MB) |
| `initial_max_streams_bidi` | Integer | `4096` | Maximum bidirectional streams |
| `initial_max_streams_uni` | Integer | `4096` | Maximum unidirectional streams |
| `max_connection_window` | Integer | `25165824` | Maximum connection window (24 MB) |
| `max_stream_window` | Integer | `16777216` | Maximum stream window (16 MB) |
| `disable_active_migration` | Boolean | `true` | Disable active connection migration |
| `enable_early_data` | Boolean | `true` | Enable 0-RTT early data |
| `message_queue_capacity` | Integer | `4096` | QUIC multiplexer queue capacity |

### Forward Protocol Settings

Configure how the endpoint forwards connections.

#### Direct Forwarding (default)

```toml
[forward_protocol]
direct = {}
```

Routes connections directly to target hosts.

#### SOCKS5 Forwarding

```toml
[forward_protocol.socks5]
address = "127.0.0.1:1080"
extended_auth = false
```

| Setting | Type | Default | Description |
| ------- | ---- | ------- | ----------- |
| `address` | String | - | **Required.** SOCKS5 proxy address |
| `extended_auth` | Boolean | `false` | Enable extended authentication |

### Reverse Proxy Settings

Optional. Enables TLS termination and HTTP protocol translation.

```toml
[reverse_proxy]
server_address = "127.0.0.1:8080"
path_mask = "/api"
h3_backward_compatibility = false
```

| Setting | Type | Default | Description |
| ------- | ---- | ------- | ----------- |
| `server_address` | String | - | **Required.** Origin server address |
| `path_mask` | String | - | **Required.** Path prefix for routing (must start with `/`) |
| `h3_backward_compatibility` | Boolean | `false` | Override HTTP method for H3→H1 translation |

The reverse proxy translates HTTP/x traffic to HTTP/1.1 towards the origin server. Translated requests include the `X-Original-Protocol` header (`HTTP1` or `HTTP3`).

### ICMP Settings

Optional. Enables ICMP forwarding. Requires superuser privileges on some systems.

```toml
[icmp]
interface_name = "eth0"
request_timeout_secs = 3
recv_message_queue_capacity = 256
```

| Setting | Type | Default | Description |
| ------- | ---- | ------- | ----------- |
| `interface_name` | String | `eth0` (Linux) / `en0` (macOS) | Network interface for ICMP socket |
| `request_timeout_secs` | Integer | `3` | ICMP request timeout in seconds |
| `recv_message_queue_capacity` | Integer | `256` | Message queue capacity per client |

### Metrics Settings

Optional. Enables Prometheus-compatible metrics endpoint.

```toml
[metrics]
enabled = true
jwt_error_enabled = true
address = "127.0.0.1:1987"
request_timeout_secs = 3
```

| Setting | Type | Default | Description |
| ------- | ---- | ------- | ----------- |
| `enabled` | Boolean | `true` | Enables metrics listener (`false` disables `/metrics` and `/health-check`) |
| `jwt_error_enabled` | Boolean | `true` | Enables `vpn_jwt_validation_errors_total` increments |
| `address` | String | `127.0.0.1:1987` | Metrics endpoint address |
| `request_timeout_secs` | Integer | `3` | Request timeout in seconds |

---

## TLS Hosts Reference

Each TLS host entry requires:

| Field | Type | Description |
| ----- | ---- | ----------- |
| `hostname` | String | **Required.** Hostname for TLS SNI matching (must be unique) |
| `cert_chain_path` | String | **Required.** Path to PEM certificate chain file |
| `private_key_path` | String | **Required.** Path to PEM private key file |

### Host Types

- **`main_hosts`** - Primary hosts for VPN traffic tunneling and service requests
- **`ping_hosts`** - Respond with `200 OK` to HTTPS GET requests (health checks)
- **`speedtest_hosts`** - Handle speed test requests:
    - `GET /Nmb.bin` (N=1-100): Download N megabytes
    - `POST /upload.html`: Upload test (up to 120 MB)
- **`reverse_proxy_hosts`** - Forward to reverse proxy server (requires `[reverse_proxy]`)

---

## Rules Reference

Rules filter incoming connections based on client IP and/or TLS client random data.

### Rule Structure

```toml
[[rule]]
cidr = "192.168.0.0/16"           # Optional: IP range in CIDR notation
client_random_prefix = "aabbcc"   # Optional: Hex-encoded prefix or prefix/mask
action = "allow"                  # Required: "allow" or "deny"
```

### Evaluation

1. Rules are evaluated in order
2. First matching rule's action is applied
3. If no rules match, connection is **allowed** by default
4. If both `cidr` and `client_random_prefix` are specified, both must match

### Client Random Matching

Two formats are supported:

**Simple prefix matching:**

```toml
client_random_prefix = "aabbcc"
```

Matches if TLS client random starts with `0xaabbcc`.

**Bitwise matching with mask:**

```toml
client_random_prefix = "a0b0/f0f0"
```

Matches if `(client_random & 0xf0f0) == (0xa0b0 & 0xf0f0)`.

### Examples

```toml
# Block specific IP range
[[rule]]
cidr = "192.168.1.0/24"
action = "deny"

# Allow specific client random prefix
[[rule]]
client_random_prefix = "deadbeef"
action = "allow"

# Block internal networks with specific client signature
[[rule]]
cidr = "10.0.0.0/8"
client_random_prefix = "bad0/ff00"
action = "deny"

# Catch-all deny (place last)
[[rule]]
action = "deny"
```

---

## Runtime Configuration

### Hot Reloading TLS Hosts

Send `SIGHUP` to the endpoint process to reload TLS hosts settings without restart:

```bash
kill -HUP $(pidof trusttunnel_endpoint)
```

This reloads the TLS hosts settings file specified at startup.

### Systemd Service

A systemd service template is provided. Default configuration assumes files in `/opt/trusttunnel/`:

```bash
# Install service
sudo cp /opt/trusttunnel/trusttunnel.service.template /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now trusttunnel

# Reload TLS settings
sudo systemctl reload trusttunnel

# View logs
sudo journalctl -u trusttunnel -f
```

---

## See Also

- [README.md](README.md) - Quick start guide
- [DEVELOPMENT.md](DEVELOPMENT.md) - Development documentation
- [PROTOCOL.md](PROTOCOL.md) - Protocol specification
- [CHANGELOG.md](CHANGELOG.md) - Changelog
