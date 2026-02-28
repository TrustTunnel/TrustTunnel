# LK Integration Notes

- **Scope:** Contract between LK and TrustTunnel endpoint/client payload.
- **Applies to:** Classic (active), Modified (planned)
- **Last updated:** 2026-02-27

## 1. Payload contract from LK to client

LK must return (per connection profile):
- `endpoint.address` — endpoint network address in `IP:443` form;
- `endpoint.hostname` — SNI hostname expected by endpoint TLS;
- `protocol` — selected transport profile (matching enabled listener set);
- `username` — auth identity;
- `password` — Classic static password OR Modified short-lived JWT.

## 2. Snapshot contract from LK to agent

`GET /internal/vpn/classic/accounts?node_id=<NODE_ID>` returns:
- `version` (string)
- `accounts` (array of objects `{ "username": string, "password": string, "enabled": bool }`)
- `checksum` (lowercase hex SHA-256 string)

### Checksum canonical algorithm (MUST)

`checksum` is SHA-256 over UTF-8 bytes of **canonical JSON string** built from `accounts`:

1. Start from JSON object with single key `accounts`.
2. `accounts` value is an array of account objects with keys exactly `username`, `password`, `enabled`.
3. Before hashing, sort the `accounts` array by tuple `(username, password, enabled)` in ascending lexicographic order.
4. Serialize to compact JSON (no extra spaces/newlines), UTF-8.
5. Compute SHA-256 and encode digest as lowercase hex.

Canonical JSON template:

```json
{"accounts":[{"username":"alice","password":"pw1","enabled":true},{"username":"bob","password":"pw2","enabled":true}]}
```

For the template above:

- SHA-256 input bytes = UTF-8 bytes of that exact line.
- `checksum` = `c24a2e9b8f1f2a8d0aa6ec2e50b63fba5fbf2f0bb7f6170c34f663ddf723352f`.

## 3. Sync report contract

`POST /internal/vpn/classic/sync-report` accepts:
- `node_id` (string)
- `version` (string)
- `applied_count` (number)
- `status` (string)
- `error` (string \/ null)
- `timestamp` (RFC3339 string)

## 4. Heartbeat contract

`POST /internal/nodes/heartbeat` accepts:
- `node_id` (string)
- `status` (string, e.g. `ok`/`degraded`)
- `metrics_json` object:
  - `active_connections` (number)
  - `cpu_percent` (number)
  - `mem_percent` (number)
  - `rx_mbps` (number)
  - `tx_mbps` (number)

## 5. Request/response examples

Snapshot request:

```http
GET /internal/vpn/classic/accounts?node_id=node-1 HTTP/1.1
Authorization: Bearer <INTERNAL_AGENT_TOKEN>
Accept: application/json
```

Snapshot response:

```json
{
  "version": "v-doc-example",
  "accounts": [
    { "username": "bob", "password": "pw2", "enabled": true },
    { "username": "alice", "password": "pw1", "enabled": true },
    { "username": "carol", "password": "pw3", "enabled": false }
  ],
  "checksum": "7bce7f061c8a8acc752af7f8fddca9205f6de0154a766dc0c7416a5dc87ca8ea"
}
```

Sync-report request:

```http
POST /internal/vpn/classic/sync-report HTTP/1.1
Authorization: Bearer <INTERNAL_AGENT_TOKEN>
Content-Type: application/json
```

```json
{
  "node_id": "node-1",
  "version": "v-doc-example",
  "applied_count": 2,
  "status": "success",
  "error": null,
  "timestamp": "2026-02-27T12:34:56Z"
}
```

Heartbeat request:

```http
POST /internal/nodes/heartbeat HTTP/1.1
Authorization: Bearer <INTERNAL_AGENT_TOKEN>
Content-Type: application/json
```

```json
{
  "node_id": "node-1",
  "status": "ok",
  "metrics_json": {
    "active_connections": 1,
    "cpu_percent": 12.4,
    "mem_percent": 38.9,
    "rx_mbps": 8.1,
    "tx_mbps": 2.3
  }
}
```

## 6. What TrustTunnel expects

- TLS handshake with SNI that maps to `hosts.toml`.
- HTTP auth during tunnel setup:
  - Classic: Basic auth (`username/password`).
  - JWT mode: Bearer token validation by `[auth.jwt]` settings.
- If `device_id_claim` is configured, token must include that claim.

## 7. Classic mode (active)

- LK allocates single endpoint IP per node for client config.
- No endpoint-side balancing/rotation expected.
- Password is static (or manually rotated) and validated from credentials file.
- Agent writes only accounts with `enabled=true` into runtime credentials file.

## 8. Modified mode (planned, not enabled in this release)

Target behavior:
- LK can issue pool of endpoint IPs (5–10).
- Selection by quality (client-side or LK-side policy).
- JWT is short-lived and bound to required claims.
- Optional observations can be added for quality feedback loop.

Current constraint:
- In this release, treat Modified as forward-compatible contract only.

## 9. Integration limitations and constraints

- Use `endpoint.address` as concrete `IP:443` for deterministic routing.
- `endpoint.hostname` must be provided separately for SNI/certificate match.
- Domain-only endpoint without explicit IP is not recommended for this client contract unless client explicitly supports that mode.
