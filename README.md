# TrustTunnel Server Fork (LK + TrustTunnel)

Этот репозиторий содержит серверный форк TrustTunnel для HTTPS/HTTP2/HTTP3-туннелирования и интеграции с LK.
Проект ориентирован на эксплуатацию: конфигурация endpoint, TLS/SNI, аутентификация (Basic/JWT), деплой и наблюдаемость.
Кодовая база предназначена для backend/DevOps сценариев, без пользовательских onboarding-сценариев.

Техническая документация: [docs/INDEX.md](docs/INDEX.md).

Операционный runbook для Classic sidecar: [docs/RUNBOOK_CLASSIC_SIDECAR.md](docs/RUNBOOK_CLASSIC_SIDECAR.md).

Режимы: **Classic** (текущий production-path) и **Modified** (planned, not enabled in this release).


## CI build/publish (GHCR)

`build.yml` публикует sidecar image в GHCR как `ghcr.io/<org>/trusttunnel:sha-<short_sha>` и (для `main`) `:latest`.

Локальная сборка и smoke:

```bash
docker build -f agent/Dockerfile -t trusttunnel:local .
docker run --rm -p 9105:9105   -e LK_INTERNAL_BASE_URL=http://127.0.0.1:9   -e INTERNAL_AGENT_TOKEN=local-token   -e NODE_ID=local-node   -e TRUSTTUNNEL_TCP_ADDR=127.0.0.1:18443   trusttunnel:local
```

Проверка health:

```bash
curl -fsS http://127.0.0.1:9105/healthz
```

## Helm/CD синхронизация тега

Chart `deploy/helm/trusttunnel-classic` использует единый sidecar image:

- `image.repository`
- `image.tag`

Для staging/prod CI/CD должен передавать тег из TrustTunnel CI outputs:

```bash
helm upgrade --install trusttunnel deploy/helm/trusttunnel-classic   --set image.tag="${sha_tag}"
```
