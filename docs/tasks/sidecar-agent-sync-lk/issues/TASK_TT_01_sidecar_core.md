# TASK_TT_01 — Sidecar core

Status: active
Priority: high

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
- WS client к LK
- register + heartbeat
- command handler (ack/result) + idempotency по command_id
- checklist updates + aktработа generation + akt_url reporting

## Acceptance criteria
- sidecar подключается и регистрирует ноду
- команды принимаются и возвращают result + akt_url
- checklist/akt доступны из LK

