# Runbook: Classic Sidecar

- **Scope:** Операционная диагностика и восстановление Classic sidecar, который синхронизирует аккаунты с LK и публикует health/metrics.
- **Applies to:** Classic (active)
- **Last updated:** 2026-02-28

## 0. Быстрые переменные и доступ

Перед диагностикой задайте переменные окружения:

```bash
export NS=trusttunnel
export APP_LABEL=classic-sidecar
export POD=$(kubectl -n "$NS" get pod -l app="$APP_LABEL" -o jsonpath='{.items[0].metadata.name}')
```

Базовые проверки:

```bash
kubectl -n "$NS" get pods -l app="$APP_LABEL" -o wide
kubectl -n "$NS" describe pod "$POD"
kubectl -n "$NS" logs "$POD" --since=15m
```

Проброс локальных портов для health/metrics:

```bash
kubectl -n "$NS" port-forward pod/"$POD" 18080:8080 19090:9090
curl -fsS http://127.0.0.1:18080/healthz
curl -fsS http://127.0.0.1:19090/metrics | head -n 80
```

---

## 1. Сценарий: LK unreachable

### Симптомы

- В логах sidecar повторяются ошибки сетевого доступа к LK (`timeout`, `connection refused`, `no route to host`, `TLS handshake failed`).
- Рост `lk_timeout_total` и/или ошибок запросов к LK.
- `healthz` может возвращать degraded/not ready (в зависимости от реализации), синк не обновляется.

### Диагностика (точные команды)

```bash
kubectl -n "$NS" logs "$POD" --since=30m | tail -n 200
kubectl -n "$NS" describe pod "$POD"
kubectl -n "$NS" port-forward pod/"$POD" 18080:8080 19090:9090
curl -i http://127.0.0.1:18080/healthz
curl -s http://127.0.0.1:19090/metrics | rg 'lk_timeout_total|lk_request_duration_seconds|node_sync_failures_total|agent_last_sync_timestamp_seconds'
```

Дополнительно проверить сетевой путь до LK из pod:

```bash
kubectl -n "$NS" exec "$POD" -- sh -c 'getent hosts lk.internal || nslookup lk.internal'
kubectl -n "$NS" exec "$POD" -- sh -c 'curl -vk --max-time 5 https://lk.internal/health || true'
```

### Remediation

1. Проверить доступность LK (service/endpoints/ingress) и внутренний DNS.
2. Проверить egress policy/firewall/NAT для namespace sidecar.
3. Проверить TLS trust chain sidecar (CA bundle, cert rotation).
4. Временно увеличить retry/backoff/timeout sidecar (если есть параметр конфигурации), чтобы пережить деградацию LK.
5. После восстановления LK убедиться, что прошёл успешный sync и обновился `agent_last_sync_timestamp_seconds`.

### Rollback

- Если проблема началась после релиза sidecar, откатить deployment на предыдущий revision:

```bash
kubectl -n "$NS" rollout undo deployment/classic-sidecar
kubectl -n "$NS" rollout status deployment/classic-sidecar
```

- Если менялись NetworkPolicy/egress правила — вернуть предыдущий манифест из GitOps/Helm release.
- Если менялись cert/secret для LK — вернуть предыдущую версию секрета и рестартовать pod.

---

## 2. Сценарий: node sync broken

### Симптомы

- Sidecar жив, heartbeat может быть успешным, но учётки на ноде не обновляются.
- Рост `node_sync_failures_total`.
- Устаревший `agent_last_sync_timestamp_seconds` (давно не менялся).
- Логи содержат ошибки checksum mismatch, apply/update failure, write permissions error.

### Диагностика (точные команды)

```bash
kubectl -n "$NS" logs "$POD" --since=30m | tail -n 300
kubectl -n "$NS" describe pod "$POD"
kubectl -n "$NS" port-forward pod/"$POD" 18080:8080 19090:9090
curl -i http://127.0.0.1:18080/healthz
curl -s http://127.0.0.1:19090/metrics | rg 'node_sync_failures_total|sync_duration_seconds|agent_last_sync_timestamp_seconds|agent_heartbeat_success_total|agent_heartbeat_failure_total'
```

Проверка прав/volume для файла credentials (пример):

```bash
kubectl -n "$NS" exec "$POD" -- sh -c 'id && ls -l /var/lib/trusttunnel && ls -l /var/lib/trusttunnel/credentials.toml'
```

### Remediation

1. Проверить корректность payload от LK (version/checksum/accounts), особенно сортировку и canonical JSON для checksum.
2. Проверить write permissions на runtime credentials file/volumeMount.
3. Проверить, что `node_id` sidecar совпадает с ожидаемым на LK.
4. Исправить ошибочный конфиг sidecar (пути, формат credentials, feature flags sync).
5. Перезапустить pod после фикса и убедиться, что счётчик ошибок перестал расти, а timestamp синка обновился.

### Rollback

- Откатить sidecar config/deployment до последней стабильной версии.
- Вернуть предыдущий секрет/ConfigMap с рабочими путями и правами.
- Если поломка вызвана новым форматом ответа LK, временно вернуть LK API contract к предыдущей совместимой версии.

---

## 3. Сценарий: sidecar unhealthy

### Симптомы

- Pod в `CrashLoopBackOff`, `NotReady`, либо часто перезапускается.
- `curl /healthz` возвращает non-200 или не отвечает.
- Высокий уровень ошибок heartbeat/sync, метрики недоступны.

### Диагностика (точные команды)

```bash
kubectl -n "$NS" get pod "$POD" -o wide
kubectl -n "$NS" describe pod "$POD"
kubectl -n "$NS" logs "$POD" --previous --tail=200
kubectl -n "$NS" logs "$POD" --tail=200
kubectl -n "$NS" port-forward pod/"$POD" 18080:8080 19090:9090
curl -i --max-time 3 http://127.0.0.1:18080/healthz
curl -s --max-time 3 http://127.0.0.1:19090/metrics | head -n 120
```

### Remediation

1. Исправить причину падения по логам (`env`/secret not found, bad config, panic, OOMKilled).
2. Проверить и скорректировать `livenessProbe` / `readinessProbe` таймауты при необходимости.
3. Проверить лимиты/реквесты CPU/RAM; при OOM увеличить memory limit/request.
4. Проверить доступность зависимостей (LK, DNS, volume mounts).
5. После фикса дождаться `Ready=True`, проверить `/healthz` и `/metrics`.

### Rollback

- Быстрый откат deployment:

```bash
kubectl -n "$NS" rollout undo deployment/classic-sidecar
kubectl -n "$NS" rollout status deployment/classic-sidecar
```

- Вернуть предыдущие probe/resource значения через Helm/GitOps rollback.
- При необходимости масштабировать в 0 и обратно после восстановления конфигурации:

```bash
kubectl -n "$NS" scale deployment/classic-sidecar --replicas=0
kubectl -n "$NS" scale deployment/classic-sidecar --replicas=1
```

---

## 4. Готовые PromQL-запросы

### Timeout (LK)

```promql
sum(increase(lk_timeout_total[10m])) > 2
```

```promql
sum by (op) (rate(lk_request_duration_seconds_sum[5m]))
/
clamp_min(sum by (op) (rate(lk_request_duration_seconds_count[5m])), 1)
```

### Error growth (sync/heartbeat)

```promql
sum(increase(node_sync_failures_total[10m])) > 2
```

```promql
sum(increase(agent_heartbeat_failure_total[15m]))
/
clamp_min(
  sum(increase(agent_heartbeat_success_total[15m]))
  + sum(increase(agent_heartbeat_failure_total[15m])),
  1
) > 0.1
```

### Sync degradation / stale state

```promql
(time() - max(agent_last_sync_timestamp_seconds) > 300)
  OR absent(agent_last_sync_timestamp_seconds)
```

```promql
max(sync_duration_seconds) > 30
```

```promql
histogram_quantile(
  0.95,
  sum(rate(vpn_request_latency_seconds_bucket[5m])) by (le, protocol)
)
```

## 5. Критерии восстановления

Считать инцидент закрытым, когда одновременно выполняется:

- `/healthz` стабильно возвращает 200.
- `/metrics` доступен и показывает обновление timestamp/heartbeat.
- За последние 15 минут не растут `lk_timeout_total` и `node_sync_failures_total`.
- После rollback/remediation проверено, что deployment в состоянии `rollout status: successfully rolled out`.


---

## 6. CI smoke и ручной запуск e2e

Локальный/CI smoke сценарий sidecar:

```bash
./scripts/ci/lk_sidecar_e2e_smoke.sh
```

Что проверяется скриптом:

- mock-LK сценарии `normal`, `delayed`, `drop`;
- синхронизация `accounts.toml`;
- отправка `sync-report` и heartbeat;
- retry-backoff (`1s/2s/4s`) и восстановление после ошибок.

Успешный проход содержит строку:

```text
E2E smoke passed
```

### Быстрый дебаг "sidecar failing to register"

1. Проверить секреты и обязательные env (`INTERNAL_AGENT_TOKEN`, `LK_INTERNAL_BASE_URL`, `NODE_ID`).
2. Проверить reachability LK из pod и DNS (`kubectl exec ... curl ...`).
3. Проверить `/healthz` и логи sidecar на backoff/retry.
4. Проверить, что runtime volume writable и `accounts.toml` обновляется.
5. Если проблема после релиза — сделать `rollout undo` и зафиксировать failing payload/response от LK.
