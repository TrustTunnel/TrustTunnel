# Protocol

- **Scope:** Handshake and authentication semantics between client/LK/endpoint.
- **Applies to:** Classic (active), Modified (planned)
- **Last updated:** 2026-02-26

## 1. Transport

- Endpoint accepts TLS-based sessions.
- Supported listener stack by configuration:
  - HTTP/1.1
  - HTTP/2
  - QUIC/HTTP/3 (if `listen_protocols.quic` enabled)
- ALPN is negotiated according to enabled listener protocols.

## 2. Authentication transport

TrustTunnel uses HTTP auth headers at tunnel setup stage:
- `Authorization: Basic ...` and/or `Proxy-Authorization: Basic ...` for credentials mode;
- `Authorization: Bearer <jwt>` for JWT mode;
- mixed mode accepts both.

## 3. Auth variants

### Classic (current)
- LK returns static `username/password`.
- Client uses Basic auth.
- Endpoint validates credentials against `credentials_file`.

### Modified (planned)
- LK returns short-lived JWT as password-equivalent secret.
- Client sends JWT in Bearer format (or legacy-compatible mapping if client wrapper requires).
- Endpoint validates token using `[auth.jwt]` settings.

Required/expected JWT claims contract:
- subject claim (`username_claim`, default `sub`) for identity;
- optional `device_id_claim` when device binding is required;
- standard time validity (`exp`, with `leeway_seconds` support).

Optional token constraints:
- `issuer` match when configured;
- `audience` match when configured;
- signing algorithm must match `RS256` or `HS256` as configured.

Status: **planned, not enabled in this release** as full end-to-end Modified flow.

## 4. SNI/TLS coupling

- Client must present SNI aligned with `hosts.toml`.
- TLS certificate and SNI mismatch leads to handshake failure before auth stage.
- For ingress/LB passthrough deployment, original SNI must arrive to endpoint unchanged.
