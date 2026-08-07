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
    })
    .build();
```

Default metrics endpoint: `http://127.0.0.1:1987/metrics`

## Endpoints

### `/metrics`

Returns all metrics in Prometheus text format.

**Example response:**

```console
# HELP client_sessions Number of active client sessions
# TYPE client_sessions gauge
client_sessions{protocol_type="http1",username=""} 5
client_sessions{protocol_type="http2",username="alice"} 3

# HELP inbound_traffic_bytes Total number of bytes uploaded by clients
# TYPE inbound_traffic_bytes counter
inbound_traffic_bytes{username="alice"} 1234567

# HELP outbound_traffic_bytes Total number of bytes downloaded by clients
# TYPE outbound_traffic_bytes counter
outbound_traffic_bytes{username="alice"} 7654321

# HELP outbound_tcp_sockets Number of active outbound TCP connections
# TYPE outbound_tcp_sockets gauge
outbound_tcp_sockets 12

# HELP outbound_udp_sockets Number of active outbound UDP sockets
# TYPE outbound_udp_sockets gauge
outbound_udp_sockets 8
```

### `/health-check`

Health check endpoint that returns HTTP 200 OK if the endpoint is running.

### `/clients`

Returns per-client traffic and session aggregates as JSON. Includes all users from
`credentials.toml`, even if they have never connected.

**Example response:**

```json
[
  {
    "username": "alice",
    "ip": "203.0.113.10",
    "sessions": 2,
    "inbound": 123456,
    "outbound": 789012,
    "total": 912468,
    "limit": 10737418240,
    "quota_exceeded": false
  },
  {
    "username": "bob",
    "ip": null,
    "sessions": 0,
    "inbound": 0,
    "outbound": 0,
    "total": 0,
    "limit": 5368709120,
    "quota_exceeded": false
  }
]
```

**Notes:**

- `inbound` is upload (client → internet), `outbound` is download (internet → client).
- `total` is `inbound + outbound` and is what traffic quotas compare against.
- `limit` is the configured quota in bytes, or omitted/`null` when unlimited.
- `quota_exceeded` is `true` when `total >= limit`.
- Traffic before authentication is counted under `username=""` in Prometheus, not under a named user.

## Available Metrics

### Per-client metrics

When metrics are enabled, traffic and sessions can be filtered by VPN username:

- `client_sessions{protocol_type,username}` — active sessions per protocol and user.
- `inbound_traffic_bytes{username}` — total uploaded bytes per user.
- `outbound_traffic_bytes{username}` — total downloaded bytes per user.

Use `/clients` for a ready-made JSON view suitable for admin panels and bots.

### Client Sessions

**Name:** `client_sessions`
**Type:** Gauge
**Labels:**

- `protocol_type`: Protocol type (`http1`, `http2`, `http3`)
- `username`: VPN username, or empty string before/for unauthenticated traffic

**Description:** Current number of active client sessions grouped by protocol type and user.

**Use cases:**

- Monitor active connections per user
- Detect protocol distribution
- Identify connection leaks
- Capacity planning

### Inbound Traffic

**Name:** `inbound_traffic_bytes`
**Type:** Counter
**Labels:**

- `username`: VPN username, or empty string for unauthenticated traffic

**Description:** Total number of bytes uploaded by clients (client → endpoint → destination).

**Use cases:**

- Monitor upload bandwidth usage per user
- Billing and quota management
- Anomaly detection

### Outbound Traffic

**Name:** `outbound_traffic_bytes`
**Type:** Counter
**Labels:**

- `username`: VPN username, or empty string for unauthenticated traffic

**Description:** Total number of bytes downloaded by clients (destination → endpoint → client).

**Use cases:**

- Monitor download bandwidth usage per user
- Billing and quota management
- Anomaly detection

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
