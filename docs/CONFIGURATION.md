# Configuration

- **Scope:** Runtime files/fields and validated examples for this fork.
- **Applies to:** Classic (active), Modified (planned)
- **Last updated:** 2026-02-26

## 1. Runtime files

Mandatory at startup:
- `vpn.toml` — main endpoint settings.
- `hosts.toml` — TLS host/certificate mapping.

Optional by reference from `vpn.toml`:
- `credentials.toml` via `credentials_file`.
- `rules.toml` via `rules_file`.

## 2. `vpn.toml` full field reference (current fork)

### Root fields
- `listen_address` (SocketAddr) — endpoint bind address.
- `ipv6_available` (bool) — allow IPv6 forwarding.
- `allow_private_network_connections` (bool) — allow forwarding to private nets.
- `tls_handshake_timeout_secs` (u64).
- `client_listener_timeout_secs` (u64).
- `connection_establishment_timeout_secs` (u64).
- `tcp_connections_timeout_secs` (u64).
- `udp_connections_timeout_secs` (u64).
- `forward_protocol` (enum):
  - `direct = {}`
  - `socks5 = { address = "ip:port", extended_auth = false }`
- `listen_protocols`:
  - `http1 = { upload_buffer_size }`
  - `http2 = { initial_connection_window_size, initial_stream_window_size, max_concurrent_streams, max_frame_size, header_table_size }`
  - `quic = { recv_udp_payload_size, send_udp_payload_size, initial_max_data, initial_max_stream_data_bidi_local, initial_max_stream_data_bidi_remote, initial_max_stream_data_uni, initial_max_streams_bidi, initial_max_streams_uni, max_connection_window, max_stream_window, disable_active_migration, enable_early_data, message_queue_capacity }`
- `credentials_file` (string, optional for credentials/mixed mode).
- `[auth]` section.
- `[reverse_proxy]` section (optional): `server_address`, `path_mask`, `h3_backward_compatibility`.
- `[icmp]` section (optional): `interface_name`, `request_timeout_secs`, `recv_message_queue_capacity`.
- `[metrics]` section (optional): `enabled`, `jwt_error_enabled`, `address`, `request_timeout_secs`.
- `rules_file` (string, optional).
- `speedtest_enable` (bool).

### `[auth]`
- `mode = "credentials" | "jwt" | "mixed"`.
- `cache_ttl_seconds`.
- `revocation_sync_seconds`.

### `[auth.jwt]` (required when `mode = "jwt"` or `"mixed"`)
- `algorithm = "RS256" | "HS256"`.
- `issuer` (optional).
- `audience` (optional).
- `leeway_seconds`.
- `username_claim` (default `sub`).
- `device_id_claim` (optional).
- `public_key_path` (for RS256).
- `hmac_secret_env` (for HS256; contains ENV variable name with secret).

## 3. `hosts.toml` full field reference

- `main_hosts = [{ hostname, cert_chain_path, private_key_path, allowed_sni = [] }]`.
- `ping_hosts = [...]` (optional).
- `speedtest_hosts = [...]` (optional).
- `reverse_proxy_hosts = [...]` (optional).

`allowed_sni` allows additional accepted SNI values mapped to the same cert/key pair.

## 4. Minimal config examples

### Classic (active): Basic auth, single endpoint

`vpn.toml`:
```toml
listen_address = "0.0.0.0:443"
ipv6_available = true
allow_private_network_connections = false

[forward_protocol]
direct = {}

[listen_protocols]
http2 = {}

credentials_file = "credentials.toml"

[auth]
mode = "credentials"
cache_ttl_seconds = 5
revocation_sync_seconds = 15

[metrics]
enabled = true
address = "127.0.0.1:1987"
```

`hosts.toml`:
```toml
[[main_hosts]]
hostname = "node-1.example.net"
cert_chain_path = "/etc/trusttunnel/certs/fullchain.pem"
private_key_path = "/etc/trusttunnel/certs/privkey.pem"
allowed_sni = ["edge.example.net"]
```

`credentials.toml`:
```toml
[client_a]
password = "strong-static-password"
```

### Modified (planned placeholder)

В этом релизе нет отдельного runtime режима `modified` в endpoint-конфиге.
Планируемая модель:
- LK отдает пул IP (5–10) и короткий JWT вместо static password;
- endpoint работает в `auth.mode = "jwt"` или `"mixed"`, но логика выбора IP находится вне endpoint;
- статус: **planned, not enabled in this release**.

## 5. Breaking changes vs upstream

Для этого форка фиксируем отличия/акценты:
- Операционный runtime опирается на `vpn.toml` + `hosts.toml` как обязательную пару файлов.
- Техдок исключает deeplink/wizard user-flow из core документации.
- LK integration contract документирован как отдельный файл (`docs/LK_INTEGRATION.md`) для server-side интеграции.
