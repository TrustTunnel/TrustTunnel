____________________________________________________________________
## Sidecar external checklist requirement
1. Sidecar обязан при старте создать внешний чек-лист задач (файл/ConfigMap/объект в хранилище), где каждая задача по умолчанию отмечена **красным крестиком** (не выполнено).
2. По выполнении каждой задачи sidecar отмечает её **зелёным** и в том же месте добавляет рапорт под названием **«актработа»** — сводку выполненных задач (human-readable + JSON).
3. Чек-лист и акт должны быть доступны из LK (админка) по ссылке на ноду.
Пример JSON:
{
 "checklist":[{"id":"sync-configmap","title":"Sync ConfigMap","status":"pending"}],
 "akt": {"generated_at":"...","tasks_completed":["sync-configmap"],"summary":"..."}
}
Acceptance: в админке доступен checklist и akt ссылка после выполнения задач.
____________________________________________________________________

# Nodes API spec

## WebSocket messages
- `register` — sidecar registration payload.
- `heartbeat` — periodic node stats (`clients_count`, `checklist_url`, `akt_url`).
- `command` — LK -> sidecar command dispatch.
- `ack` — immediate sidecar acknowledgement for command receive.
- `result` — command execution result with optional `akt_url`.
- `checklist_update` — current checklist state sync.
- `log` — live text log frame for admin console.

## REST endpoints
- `POST /api/nodes/register`
- `POST /api/nodes/heartbeat`
- `POST /api/nodes/configsync`
- `POST /api/nodes/{id}/command`
- `GET /api/nodes/{id}/checklist`
