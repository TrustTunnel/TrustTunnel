# Metrics

- **Scope:** Available and MVP-required observability signals.
- **Applies to:** Classic (active), Modified (planned-compatible)
- **Last updated:** 2026-02-26

## 1. Collection model

- Prometheus-compatible metrics endpoint is built into endpoint.
- Controlled via `[metrics]` in `vpn.toml` (`enabled`, `address`, `request_timeout_secs`).
- Default bind is local (`127.0.0.1:1987`); publish externally only with access controls.

## 2. Core metrics currently available

MVP baseline:
- Active connections: `vpn_active_connections` (gauge, by `connection_type`).
- Handshake errors/auth errors:
  - `auth_basic_failure_total`
  - `auth_jwt_failure_total`
  - `vpn_jwt_validation_errors_total` (label `reason`, controlled by `jwt_error_enabled`).
- Latency:
  - `vpn_handshake_duration_seconds` (histogram)
  - `vpn_request_latency_seconds` (histogram)
- Bandwidth/traffic:
  - `vpn_traffic_bytes_total` (counter, by `protocol_type`, `direction`)
  - `inbound_traffic_bytes` / `outbound_traffic_bytes` (protocol-labelled counters)

Additional capacity signals:
- `client_sessions` (active sessions by protocol);
- `outbound_tcp_sockets`, `outbound_udp_sockets`.

## 3. How to scrape

Option A (recommended):
- scrape endpoint metrics listener directly from Prometheus over private network.

Option B:
- expose via sidecar/reverse-proxy only in controlled environments.

Option C:
- derive partial KPI from logs when metrics endpoint is restricted.

## 4. Minimal dashboard (text spec)

Suggested panels:
1. `sum(vpn_active_connections)` with split by `connection_type`.
2. `rate(auth_basic_failure_total[5m])` and `rate(auth_jwt_failure_total[5m])`.
3. `histogram_quantile(0.95, sum(rate(vpn_handshake_duration_seconds_bucket[5m])) by (le, protocol))`.
4. `sum(rate(vpn_traffic_bytes_total[1m])) by (direction, protocol_type)`.
5. `outbound_tcp_sockets` / `outbound_udp_sockets` for saturation tracking.

## 5. Planned compatibility for Modified mode

No separate metric namespace required; JWT and latency metrics already cover planned short-lived token flow. Modified mode remains **planned, not enabled in this release**.

## 6. Agent SLA metrics (Classic sidecar)

For sidecar SLA monitoring use the canonical `agent_*` names below (without mixing aliases in alerts):

- Sync freshness and duration:
  - `agent_last_sync_timestamp_seconds` (gauge, unix timestamp of last successful sync)
  - `agent_sync_duration_seconds` (gauge, duration of the latest sync cycle)
- LK reliability counters:
  - `agent_lk_timeout_count` (counter, LK request timeouts)
  - `agent_lk_error_count` (counter, non-timeout LK request errors)
- Heartbeat delivery:
  - `agent_heartbeat_success_total` (counter)
  - `agent_heartbeat_failure_total` (counter)

Compatibility aliases still exposed by the sidecar:
- `agent_last_sync_timestamp` -> alias of `agent_last_sync_timestamp_seconds`
- `agent_lk_timeout_total` -> alias of `agent_lk_timeout_count`
- `agent_lk_error_total` -> alias of `agent_lk_error_count`

### PromQL alert examples for SLA

1. LK timeout growth:

```promql
sum(increase(agent_lk_timeout_count[10m])) > 2
```

2. LK error growth:

```promql
sum(increase(agent_lk_error_count[10m])) > 2
```

3. Stale sync (no successful sync update within 5 minutes):

```promql
(time() - max(agent_last_sync_timestamp_seconds) > 300)
  OR absent(agent_last_sync_timestamp_seconds)
```

4. Heartbeat degradation (success ratio below 90% over 15 minutes):

```promql
(
  sum(increase(agent_heartbeat_success_total[15m]))
  /
  clamp_min(
    sum(increase(agent_heartbeat_success_total[15m]))
    + sum(increase(agent_heartbeat_failure_total[15m])),
    1
  )
) < 0.9
```
