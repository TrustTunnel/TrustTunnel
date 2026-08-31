# Metrics

This document describes the metrics exposed by vpn-libs-endpoint for monitoring and observability.

## Overview

The endpoint exposes Prometheus-compatible metrics via an HTTP endpoint. Metrics are available when the `MetricsSettings` is configured in the endpoint settings.

## Configuration

To enable metrics, configure the metrics listener in your settings:

```rust
use vpn_libs_endpoint::Settings;

let settings = Settings::builder()
    .metrics(MetricsSettings {
        address: "127.0.0.1:1987".parse().unwrap(),
        request_timeout: Duration::from_secs(3),
        per_client_metrics: false,
    })
    .build();
```

Default metrics endpoint: `http://127.0.0.1:1987/metrics`

### Per-client metrics

Per-user metrics and the `/clients` endpoint are opt-in and controlled by the
`per_client_metrics` metrics setting (default `false`).

Security note: enabling this exposes authenticated usernames on `/metrics` and
usernames plus client IPs on `/clients`. Usernames are normally stripped from
logs, so only enable this when the metrics listener is bound to a trusted
interface and adequately protected.

## Endpoints

### `/metrics`

Returns all metrics in Prometheus text format. When `per_client_metrics` is
enabled, the aggregate metrics below are additionally accompanied by the
per-user `*_per_user` series labelled with the authenticated `username`.

**Example response (aggregate metrics, `per_client_metrics = false`):**

```console
# HELP client_sessions Number of active client sessions
# TYPE client_sessions gauge
client_sessions{protocol_type="http1"} 5
client_sessions{protocol_type="http2"} 3

# HELP inbound_traffic_bytes Total number of bytes uploaded by clients
# TYPE inbound_traffic_bytes counter
inbound_traffic_bytes{protocol_type="http1"} 1234567

# HELP outbound_traffic_bytes Total number of bytes downloaded by clients
# TYPE outbound_traffic_bytes counter
outbound_traffic_bytes{protocol_type="http1"} 7654321

# HELP outbound_tcp_sockets Number of active outbound TCP connections
# TYPE outbound_tcp_sockets gauge
outbound_tcp_sockets 12

# HELP outbound_udp_sockets Number of active outbound UDP sockets
# TYPE outbound_udp_sockets gauge
outbound_udp_sockets 8
```

**Per-user metrics (additional series when `per_client_metrics = true`):**

```console
# HELP client_sessions_per_user Number of active client sessions per user
# TYPE client_sessions_per_user gauge
client_sessions_per_user{protocol_type="http2",username="alice"} 1

# HELP inbound_traffic_bytes_per_user Total number of bytes uploaded per user
# TYPE inbound_traffic_bytes_per_user counter
inbound_traffic_bytes_per_user{username="alice"} 1234567

# HELP outbound_traffic_bytes_per_user Total number of bytes downloaded per user
# TYPE outbound_traffic_bytes_per_user counter
outbound_traffic_bytes_per_user{username="alice"} 7654321
```

### `/clients`

JSON endpoint with per-user aggregates (current sessions, inbound and outbound
bytes, last seen IP). Available only when `per_client_metrics` is enabled;
otherwise it returns `404 Not Found`.

Example response:

```json
[{"username":"alice","ip":"1.2.3.4","sessions":3,"inbound":123456,"outbound":789012},
 {"username":"bob","ip":null,"sessions":0,"inbound":0,"outbound":0}]
```

Notes:

- The `/clients` endpoint includes both configured clients (from settings) and
  active runtime connections. Configured clients with no active sessions are
  present with zero counters, so the monitoring system can track unused users.
- `inbound`/`outbound` are lifetime totals per user; `sessions` is the current
  number of active sessions; `ip` is a client IP observed for this user (one is
  chosen arbitrarily when the user connects from multiple addresses) and may be
  `null` (e.g. HTTP/3 connections where the peer address could not be resolved).

### `/health-check`

Health check endpoint that returns HTTP 200 OK if the endpoint is running.

## Available Metrics

### Client Sessions

**Name:** `client_sessions`
**Type:** Gauge
**Labels:**

- `protocol_type`: Protocol type (`http1`, `http2`, `http3`)

**Description:** Current number of active client sessions grouped by protocol type.

**Use cases:**

- Monitor active connections
- Detect protocol distribution
- Identify connection leaks
- Capacity planning

### Inbound Traffic

**Name:** `inbound_traffic_bytes`
**Type:** Counter
**Labels:**

- `protocol_type`: Protocol type (`http1`, `http2`, `http3`)

**Description:** Total number of bytes uploaded by clients (client → endpoint → destination).

**Use cases:**

- Monitor upload bandwidth usage
- Track traffic patterns by protocol
- Billing and quota management
- Anomaly detection

### Outbound Traffic

**Name:** `outbound_traffic_bytes`
**Type:** Counter
**Labels:**

- `protocol_type`: Protocol type (`http1`, `http2`, `http3`)

**Description:** Total number of bytes downloaded by clients (destination → endpoint → client).

**Use cases:**

- Monitor download bandwidth usage
- Track traffic patterns by protocol
- Billing and quota management
- Anomaly detection

### Per-user Client Sessions

**Name:** `client_sessions_per_user`
**Type:** Gauge
**Labels:**

- `username`: Authenticated user name
- `protocol_type`: Protocol type (`http1`, `http2`, `http3`)

**Description:** Current number of active client sessions grouped by
authenticated user and protocol type. Only exposed when `per_client_metrics`
is enabled.

### Per-user Inbound Traffic

**Name:** `inbound_traffic_bytes_per_user`
**Type:** Counter
**Labels:**

- `username`: Authenticated user name

**Description:** Total number of bytes uploaded by each authenticated user.
Only exposed when `per_client_metrics` is enabled.

### Per-user Outbound Traffic

**Name:** `outbound_traffic_bytes_per_user`
**Type:** Counter
**Labels:**

- `username`: Authenticated user name

**Description:** Total number of bytes downloaded by each authenticated user.
Only exposed when `per_client_metrics` is enabled.

### Outbound TCP Sockets

**Name:** `outbound_tcp_sockets`
**Type:** Gauge
**Labels:** None

**Description:** Current number of active outbound TCP connections from the endpoint to destination servers.

**Use cases:**

- Monitor connection pool size
- Detect connection leaks
- Identify resource exhaustion
- Optimize connection limits
- Debug proxy performance issues

**Notes:**

- Incremented when a new TCP connection is established
- Decremented when the connection is closed
- Includes connections through direct forwarder and SOCKS5 forwarder
- Does not include connections to SOCKS5 proxy itself

### Outbound UDP Sockets

**Name:** `outbound_udp_sockets`
**Type:** Gauge
**Labels:** None

**Description:** Current number of active outbound UDP sockets from the endpoint to destination servers.

**Use cases:**

- Monitor UDP multiplexer state
- Detect socket leaks
- Track UDP traffic load
- Optimize socket pool configuration
- Debug UDP forwarding issues

**Notes:**

- Incremented when a new UDP association is created
- Decremented when the association is closed
- Includes sockets through direct forwarder and SOCKS5 UDP associations
- Each unique source-destination pair counts as one socket

## Metric Types

### Gauge

A gauge is a metric that represents a single numerical value that can arbitrarily go up and down. Gauges are typically used for measured values like current memory usage or number of active connections.

**Examples:** `client_sessions`, `outbound_tcp_sockets`, `outbound_udp_sockets`

### Counter

A counter is a cumulative metric that represents a single monotonically increasing counter whose value can only increase or be reset to zero. Counters are typically used for counts of events like number of requests or bytes transferred.

**Examples:** `inbound_traffic_bytes`, `outbound_traffic_bytes`

## Implementation Details

### Lifecycle Management

Metrics are automatically managed through RAII (Resource Acquisition Is Initialization) pattern:

- **Client sessions:** Counter incremented when session starts, decremented when session ends
- **TCP sockets:** Counter incremented when TCP connection established, decremented when closed
- **UDP sockets:** Counter incremented when UDP association created, decremented when cleaned up
- **Traffic counters:** Incremented as data flows through the pipe

## Troubleshooting

### Metrics endpoint not responding

1. Verify metrics are enabled in configuration
2. Check the listen address is not already in use
3. Verify firewall rules allow connections
4. Check logs for bind errors

### Missing metrics

1. Ensure client sessions are active to generate traffic metrics
2. Verify protocol type labels match expected values
3. Check metrics collection interval in your monitoring system

### Unexpected metric values

1. **Outbound sockets > client sessions:** Normal for HTTP/1.1 with multiple concurrent requests
2. **Outbound sockets = 0 with active sessions:** May indicate all requests are cached or failing
3. **Continuously growing sockets:** Check for connection leaks or slow destinations

## See Also

- [CONFIGURATION.md](CONFIGURATION.md) - Endpoint configuration reference
- [PROTOCOL.md](PROTOCOL.md) - Supported protocols documentation
- [DEVELOPMENT.md](DEVELOPMENT.md) - Development and testing guide
