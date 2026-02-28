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

## 2. Snapshot contract from LK to agent

`GET /internal/vpn/classic/accounts?node_id=<NODE_ID>` returns:
- `version` (string)
- `accounts` (array of objects `{ "username": string, "password": string, "enabled": bool }`)
- `checksum` (lowercase hex SHA-256 string)

### Checksum canonical algorithm (MUST)

`checksum` is SHA-256 over UTF-8 bytes of **canonical JSON string** built from `accounts`:

1. Start from JSON object with single key `accounts`.
2. `accounts` value is an array of account objects with keys exactly `username`, `password`, `enabled`.
3. Before hashing, sort the `accounts` array by tuple `(username, password, enabled)` in ascending lexicographic order.
4. Serialize to compact JSON (no extra spaces/newlines), UTF-8.
5. Compute SHA-256 and encode digest as lowercase hex.

Canonical JSON template:

```json
{"accounts":[{"username":"alice","password":"pw1","enabled":true},{"username":"bob","password":"pw2","enabled":true}]}
```

For the template above:

- SHA-256 input bytes = UTF-8 bytes of that exact line.
- `checksum` = `c24a2e9b8f1f2a8d0aa6ec2e50b63fba5fbf2f0bb7f6170c34f663ddf723352f`.

## 3. Sync report contract

`POST /internal/vpn/classic/sync-report` accepts:
- `node_id` (string)
- `version` (string)
- `applied_count` (number)
- `status` (string)
- `error` (string \/ null)
- `timestamp` (RFC3339 string)

## 4. Heartbeat contract

`POST /internal/nodes/heartbeat` accepts:
- `node_id` (string)
- `status` (string, e.g. `ok`/`degraded`)
- `metrics_json` object:
  - `active_connections` (number)
  - `cpu_percent` (number)
  - `mem_percent` (number)
  - `rx_mbps` (number)
  - `tx_mbps` (number)

## 5. Request/response examples

Snapshot request:

```http
GET /internal/vpn/classic/accounts?node_id=node-1 HTTP/1.1
Authorization: Bearer <INTERNAL_AGENT_TOKEN>
Accept: application/json
```

Snapshot response:

```json
{
  "version": "v-doc-example",
  "accounts": [
    { "username": "bob", "password": "pw2", "enabled": true },
    { "username": "alice", "password": "pw1", "enabled": true },
    { "username": "carol", "password": "pw3", "enabled": false }
  ],
  "checksum": "7bce7f061c8a8acc752af7f8fddca9205f6de0154a766dc0c7416a5dc87ca8ea"
}
```

Sync-report request:

```http
POST /internal/vpn/classic/sync-report HTTP/1.1
Authorization: Bearer <INTERNAL_AGENT_TOKEN>
Content-Type: application/json
```

```json
{
  "node_id": "node-1",
  "version": "v-doc-example",
  "applied_count": 2,
  "status": "success",
  "error": null,
  "timestamp": "2026-02-27T12:34:56Z"
}
```

Heartbeat request:

```http
POST /internal/nodes/heartbeat HTTP/1.1
Authorization: Bearer <INTERNAL_AGENT_TOKEN>
Content-Type: application/json
```

```json
{
  "node_id": "node-1",
  "status": "ok",
  "metrics_json": {
    "active_connections": 1,
    "cpu_percent": 12.4,
    "mem_percent": 38.9,
    "rx_mbps": 8.1,
    "tx_mbps": 2.3
  }
}
```

## 6. What TrustTunnel expects

- TLS handshake with SNI that maps to `hosts.toml`.
- HTTP auth during tunnel setup:
  - Classic: Basic auth (`username/password`).
  - JWT mode: Bearer token validation by `[auth.jwt]` settings.
- If `device_id_claim` is configured, token must include that claim.

## 7. Classic mode (active)

- LK allocates single endpoint IP per node for client config.
- No endpoint-side balancing/rotation expected.
- Password is static (or manually rotated) and validated from credentials file.
- Agent writes only accounts with `enabled=true` into runtime credentials file.

## 8. Modified mode (planned, not enabled in this release)

Target behavior:
- LK can issue pool of endpoint IPs (5–10).
- Selection by quality (client-side or LK-side policy).
- JWT is short-lived and bound to required claims.
- Optional observations can be added for quality feedback loop.

Current constraint:
- In this release, treat Modified as forward-compatible contract only.

## 9. Integration limitations and constraints

- Use `endpoint.address` as concrete `IP:443` for deterministic routing.
- `endpoint.hostname` must be provided separately for SNI/certificate match.
- Domain-only endpoint without explicit IP is not recommended for this client contract unless client explicitly supports that mode.

## 10. TG-shops acceptance profile

### 10.1 Required fields (`node_id` / `status`)

For staging and production acceptance, these fields are mandatory:

- Snapshot pull: `GET /internal/vpn/classic/accounts?node_id=<NODE_ID>`
  - `node_id` is required and must identify a unique sidecar/node in LK inventory.
- Sync-report push: `POST /internal/vpn/classic/sync-report`
  - `node_id` is required.
  - `status` is required and must be one of:
    - `success`
    - `checksum_mismatch`
    - `apply_failed`
    - `retrying`
    - `degraded`
- Heartbeat push: `POST /internal/nodes/heartbeat`
  - `node_id` is required.
  - `status` is required (`ok` or `degraded` as baseline contract).

Validation expectations:
- Missing/empty `node_id` => reject with `4xx` and validation error payload.
- Unknown `status` => reject with `4xx` and validation error payload.

### 10.2 Staging simulation: seller confirmation flow

Recommended step-by-step simulation in staging:

1. Prepare seller fixture in LK with deterministic `node_id` (e.g. `stg-node-01`) and known account list.
2. Start sidecar with valid `INTERNAL_AGENT_TOKEN` and matching `node_id`.
3. Trigger initial snapshot sync and verify sidecar receives `version/accounts/checksum`.
4. Confirm checksum path:
   - run one valid checksum cycle (`status=success` expected),
   - run one intentionally broken checksum cycle (`status=checksum_mismatch` expected).
5. Confirm apply/reload path:
   - publish `enabled=true` account delta,
   - verify credentials file rewrite and runtime reload,
   - verify `sync-report` carries `applied_count > 0` and `status=success`.
6. Simulate transient LK failure (timeout/5xx) and ensure retry/backoff then reconnect behavior, followed by recovery to `success`/`ok` statuses.
7. Execute synthetic seller confirmation in LK (seller marked as confirmed/active in staging UI or API).
8. Verify confirmed seller state is visible downstream in TG-shop-facing data feed/API.

### 10.3 Expected data path up to TG-shop

Expected propagation chain:

1. LK account and seller state store (source of truth).
2. Sidecar polling with `node_id`.
3. Sidecar checksum validation and runtime apply/reload.
4. Sidecar `sync-report` + `heartbeat` posted back to LK.
5. LK internal aggregation/normalization layer updates node + seller readiness.
6. TG-shop integration endpoint/feed receives updated readiness/availability state.
7. TG-shop UI/API reflects final seller availability.

### 10.4 Acceptance criteria and observable signals

Acceptance is met when all criteria below are green:

- **Contract validity**
  - All snapshot/sync-report/heartbeat requests include non-empty `node_id`.
  - All sync-report and heartbeat payloads include valid `status` values.
- **Functional sync**
  - At least one full cycle completes with `status=success` and non-negative `applied_count`.
  - Checksum mismatch path is detectable and reported with `status=checksum_mismatch`.
  - Apply/reload failure path is detectable and reported with `status=apply_failed`.
- **Resilience**
  - Timeout/5xx/network errors produce retry attempts.
  - After fault removal, sidecar returns to steady-state `success` (sync-report) and `ok` (heartbeat).
- **TG-shop propagation**
  - Seller confirmation in LK staging is observable in TG-shop downstream endpoint/feed within agreed SLA window.

Observable signals to capture during acceptance:

- **Logs (sidecar/runtime/LK)**
  - Snapshot pull start/finish with `node_id`, `version`, latency.
  - Checksum verification result.
  - Credentials apply/reload success or error details.
  - Sync-report submission result (HTTP code + payload status).
  - Heartbeat submission result and retry/reconnect events.
- **Metrics**
  - `accounts_sync_success_total`, `accounts_sync_failure_total`.
  - `accounts_checksum_mismatch_total`.
  - `runtime_reload_success_total`, `runtime_reload_failure_total`.
  - `heartbeat_success_total`, `heartbeat_failure_total`.
  - Retry counters and backoff duration histograms.
- **API responses**
  - Snapshot `200` with `{version, accounts, checksum}`.
  - Sync-report `2xx` on valid payload; `4xx` on missing `node_id`/invalid `status`.
  - Heartbeat `2xx` on valid payload; `4xx` on missing `node_id`/invalid `status`.
