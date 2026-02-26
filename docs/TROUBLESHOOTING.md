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
