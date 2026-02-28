# Monitoring (Prometheus + Grafana)

## Start local stack

```bash
cd monitoring
docker compose up -d
```

## Dashboards

Use dashboard `TrustTunnel & LK Overview` with panels:

- LK load: `rate(http_requests_total[5m])`, `p99 http_request_duration_seconds`
- VPN nodes: `vpn_active_connections`, `vpn_traffic_bytes_total`, `vpn_handshake_duration_seconds`, `vpn_jwt_validation_errors_total`
- System health: CPU/memory/node count/device dynamics (add node exporter metrics in production)

## Alerts

Defined in `monitoring/grafana/alert-rules.yml` examples:

- Spike in `vpn_jwt_validation_errors_total`
- p95 `http_request_duration_seconds > 1s`
- Active connections approaching node capacity (add threshold per node)

## Agent metrics contract

Primary (canonical) agent metrics (alerts should use exactly these names):

- `agent_last_sync_timestamp_seconds` (gauge)
- `agent_sync_duration_seconds` (gauge, last sync cycle duration)
- `agent_sync_duration_seconds_sum` / `agent_sync_duration_seconds_count` (summary-like pair)
- `agent_sync_success_total`, `agent_sync_failure_total` (counters)
- `agent_lk_timeout_total`, `agent_lk_error_total` (counters)

Backwards-compatible aliases:

- `agent_last_sync_timestamp` -> alias for `agent_last_sync_timestamp_seconds`
- `agent_lk_timeout_count` -> alias for `agent_lk_timeout_total`
- `agent_lk_error_count` -> alias for `agent_lk_error_total`

## CI/CD integration notes

- Enable metrics in env-specific settings via `[metrics].enabled=true`
- Keep `[metrics].jwt_error_enabled=true` unless noisy tests require temporary disablement
- Add pipeline checks (`cargo test`, `cargo fmt -- --check`) to keep metric instrumentation valid
- Deploy `monitoring/` stack in dev/stage/prod with environment-specific targets in `prometheus.yml`
