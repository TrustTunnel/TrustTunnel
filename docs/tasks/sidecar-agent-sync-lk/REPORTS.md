# REPORTS — прогресс по задачам sidecar-agent

> В этот файл добавляются отчеты **по каждой задаче из `README.md`** по мере выполнения.

## Шаблон записи
- Дата/время:
- Задача: (например, `3) Injection`)
- Статус: (`in_progress` | `blocked` | `done`)
- Что сделано:
- Что осталось:
- Блокеры/риски:
- Ссылки: (PR/commit/issue/runbook)

---

## Журнал
<!-- Добавляйте новые записи сверху -->

- Дата/время: 2026-03-05 00:00 UTC
  - Задача: `epic/sidecar-core (TASK_TT_01..05)`
  - Статус: `done`
  - Что сделано: дополнены спецификации `api/specs/nodes.md` и `docs/sidecar_integration.md` (WS/REST контракты, lifecycle команд, QA команды), подтверждён scope эпика sidecar core.
  - Что осталось: LK-side implementation (admin UI + backend queue persistence) в соответствующем LK репозитории.
  - Блокеры/риски: unit smoke для go-sidecar prototype не запускается в текущем окружении из-за недоступности `github.com/gorilla/websocket` (HTTP 403).
  - Ссылки: commit (текущая ветка)

- Дата/время: 2026-03-04 16:05 UTC
  - Задача: `2) Как sidecar получает ConfigMap/Secret (вариант A)`
  - Статус: `in_progress`
  - Что сделано: `sidecar-agent` теперь повторно пытается подключить inotify watcher на каждом sync-тикe, если на старте директории `configs/secrets` ещё не существовали; это снимает необходимость в рестарте контейнера после позднего появления mount-точек.
  - Что осталось: добавить криптографическое заполнение `value_encrypted` по LK public key flow и завершить injector wiring.
  - Блокеры/риски: при длинном окне отсутствия mount-директорий повторные попытки инициализации watcher происходят по расписанию sync-loop и логически зависят от `SYNC_INTERVAL_SECONDS`.
  - Ссылки: commit (текущая ветка)

- Дата/время: 2026-03-04 15:20 UTC
  - Задача: `2) Как sidecar получает ConfigMap/Secret (вариант A)` + `3) Injection`
  - Статус: `in_progress`
  - Что сделано: Helm deployment для classic расширен wiring’ом sidecar sync-ресурсов: добавлены значения `sidecar.sync.configMaps/secrets/secretKeys`, автоматические `volumes+volumeMounts` в `/etc/trusttunnel/configs/<name>` и `/etc/trusttunnel/secrets/<name>`, а также pod-context env (`CLUSTER_ID`, `POD_*`, `NODE_NAME`) и `TRUSTTUNNEL_*` env для file-sync протокола с LK.
  - Что осталось: реализовать mutating webhook injector, чтобы sidecar/mount wiring назначались автоматически по аннотациям pod без ручного Helm-конфига.
  - Блокеры/риски: текущий Helm-путь не валидирует имена ConfigMap/Secret на RFC1123 — некорректные имена приведут к ошибке рендера/применения манифеста.
  - Ссылки: commit (текущая ветка)

- Дата/время: 2026-03-04 13:25 UTC
  - Задача: `2) Как sidecar получает ConfigMap/Secret (вариант A)`
  - Статус: `in_progress`
  - Что сделано: добавлен inotify-триггер в `sidecar-agent` для mounted `configs/secrets` путей (`/etc/trusttunnel/configs`, `/etc/trusttunnel/secrets`) — при файловых событиях запускается внеочередной sync в LK, при этом сохраняется dedup по checksum.
  - Что осталось: завершить injector wiring (automated volumes+mounts) и внедрить шифрование opt-in secret keys в `value_encrypted`.
  - Блокеры/риски: inotify watcher активируется только для существующих директорий на старте sidecar; при позднем появлении mount-точек нужен отдельный re-watch цикл или рестарт контейнера.
  - Ссылки: commit (текущая ветка)

- Дата/время: 2026-03-04 12:40 UTC
  - Задача: `2) Как sidecar получает ConfigMap/Secret (вариант A)` + `4) Протокол с LK`
  - Статус: `in_progress`
  - Что сделано: в `sidecar-agent` добавлен сбор mounted-файлов из `/etc/trusttunnel/configs` и `/etc/trusttunnel/secrets`, дедуп отправки по checksum и отправка на `POST /api/trusttunnel/nodes/:id/configs`; для secrets отправляется masked meta и opt-in поле `value_encrypted` для ключей из `TRUSTTUNNEL_SYNC_SECRET_KEYS`.
  - Что осталось: заменить polling на fsnotify/inotify, добавить реальное шифрование значения secret-key перед заполнением `value_encrypted`, завершить injector wiring для автоматического mount ресурсов.
  - Блокеры/риски: контракт по шифрованию opt-in ключей требует LK public key flow (в текущем шаге нет крипто-реализации).
  - Ссылки: commit (текущая ветка)

- Дата/время: 2026-03-04 10:05 UTC
  - Задача: `4) Протокол с LK`
  - Статус: `in_progress`
  - Что сделано: в `sidecar-agent` добавлена регистрация через `POST /api/trusttunnel/nodes/register` с сохранением `node_id/node_token` в runtime-состоянии и heartbeat отправка на `POST /api/trusttunnel/nodes/:id/heartbeat`.
  - Что осталось: реализовать отправку `configs`/`secrets` на `POST /api/trusttunnel/nodes/:id/configs` по контракту ТЗ.
  - Блокеры/риски: sidecar crate сейчас не подключен к workspace root, поэтому unit/integration test запускаются ограниченно без обновления workspace-конфига.
  - Ссылки: commit (текущая ветка)

- Дата/время: 2026-03-04 08:16 UTC
  - Задача: `0) Ключевое ограничение (ОБЯЗАТЕЛЬНО)`
  - Статус: `in_progress`
  - Что сделано: подтверждена текущая модель с sidecar в pod и без `hostNetwork`/`hostPort` в Helm deployment.
  - Что осталось: зафиксировать в отдельной проверке окружения, что ingress-controller на каждой ноде уже слушает 443.
  - Блокеры/риски: топологию ingress нужно верифицировать в кластере (в репозитории нет runtime-состояния).
  - Ссылки: commit (текущая ветка)

- Дата/время: 2026-03-04 08:16 UTC
  - Задача: `1) Назначение sidecar`
  - Статус: `in_progress`
  - Что сделано: отмечен фактически реализованный baseline (sync-loop, checksum, sync-report, health/metrics, retry).
  - Что осталось: добавить pod-context в payload и перейти с credential-sync на sync ConfigMap/Secret по ТЗ.
  - Блокеры/риски: требуется отдельный контракт LK под новый payload configs/secrets.
  - Ссылки: commit (текущая ветка)

- Дата/время: 2026-03-04 08:16 UTC
  - Задача: `3) Injection`
  - Статус: `in_progress`
  - Что сделано: в чеклисте зафиксировано текущее состояние — sidecar добавляется через Helm-шаблон deployment.
  - Что осталось: реализовать mutating webhook и поддержку аннотаций из ТЗ.
  - Блокеры/риски: нужен отдельный компонент webhook + cert management + admission registration.
  - Ссылки: commit (текущая ветка)

- Дата/время: 2026-03-04 08:16 UTC
  - Задача: `6) Надежность`
  - Статус: `done`
  - Что сделано: подтверждены retry with exponential backoff и дедуп отправок по неизменному `version/checksum`.
  - Что осталось: добавить локальный кэш последней версии для восстановления после рестарта sidecar.
  - Блокеры/риски: без persist-кэша возможна повторная отправка после рестарта.
  - Ссылки: commit (текущая ветка)

- Дата/время: 2026-03-04 08:16 UTC
  - Задача: `7) Observability`
  - Статус: `in_progress`
  - Что сделано: подтверждены ключевые логи (`sync ok/error`, `retry`) и health-сигнал в метриках.
  - Что осталось: выделить отдельные метрики `last_sync_ts`, `sync_errors_total`, `lk_latency_ms` по новому ТЗ.
  - Блокеры/риски: потребуется расширение sidecar telemetry-схемы и dashboard.
  - Ссылки: commit (текущая ветка)
