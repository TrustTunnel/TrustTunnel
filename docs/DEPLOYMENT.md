# Deployment

- **Scope:** Practical deployment patterns for endpoint service.
- **Applies to:** Classic (active), Modified (planned contract only)
- **Last updated:** 2026-02-26

## 1. Build container images

Build endpoint and agent as separate images (do not combine into one runtime image):

```bash
docker build -f Dockerfile -t securelink-trusttunnel:<tag> .
docker build -f agent/Dockerfile -t securelink-trusttunnel-agent:<tag> .
```

### CI tag format (GHCR)

Build workflow publishes both images to GHCR with a fixed tag policy:

- `sha-<short_sha>` — for every `push` (branches and tags).
- `v*` release tag (`v1.2.3`, etc.) — only for tag events `refs/tags/v*`.
- `latest` — only when source ref is `refs/heads/main`.

This means release tags pushed from non-`main` refs do **not** overwrite `latest`.

## 2. Docker (minimum)

Image runs endpoint binary with fixed startup command expecting:
- `/etc/trusttunnel/vpn.toml`
- `/etc/trusttunnel/hosts.toml`

Example:
```bash
docker run -d --name trusttunnel \
  --restart unless-stopped \
  -p 443:8443/tcp \
  -p 443:8443/udp \
  -v /opt/trusttunnel/vpn.toml:/etc/trusttunnel/vpn.toml:ro \
  -v /opt/trusttunnel/hosts.toml:/etc/trusttunnel/hosts.toml:ro \
  -v /opt/trusttunnel/credentials.toml:/etc/trusttunnel/credentials.toml:ro \
  -v /opt/trusttunnel/certs:/etc/trusttunnel/certs:ro \
  ghcr.io/<org>/trusttunnel:latest
```

> Note: container `EXPOSE` is `8443`; align service mapping with your `listen_address` in `vpn.toml`.

## 3. Kubernetes (minimum)

### Deployment/service baseline
- Mount `vpn.toml`, `hosts.toml`, credentials and certs from Secret/ConfigMap.
- Expose TCP 443 (and UDP 443 if QUIC enabled).
- Run as non-root when possible (see Security doc).

Minimal resources to provide:
- `Deployment` (or StatefulSet) with read-only volumes for config/certs;
- `Service` (LoadBalancer/NodePort) with TCP 443 and optional UDP 443;
- `Ingress`/Gateway in TLS passthrough mode if SNI-based cert selection is preserved in endpoint.

### Ingress passthrough requirement
If TLS terminates before endpoint, ensure SNI and cert strategy remains compatible. Recommended for current architecture: L4 passthrough to endpoint TLS.

### Helm chart for classic profile

Classic deployment is available as Helm chart in `deploy/helm/trusttunnel-classic/`.

Staging example command:

```bash
helm upgrade --install trusttunnel-classic deploy/helm/trusttunnel-classic \
  --namespace trusttunnel-staging \
  --create-namespace \
  -f deploy/helm/trusttunnel-classic/values-staging.yaml
```

Key values:

| Key | Purpose |
| --- | --- |
| `sidecarEnabled` | Enables/disables sidecar container and sidecar NetworkPolicy. |
| `resources.endpoint` | Endpoint container requests/limits. |
| `resources.sidecar` | Sidecar container requests/limits. |
| `nodeAffinity` | Node placement rules for Pod scheduling. |
| `podAntiAffinity` | Anti-affinity rules to spread Pods across nodes. |
| `endpoint.readiness` / `endpoint.liveness` | Endpoint probe timings and thresholds. |
| `sidecar.readiness` / `sidecar.liveness` | Sidecar probe timings and thresholds. |
| `lkBaseUrl` | Base URL of LK internal API for sidecar sync/heartbeat. |
| `syncIntervalSeconds` | Sidecar credential sync interval. |
| `heartbeatIntervalSeconds` | Sidecar heartbeat push interval. |

## 4. systemd

Use `scripts/trusttunnel.service.template` as baseline.

Key points:
- set `WorkingDirectory` to config directory;
- `ExecStart` example: `/opt/trusttunnel/trusttunnel_endpoint vpn.toml hosts.toml`;
- enable restart on failure.

## 5. Runtime environment variables

Documented real usage in current fork:
- `hmac_secret_env` in `[auth.jwt]` points to ENV variable containing HS256 secret.

No other mandatory runtime env vars are required by endpoint startup path.

## 6. Classic mode with Sidecar Agent

For Classic production mode run two containers in one Pod:
- `trusttunnel_endpoint`
- `trusttunnel_sidecar_agent`

Both containers share writable volume `/runtime`:
- agent writes `/runtime/accounts.toml` atomically;
- endpoint reads same file through `credentials_file` in `vpn.toml`;
- with `TRUSTTUNNEL_RELOAD_MODE=signal` agent sends `SIGHUP` to `TRUSTTUNNEL_PID` (default `1`).

Required sidecar env:
- `LK_INTERNAL_BASE_URL`
- `INTERNAL_AGENT_TOKEN`
- `NODE_ID`
- `SYNC_INTERVAL_SECONDS` (default: `30`)
- `HEARTBEAT_INTERVAL_SECONDS` (default: `10`)
- `CREDENTIALS_PATH` (default: `/runtime/accounts.toml`)
- `TRUSTTUNNEL_RELOAD_MODE` (default: `signal`)
- `TRUSTTUNNEL_PID` (default: `1`)
- `AGENT_PORT` (default: `9105`)

Metrics semantics in sidecar:
- heartbeat push runs on `HEARTBEAT_INTERVAL_SECONDS` cadence;
- sidecar health probe checks `127.0.0.1:AGENT_PORT` before posting heartbeat;
- if endpoint is down, sidecar reports `active_connections=0` and degraded status.

Kubernetes notes:
- use `envFrom.secretRef` for `INTERNAL_AGENT_TOKEN`;
- keep LK internal API reachable only inside private network;
- enable `shareProcessNamespace: true` so sidecar can signal endpoint PID;
- restart is not required for credentials update.

### Operational constraints (timeouts/retries)

Agent calls to LK (`accounts`, `sync-report`, `heartbeat`) share the same HTTP envelope:
- `connect_timeout = 2s`;
- `request_timeout = 5s` (total request budget).

Sync retries use exponential backoff: `1s`, `2s`, `4s` (3 retries / 4 attempts total).

Operational budget for one worst-case sync cycle:
- backoff wait: `1 + 2 + 4 = 7s`;
- 4 timed-out requests: `4 * 5s = 20s`;
- total upper bound: `27s` (`<= 30s` target when `SYNC_INTERVAL_SECONDS=30`).

### Kubernetes probes for sidecar and endpoint

Recommended probe profile from the baseline manifest:

- endpoint container (`trusttunnel_endpoint`):
  - `readinessProbe`: `tcpSocket:8443`, `initialDelaySeconds: 8`, `periodSeconds: 10`, `failureThreshold: 3`;
  - `livenessProbe`: `tcpSocket:8443`, `initialDelaySeconds: 20`, `periodSeconds: 20`, `failureThreshold: 3`.

- sidecar container (`trusttunnel_sidecar_agent`):
  - `readinessProbe`: `GET /healthz` on `AGENT_PORT`, `initialDelaySeconds: 5`, `periodSeconds: 10`, `failureThreshold: 3`;
  - `livenessProbe`: `GET /healthz` on `AGENT_PORT`, `initialDelaySeconds: 15`, `periodSeconds: 10`, `failureThreshold: 3`.

Probe tuning guidance:
- keep sidecar `periodSeconds <= HEARTBEAT_INTERVAL_SECONDS` so degraded state appears before long heartbeat blind zones;
- if LK is intermittently slow, prefer increasing `failureThreshold` before increasing HTTP timeouts;
- avoid overly aggressive liveness restarts: retry logic is already built into agent LK requests.

## 7. Smoke-проверка classic deployment (kind/minikube)

Для базовой проверки Kubernetes-манифеста используйте скрипт:

```bash
scripts/ci/k8s_classic_smoke.sh
```

Что делает smoke:
- применяет `deploy/k8s/trusttunnel-classic.yaml` в отдельный namespace;
- дожидается `Ready` у Pod/Deployment;
- проверяет sidecar `GET /healthz` и `GET /metrics` через `kubectl port-forward`;
- выполняет базовую TCP-проверку доступности endpoint (`8443`) и sidecar (`9105`) внутри Pod.

Поддерживаемые переменные окружения:
- `NAMESPACE` (default: `trusttunnel-smoke`);
- `WAIT_TIMEOUT` (default: `180s`);
- `MANIFEST_PATH` (default: `deploy/k8s/trusttunnel-classic.yaml`);
- `ENDPOINT_IMAGE` и `SIDECAR_IMAGE` — переопределение image в манифесте для smoke.

Для kind/minikube при заданных `ENDPOINT_IMAGE`/`SIDECAR_IMAGE` скрипт автоматически делает `kind load docker-image` или `minikube image load`.
