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
        enabled: true,
        jwt_error_enabled: true,
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

### VPN Handshake Duration

**Name:** `vpn_handshake_duration_seconds`  
**Type:** Histogram  
**Labels:** `protocol`

Tracks TLS/channel handshake duration for incoming VPN sessions.


### Authentication Counters

**Names:** `auth_basic_success_total`, `auth_basic_failure_total`, `auth_jwt_success_total`, `auth_jwt_failure_total`  
**Type:** Counter  
**Labels:** None

Tracks successful and failed authentication attempts split by auth type (Basic vs JWT).

### JWT Validation Errors

**Name:** `vpn_jwt_validation_errors_total`  
**Type:** Counter  
**Labels:** `reason`

Counts failed JWT validations (expired/invalid/signature/device claim issues).

### Active VPN Connections

**Name:** `vpn_active_connections`  
**Type:** Gauge  
**Labels:** `connection_type`

Current number of active tunnel handlers (`tcp`, `udp_or_icmp`, `speedtest`).

### VPN Request Latency

**Name:** `vpn_request_latency_seconds`  
**Type:** Histogram  
**Labels:** `tunnel_type`

Latency of processing tunnel requests.

### Directional Traffic Counters

**Name:** `vpn_traffic_bytes_total`  
**Type:** Counter  
**Labels:** `protocol_type`, `direction`

Bytes split by direction (`in`, `out`).

### Session Guard Metrics

- `session_guard_active_sessions_total` (gauge): current total active client sessions tracked by session guard.
- `session_guard_rejections_total` (counter): total session acquisition rejections due to per-user session limit.
- `session_guard_stale_reaped_total` (counter): total stale sessions removed by background cleanup.
- `session_guard_users_at_limit` (gauge): current number of users that reached the configured session limit.
- `session_guard_registry_size` (gauge): current number of users present in the in-memory session registry.

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
