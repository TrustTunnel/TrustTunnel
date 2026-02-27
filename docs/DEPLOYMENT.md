# Deployment

- **Scope:** Practical deployment patterns for endpoint service.
- **Applies to:** Classic (active), Modified (planned contract only)
- **Last updated:** 2026-02-26

## 1. Build container images

Build endpoint and agent as separate images (do not combine into one runtime image):

```bash
docker build -f Dockerfile -t securelink-trusttunnel:<tag> .
docker build -f agent/Dockerfile -t securelink-trusttunnel-agent:<tag> .
```

## 2. Docker (minimum)

Image runs endpoint binary with fixed startup command expecting:
- `/etc/trusttunnel/vpn.toml`
- `/etc/trusttunnel/hosts.toml`

Example:
```bash
docker run -d --name trusttunnel \
  --restart unless-stopped \
  -p 443:8443/tcp \
  -p 443:8443/udp \
  -v /opt/trusttunnel/vpn.toml:/etc/trusttunnel/vpn.toml:ro \
  -v /opt/trusttunnel/hosts.toml:/etc/trusttunnel/hosts.toml:ro \
  -v /opt/trusttunnel/credentials.toml:/etc/trusttunnel/credentials.toml:ro \
  -v /opt/trusttunnel/certs:/etc/trusttunnel/certs:ro \
  ghcr.io/<org>/trusttunnel:latest
```

> Note: container `EXPOSE` is `8443`; align service mapping with your `listen_address` in `vpn.toml`.

## 3. Kubernetes (minimum)

### Deployment/service baseline
- Mount `vpn.toml`, `hosts.toml`, credentials and certs from Secret/ConfigMap.
- Expose TCP 443 (and UDP 443 if QUIC enabled).
- Run as non-root when possible (see Security doc).

Minimal resources to provide:
- `Deployment` (or StatefulSet) with read-only volumes for config/certs;
- `Service` (LoadBalancer/NodePort) with TCP 443 and optional UDP 443;
- `Ingress`/Gateway in TLS passthrough mode if SNI-based cert selection is preserved in endpoint.

### Ingress passthrough requirement
If TLS terminates before endpoint, ensure SNI and cert strategy remains compatible. Recommended for current architecture: L4 passthrough to endpoint TLS.

## 4. systemd

Use `scripts/trusttunnel.service.template` as baseline.

Key points:
- set `WorkingDirectory` to config directory;
- `ExecStart` example: `/opt/trusttunnel/trusttunnel_endpoint vpn.toml hosts.toml`;
- enable restart on failure.

## 5. Runtime environment variables

Documented real usage in current fork:
- `hmac_secret_env` in `[auth.jwt]` points to ENV variable containing HS256 secret.

No other mandatory runtime env vars are required by endpoint startup path.

## 6. Classic mode with Sidecar Agent

For Classic production mode run two containers in one Pod:
- `trusttunnel_endpoint`
- `trusttunnel_sidecar_agent`

Both containers share writable volume `/shared`:
- agent writes `/shared/credentials.toml` atomically;
- endpoint reads same file through `credentials_file` in `vpn.toml`;
- agent sends `SIGHUP` to endpoint process, endpoint reloads auth data without restart.

Required sidecar env:
- `LK_INTERNAL_BASE_URL`
- `INTERNAL_AGENT_TOKEN`
- `NODE_ID`
- `SYNC_INTERVAL_SECONDS` (default: `60`)
- `CREDENTIALS_PATH` (default: `/shared/credentials.toml`)
- `TRUSTTUNNEL_RELOAD_SIGNAL` (default: `SIGHUP`)
- `TRUSTTUNNEL_HEALTH_ADDR` (default: `localhost:443`)
- `HEALTH_CHECK_INTERVAL_SECONDS` (default: `15`)
- `METRICS_PUSH_INTERVAL` (default: `30`)

Metrics semantics in sidecar:
- health probing (`TRUSTTUNNEL_HEALTH_ADDR`) runs on `HEALTH_CHECK_INTERVAL_SECONDS` cadence and stores the last known state;
- metrics push uses the latest known health state instead of doing an on-demand health TCP dial;
- if endpoint is down, sidecar reports `active_connections=0` and marks degraded state via `error_rate=1.0`.

Kubernetes notes:
- use `envFrom.secretRef` for `INTERNAL_AGENT_TOKEN`;
- keep LK internal API reachable only inside private network;
- enable `shareProcessNamespace: true` so sidecar can signal endpoint PID;
- restart is not required for credentials update.
