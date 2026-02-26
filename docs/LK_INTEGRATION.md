# LK Integration Notes

- **Scope:** Contract between LK and TrustTunnel endpoint/client payload.
- **Applies to:** Classic (active), Modified (planned)
- **Last updated:** 2026-02-26

## 1. Payload contract from LK to client

LK must return (per connection profile):
- `endpoint.address` — endpoint network address in `IP:443` form;
- `endpoint.hostname` — SNI hostname expected by endpoint TLS;
- `protocol` — selected transport profile (matching enabled listener set);
- `username` — auth identity;
- `password` — Classic static password OR Modified short-lived JWT.

## 2. What TrustTunnel expects

- TLS handshake with SNI that maps to `hosts.toml`.
- HTTP auth during tunnel setup:
  - Classic: Basic auth (`username/password`).
  - JWT mode: Bearer token validation by `[auth.jwt]` settings.
- If `device_id_claim` is configured, token must include that claim.

## 3. Classic mode (active)

- LK allocates single endpoint IP per node for client config.
- No endpoint-side balancing/rotation expected.
- Password is static (or manually rotated) and validated from credentials file.

## 4. Modified mode (planned, not enabled in this release)

Target behavior:
- LK can issue pool of endpoint IPs (5–10).
- Selection by quality (client-side or LK-side policy).
- JWT is short-lived and bound to required claims.
- Optional observations can be added for quality feedback loop.

Current constraint:
- In this release, treat Modified as forward-compatible contract only.

## 5. Integration limitations and constraints

- Use `endpoint.address` as concrete `IP:443` for deterministic routing.
- `endpoint.hostname` must be provided separately for SNI/certificate match.
- Domain-only endpoint without explicit IP is not recommended for this client contract unless client explicitly supports that mode.
