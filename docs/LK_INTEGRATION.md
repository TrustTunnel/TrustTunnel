# LK Integration Notes

- **Scope:** Contract between LK and TrustTunnel endpoint/client payload.
- **Applies to:** Classic (active), Modified (planned)
- **Last updated:** 2026-02-27

## 1. Payload contract from LK to client

LK must return (per connection profile):
- `endpoint.address` — endpoint network address in `IP:443` form;
- `endpoint.hostname` — SNI hostname expected by endpoint TLS;
- `protocol` — selected transport profile (matching enabled listener set);
- `username` — auth identity;
- `password` — Classic static password OR Modified short-lived JWT.

## 2. Snapshot contract from LK to sidecar-agent

`GET /internal/trusttunnel/nodes/{node_id}/credentials-snapshot` returns:
- `version` (string)
- `credentials` (array of objects `{ "username": string, "password": string }`)
- `checksum` (lowercase hex SHA-256 string)

### Checksum canonical algorithm (MUST)

`checksum` is SHA-256 over UTF-8 bytes of **canonical JSON string** built from `credentials`:

1. Start from JSON object with single key `credentials`.
2. `credentials` value is an array of credential objects with keys exactly `username`, `password`.
3. Before hashing, sort the `credentials` array by tuple `(username, password)` in ascending lexicographic order.
4. Serialize to compact JSON (no extra spaces/newlines), UTF-8.
5. Compute SHA-256 and encode digest as lowercase hex.

Canonical JSON template:

```json
{"credentials":[{"username":"alice","password":"pw1"},{"username":"bob","password":"pw2"}]}
```

For the template above:

- SHA-256 input bytes = UTF-8 bytes of that exact line.
- `checksum` = `84cf9958ba7047e33b96652394c2ee7314185913a2517bf89954472c1bdafb14`.

## 3. Request/response example with checksum

Request:

```http
GET /internal/trusttunnel/nodes/node-1/credentials-snapshot HTTP/1.1
Authorization: Bearer <INTERNAL_AGENT_TOKEN>
Accept: application/json
```

Response:

```json
{
  "version": "v-doc-example",
  "credentials": [
    { "username": "bob", "password": "pw2" },
    { "username": "alice", "password": "pw1" }
  ],
  "checksum": "84cf9958ba7047e33b96652394c2ee7314185913a2517bf89954472c1bdafb14"
}
```

Note: input order in response can differ; checksum verification uses sorted canonical order.

## 4. What TrustTunnel expects

- TLS handshake with SNI that maps to `hosts.toml`.
- HTTP auth during tunnel setup:
  - Classic: Basic auth (`username/password`).
  - JWT mode: Bearer token validation by `[auth.jwt]` settings.
- If `device_id_claim` is configured, token must include that claim.

## 5. Classic mode (active)

- LK allocates single endpoint IP per node for client config.
- No endpoint-side balancing/rotation expected.
- Password is static (or manually rotated) and validated from credentials file.

## 6. Modified mode (planned, not enabled in this release)

Target behavior:
- LK can issue pool of endpoint IPs (5–10).
- Selection by quality (client-side or LK-side policy).
- JWT is short-lived and bound to required claims.
- Optional observations can be added for quality feedback loop.

Current constraint:
- In this release, treat Modified as forward-compatible contract only.

## 7. Integration limitations and constraints

- Use `endpoint.address` as concrete `IP:443` for deterministic routing.
- `endpoint.hostname` must be provided separately for SNI/certificate match.
- Domain-only endpoint without explicit IP is not recommended for this client contract unless client explicitly supports that mode.
