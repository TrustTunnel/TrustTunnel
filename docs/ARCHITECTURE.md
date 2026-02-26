# Architecture

- **Scope:** Component boundaries, data/control flow, network requirements.
- **Applies to:** Classic (active), Modified (planned)
- **Last updated:** 2026-02-26

## 1. Roles and responsibility split

### LK side (control plane)
LK is responsible for:
- account authentication and subscription checks;
- device registration/limits;
- issuing connection credentials (Classic: static password; Modified: short-lived JWT);
- returning endpoint payload for clients (`endpoint.address`, `endpoint.hostname`, `protocol`, `username`, `password/JWT`).

### TrustTunnel side (data plane)
TrustTunnel endpoint is responsible for:
- accepting incoming TLS sessions on `listen_address`;
- SNI-based cert selection from `hosts.toml`;
- HTTP/1.1, HTTP/2 and optionally QUIC/HTTP/3 listener stack;
- validating Basic credentials and/or JWT based on `[auth]` mode;
- proxying TCP/UDP/ICMP traffic to destination via configured forwarder.

## 2. Connection flow

### Classic (current)
1. Client receives fixed endpoint and `username/password` from LK.
2. Client opens TLS connection to `endpoint.address` and sends SNI = `endpoint.hostname`.
3. TrustTunnel selects matching certificate from `hosts.toml` (`hostname`/`allowed_sni`).
4. Client authenticates with Basic auth.
5. TrustTunnel opens tunneled destination sockets and forwards traffic.

Properties:
- one endpoint IP per node (no runtime endpoint selection);
- login/password are issued from LK pool;
- no endpoint balancing/rotation in endpoint itself.

### Modified (planned, not enabled in this release)
Design target only:
- LK may provide a pool of 5–10 endpoint IPs per logical location;
- client-side or LK-side quality selection can pick better IP;
- optional server/client observations can feed quality logic.

Current status:
- described as roadmap contract only;
- not enabled in current release of this fork.

## 3. TLS / SNI / ingress requirements

- TLS is terminated by TrustTunnel endpoint.
- SNI must match configured `main_hosts.hostname` or `main_hosts.allowed_sni`.
- For public deployments use `listen_address = "0.0.0.0:443"` or equivalent external bind.
- If Kubernetes Ingress is used, passthrough mode is required for native endpoint TLS/SNI behavior.
- L4 path must preserve original TLS handshake (no TLS re-encryption with altered SNI unless certificates are aligned).

## 4. Ports and traffic

Recommended baseline:
- `443/tcp` for HTTP/1.1 and HTTP/2 listeners;
- `443/udp` if QUIC/HTTP/3 is enabled;
- separate internal metrics port (default `127.0.0.1:1987`) with restricted access.
