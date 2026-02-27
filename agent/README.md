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
