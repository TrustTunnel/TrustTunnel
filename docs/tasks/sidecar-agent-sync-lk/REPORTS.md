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
