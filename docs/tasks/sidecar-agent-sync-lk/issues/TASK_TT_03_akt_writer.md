# TASK_TT_03 — Akt writer + storage

Status: done
Priority: medium

## Sidecar external checklist requirement
1. Sidecar обязан при старте создать внешний чек-лист задач (файл/ConfigMap/объект в хранилище), где каждая задача по умолчанию отмечена **красным крестиком** (не выполнено).
2. По выполнении каждой задачи sidecar отмечает её **зелёным** и в том же месте добавляет рапорт под названием **«актработа»** — сводку выполненных задач (**human-readable** + **JSON**).
3. Чек-лист и акт должны быть доступны из LK (админка) по ссылке на ноду.

Пример формата (JSON):
```json
{
  "checklist": [
    {"id":"sync-configmap","title":"Sync ConfigMap","status":"pending"},
    {"id":"register-node","title":"Register node in LK","status":"pending"}
  ],
  "akt": {
    "generated_at":"2026-01-01T12:00:00Z",
    "tasks_completed":[{"id":"register-node","time":"...","notes":"..."}],
    "summary":"..."
  }
}
```

Acceptance (обязательное):
- при регистрации ноды в админке виден текущий checklist (pending/done);
- после выполнения задач доступен `актработа` (human + json) и ссылка на него хранится/отображается в LK.


## Описание
- Отделить akt writer в модуль:
  - human report + json
  - storage backend: ConfigMap (dev) / PV / S3 (prod)
- Возвращать akt_url в LK.

## Acceptance criteria
- актработа доступен по ссылке из LK



## Progress
- [x] Extracted akt writer flow into reusable sidecar methods.
- [x] Store both human-readable (`.txt`) and JSON (`.json`) reports.
- [x] Return and publish `akt_url` in command result and heartbeat.

akt_url: file://artifacts/akt/<command_id>-<node>-<timestamp>.json
