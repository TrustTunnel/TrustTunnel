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
