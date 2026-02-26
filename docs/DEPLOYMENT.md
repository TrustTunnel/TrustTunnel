# Deployment

- **Scope:** Practical deployment patterns for endpoint service.
- **Applies to:** Classic (active), Modified (planned contract only)
- **Last updated:** 2026-02-26

## 1. Docker (minimum)

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

## 2. Kubernetes (minimum)

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

## 3. systemd

Use `scripts/trusttunnel.service.template` as baseline.

Key points:
- set `WorkingDirectory` to config directory;
- `ExecStart` example: `/opt/trusttunnel/trusttunnel_endpoint vpn.toml hosts.toml`;
- enable restart on failure.

## 4. Runtime environment variables

Documented real usage in current fork:
- `hmac_secret_env` in `[auth.jwt]` points to ENV variable containing HS256 secret.

No other mandatory runtime env vars are required by endpoint startup path.
