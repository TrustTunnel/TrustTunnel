# TrustTunnel Sidecar Agent

Sidecar service for Classic mode nodes.

## Responsibilities

- Poll LK internal API for credentials snapshots.
- Atomically update credentials file (`[[client]]` TOML format).
- Trigger TrustTunnel reload via POSIX signal.
- Push sync status and lightweight node metrics back to LK.

## Metrics and health semantics

- Health checks for `TRUSTTUNNEL_HEALTH_ADDR` are executed on a dedicated interval (`HEALTH_CHECK_INTERVAL_SECONDS`, default `15`).
- Metrics push (`METRICS_PUSH_INTERVAL`) uses the latest cached health status from the health loop.
- If endpoint health is down, metrics are sent in degraded mode with:
  - `active_connections=0`
  - `error_rate=1.0` (also set when credential sync previously failed)

## Run

```bash
cargo run -p trusttunnel_sidecar_agent
```

Required environment variables are documented in `docs/DEPLOYMENT.md`.
