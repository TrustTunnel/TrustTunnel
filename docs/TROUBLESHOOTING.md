# Troubleshooting

- **Scope:** Operational diagnostics for common endpoint failures.
- **Applies to:** Classic (active), Modified (planned-compatible)
- **Last updated:** 2026-02-26

## 1. Symptom → cause → fix

### TLS handshake fails / SNI mismatch
- **Symptom:** clients fail before auth; handshake/cert errors.
- **Cause:** `endpoint.hostname` from LK does not match `hosts.toml` hostname/allowed_sni, or wrong certificate chain.
- **Fix:** align LK hostname contract with `main_hosts`; verify cert/key paths and certificate SANs.

### 401 / auth rejected
- **Symptom:** connection established but auth fails.
- **Cause:** wrong Basic credentials, stale credentials cache window, invalid JWT/signature/issuer/audience/claims.
- **Fix:**
  - Classic: verify `credentials.toml` record for username.
  - JWT: verify `[auth.jwt]` algorithm and key/secret source, inspect token claims and expiry.

### Connection timeout to destination
- **Symptom:** tunnel established, but egress destinations unreachable.
- **Cause:** firewall/egress ACL, DNS/connectivity issues, too strict `connection_establishment_timeout_secs`.
- **Fix:** test outbound reachability from node, inspect network policy, tune timeout values.

### Ingress passthrough misconfiguration
- **Symptom:** TLS works inconsistently, wrong cert observed, HTTP protocol downgrade issues.
- **Cause:** ingress terminates TLS when endpoint expects passthrough.
- **Fix:** switch ingress/gateway to L4 TLS passthrough or make termination model explicit and compatible.

## 2. Diagnostic checklist

1. Validate config parsing locally:
   - run endpoint with target `vpn.toml` and `hosts.toml` in foreground.
2. Check bind/listen:
   - verify `listen_address` and port occupancy.
3. Check TLS artifacts:
   - cert/key file existence and permissions.
4. Check auth mode:
   - `credentials`/`jwt`/`mixed` matches LK payload format.
5. Check metrics endpoint:
   - scrape `metrics.address` and inspect auth/latency counters.

## 3. Useful commands

```bash
# Endpoint startup (foreground)
trusttunnel_endpoint vpn.toml hosts.toml -l debug

# TLS/SNI check from operator host
openssl s_client -connect <ip>:443 -servername <sni-hostname>

# Metrics reachability
curl -s http://127.0.0.1:1987/metrics | head -n 40
```

## 4. Sidecar structured logs (register/reconcile/fetch/apply/reload/health/report)

Sidecar now emits JSON events with a fixed envelope (`event=sidecar_sync`) and mandatory operational fields:

- `node_id`
- `node_name`
- `desired_pool_size`
- `current_revision`
- `fetched_revision`
- `last_sync_status`
- `last_sync_error`

Example successful apply/report flow:

```json
{"event":"sidecar_sync","stage":"fetch","message":"fetched revision r-2026-03-01","node_id":"node-1","node_name":"worker-a","desired_pool_size":30,"current_revision":"r-2026-02-28","fetched_revision":"r-2026-03-01","last_sync_status":"registered","last_sync_error":""}
{"event":"sidecar_sync","stage":"apply","message":"applied reconcile revision=r-2026-03-01 desired_pool=30 allocated_pool=30 healthy_pool=30","node_id":"node-1","node_name":"worker-a","desired_pool_size":30,"current_revision":"r-2026-02-28","fetched_revision":"r-2026-03-01","last_sync_status":"success","last_sync_error":""}
{"event":"sidecar_sync","stage":"report","message":"sync report sent","node_id":"node-1","node_name":"worker-a","desired_pool_size":30,"current_revision":"r-2026-03-01","fetched_revision":"r-2026-03-01","last_sync_status":"success","last_sync_error":""}
```

Example degraded case (reload/health failure):

```json
{"event":"sidecar_sync","stage":"reload","message":"[REDACTED]","node_id":"node-1","node_name":"worker-a","desired_pool_size":30,"current_revision":"r-2026-03-01","fetched_revision":"r-2026-03-02","last_sync_status":"failed","last_sync_error":"[REDACTED]"}
{"event":"sidecar_sync","stage":"health","message":"healthcheck failed after reload","node_id":"node-1","node_name":"worker-a","desired_pool_size":30,"current_revision":"r-2026-03-01","fetched_revision":"r-2026-03-02","last_sync_status":"failed","last_sync_error":"[REDACTED]"}
```

Sensitive value guardrails in logs:

- messages/errors containing `username`, `password`, `token`, `secret` are replaced with `[REDACTED]`.
- never rely on sidecar logs as a secret source of truth.

Sync polling guardrails:

- default `SYNC_INTERVAL_SECONDS` is `30` (MVP-safe baseline).
- values below `15` are clamped to `15`.
- values above `300` are clamped to `300`.
