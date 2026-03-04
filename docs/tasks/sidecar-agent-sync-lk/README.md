# TrustTunnel: план задач и ТЗ для sidecar-agent + auto sync конфигов/секретов + интеграция с ingress-на-нодах

Статус: **в работе**.

## Срез выполнения (итерация 2026-03-04)
- [x] Базовый sidecar-цикл синхронизации с LK уже реализован в `sidecar-agent` (polling, checksum-валидация, atomic apply, sync-report).
- [x] Реализованы ретраи с exponential backoff.
- [x] Реализован дедуп по `version+checksum` (повторный apply не выполняется).
- [x] Реализован heartbeat/health-сигнал в метриках sidecar.
- [ ] Injection через mutating webhook пока не реализован (используется Helm sidecar в deployment).
- [ ] Sync ConfigMap/Secret из mounted volumes (вариант A из этого ТЗ) пока не реализован.
- [ ] Регистрация по API `/api/trusttunnel/nodes/register` и протокол из этого ТЗ пока не реализованы.

## Правила ведения работ и отчетности
- Для **каждой задачи ниже** вести короткий прогресс-репорт по мере выполнения.
- Формат прогресса: дата/время, что сделано, что осталось, блокеры/риски, ссылка на PR/коммит (если есть).
- После полного выполнения задачи менять статус с `[ ]` на `[x]`.
- Если задача частично выполнена — оставлять `[ ]` и добавлять подпункт со статусом подзадач.
- Рекомендовано вести отчеты в файле `REPORTS.md` в этой же директории.

---

## [ ] 0) Ключевое ограничение (ОБЯЗАТЕЛЬНО)
- [ ] Ingress-controller уже запущен на каждой ноде и слушает 443.
- [ ] Ingress проксирует через Service в локальный pod.
- [ ] Sidecar НЕ слушает 443 и НЕ использует hostPort/hostNetwork.
- [ ] Sidecar работает внутри pod и решает задачи синхронизации и репортинга в LK.

Статус подзадач (2026-03-04):
- `in_progress`: ограничения зафиксированы, требуется закрепление через injector/deployment конфигурацию.

---

## [ ] 1) Назначение sidecar
Sidecar (`trusttunnel-agent`) должен:
- [ ] Определять “свой pod” и контекст (`pod_name`, `ns`, `uid`, `node_name`, `pod_ip`).
- [ ] Читать/отслеживать изменения ConfigMap’ов и Secret’ов, указанных для pod (через аннотации/инъекцию).
- [ ] Отправлять в LK:
  - [ ] версию конфигов (`content+checksum`) и историю изменений;
  - [ ] метаданные секретов (`masked`) и опционально зашифрованные значения разрешенных key’ев.
- [ ] Держать heartbeat/status (`last_seen`, `health`).
- [ ] Работать устойчиво при недоступности LK (`retry + cache`).

Статус подзадач (2026-03-04):
- `in_progress`: в текущей версии есть retry/backoff, sync-report и dedup по checksum/version, но отсутствует file-watch sync ConfigMap/Secret и pod context payload.

---

## [ ] 2) Как sidecar получает ConfigMap/Secret (вариант A = базовый)
Вариант A (рекомендуемый):
- [ ] Injector добавляет в pod:
  - [ ] sidecar контейнер;
  - [ ] volumes: configMap/secret, перечисленные в аннотациях;
  - [ ] readOnly volumeMount’ы в sidecar.
- [ ] Sidecar читает файлы из:
  - [ ] `/etc/trusttunnel/configs/<name>/...`
  - [ ] `/etc/trusttunnel/secrets/<name>/...`
- [ ] Отслеживание изменений через `inotify/fsnotify` по файловой системе.

Статус подзадач (2026-03-04):
- `blocked`: требуется реализация injector/mount wiring и watcher-цикла в sidecar.

(Опционально позже) Вариант B:
- [ ] Sidecar смотрит K8s API (watch). Требует RBAC. **Не делать на первом этапе без необходимости.**

---

## [ ] 3) Injection (автоматическое добавление sidecar)
Цель: автоматически добавлять sidecar в нужные pod.

Подходы:
- [ ] Mutating Admission Webhook (предпочтительно).
- [ ] Helm-templating вручную (временный быстрый путь).

Аннотации (пример):
- [ ] `trusttunnel.inject: "true"`
- [ ] `trusttunnel.sync.configmaps: "app-config,bot-config"`
- [ ] `trusttunnel.sync.secrets: "bot-secret"`
- [ ] `trusttunnel.sync.secret-keys: "telegram_token"` (опционально)

Поведение injector:
- [ ] Если `inject=true`:
  - [ ] добавить контейнер `trusttunnel-sidecar`;
  - [ ] добавить `volumes+mounts` для перечисленных ресурсов;
  - [ ] добавить env: `LK_URL`, `CLUSTER_ID`, `POD_NAME`, `POD_NAMESPACE`, `NODE_NAME`, `LK_PUBLIC_KEY_PATH`.

Acceptance:
- [ ] Любой pod с `trusttunnel.inject=true` получает sidecar автоматически.

Статус подзадач (2026-03-04):
- `blocked`: отсутствует готовый webhook/controller слой для sidecar injection.

---

## [ ] 4) Протокол с LK
- [ ] Регистрация:
  - [x] `POST /api/trusttunnel/nodes/register`
  - [x] получить `node_id` и `node_token`
- [ ] Отправка конфигов:
  - [ ] `POST /api/trusttunnel/nodes/:id/configs`
  - [ ] `configs`: список файлов (`path`, `content`, `checksum`)
- [ ] Secrets:
  - [ ] По умолчанию: только masked meta (`keys_count`, `checksum`)
  - [ ] Opt-in: только указанные `secret-keys` шифруются LK public key и отправляются как `value_encrypted`
- [ ] Heartbeat:
- [x] `POST /api/trusttunnel/nodes/:id/heartbeat` каждые 30s (или вместе с configs)

Статус подзадач (2026-03-04):
- `in_progress`: добавлены register + heartbeat по целевому контракту; требуется реализация payload для configs/secrets endpoint-ов.

---

## [ ] 5) Безопасность
- [ ] Все запросы — HTTPS.
- [ ] Node auth — короткоживущий JWT (`node_token`).
- [ ] Шифрование секретов:
  - [ ] sidecar использует LK public key (`RSA/OAEP` или `age`, решение за командой);
  - [ ] LK держит private key.
- [ ] NetworkPolicy (желательно): sidecar может ходить только в LK endpoint.
- [ ] Никаких cluster-wide прав (Variant A не требует RBAC на secrets).

Статус подзадач (2026-03-04):
- `in_progress`: HTTPS+auth есть, но нет реализованного encrypted opt-in секрета и NetworkPolicy ограничения egress.

---

## [ ] 6) Надежность
- [ ] Retrying с exponential backoff.
- [ ] Кеширование последних версий (на диск в `emptyDir` или memory), чтобы не терять изменения при временном падении LK.
- [ ] Дедуп: если checksum не поменялся — не слать повторно.

Статус подзадач (2026-03-04):
- `in_progress`: retry/backoff и dedup уже есть, но кэш для новой модели config/secret sync еще не внедрен.

---

## [ ] 7) Observability
- [ ] Логи: `registration ok`, `sync ok`, `sync errors`, `retry counters`.
- [ ] Метрики (опционально): `last_sync_ts`, `sync_errors_total`, `lk_latency_ms`.

Статус подзадач (2026-03-04):
- `in_progress`: базовые логи/метрики отправляются, требуется расширение набора целевыми показателями из ТЗ.

---

## [ ] 8) Definition of Done (TrustTunnel)
- [ ] Sidecar образ собирается и запускается рядом с pod.
- [ ] Injection работает.
- [ ] ConfigMap изменения отражаются в LK ≤ 30s.
- [ ] Secrets по умолчанию masked, opt-in encrypted keys работают.
- [ ] Никакого вмешательства в ingress/443/hostPort.

Статус подзадач (2026-03-04):
- `in_progress`: нужен полный цикл реализации задач 2-5 для закрытия DoD.
