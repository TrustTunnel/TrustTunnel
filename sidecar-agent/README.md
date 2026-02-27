# TrustTunnel Sidecar Agent

Sidecar service for Classic mode nodes.

## Responsibilities

- Poll LK internal API for credentials snapshots.
- Atomically update credentials file (`[[client]]` TOML format).
- Trigger TrustTunnel reload via POSIX signal.
- Push sync status and lightweight node metrics back to LK.

## Run

```bash
cargo run -p trusttunnel_sidecar_agent
```

Required environment variables are documented in `docs/DEPLOYMENT.md`.
