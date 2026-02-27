# TrustTunnel Agent

Dedicated sidecar agent crate and image for TrustTunnel Classic mode.

## Build binary

```bash
cargo build --manifest-path agent/Cargo.toml --release --bin trusttunnel_sidecar_agent
```

## Build container image

```bash
docker build -f agent/Dockerfile -t securelink-trusttunnel-agent:<tag> .
```

The endpoint image is built separately and must remain a different runtime image:

```bash
docker build -f Dockerfile -t securelink-trusttunnel:<tag> .
```

## LK internal API contract

Agent uses only `Authorization: Bearer INTERNAL_AGENT_TOKEN` for all internal LK calls.

### Snapshot pull

```http
GET /internal/vpn/classic/accounts?node_id=<NODE_ID>
Authorization: Bearer <INTERNAL_AGENT_TOKEN>
Accept: application/json
```

Response:

```json
{
  "version": "v42",
  "checksum": "f5e6f5f0df6618f56344fcf8ce8f1133e08e4ce89f5dff6f8a8712f43d6cc7a0",
  "accounts": [
    { "username": "alice", "password": "pw1", "enabled": true },
    { "username": "bob", "password": "pw2", "enabled": false }
  ]
}
```

`enabled=false` accounts are ignored when generating the local credentials file.

### Sync report push

```http
POST /internal/vpn/classic/sync-report
Authorization: Bearer <INTERNAL_AGENT_TOKEN>
Content-Type: application/json
```

```json
{
  "node_id": "node-1",
  "version": "v42",
  "applied_count": 1,
  "status": "success",
  "error": null,
  "timestamp": "2026-02-27T12:34:56Z"
}
```

### Heartbeat push

```http
POST /internal/nodes/heartbeat
Authorization: Bearer <INTERNAL_AGENT_TOKEN>
Content-Type: application/json
```

```json
{
  "node_id": "node-1",
  "status": "ok",
  "metrics_json": {
    "active_connections": 1,
    "cpu_percent": 7.5,
    "mem_percent": 32.1,
    "rx_mbps": 4.8,
    "tx_mbps": 1.3
  }
}
```

## Operational constraints (timeouts / SLA)

- All LK HTTP calls (`accounts`, `sync-report`, `heartbeat`) use shared client limits:
  - `connect_timeout`: **2s**
  - total request timeout (connect + send + response body): **5s**
- Timeout errors are handled explicitly and logged as timeout failures for the corresponding LK operation.
- Snapshot sync keeps exponential backoff (`1s`, `2s`, `4s`) with 3 retries (4 attempts total), preserving update SLA under normal LK availability:
  - max wait on backoff = `7s`
  - max wait on HTTP timeouts = `4 * 5s = 20s`
  - worst-case sync cycle budget = `27s` (within `<=30s` target).
