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

## WebSocket transport
Endpoint: `wss://lk/api/nodes/ws?node_id={node_id}&token={token}`

### Message: `register` (sidecar -> LK)
```json
{"kind":"register","payload":{"node_id":"node-1","fingerprint":"fp","ingress_ip":"10.0.0.12","max_clients":250}}
```

### Message: `heartbeat` (sidecar -> LK)
```json
{"kind":"heartbeat","payload":{"node_id":"node-1","clients_count":12,"status":"online","checklist_url":"file:///tmp/checklist.json","akt_url":"file:///artifacts/akt/cmd-1-node-1-1730000000.json"}}
```

### Message: `command` (LK -> sidecar)
```json
{"kind":"command","command_id":"cmd-1","type":"apply_configmap","payload":{"force":true}}
```

### Message: `ack` (sidecar -> LK)
```json
{"kind":"ack","command_id":"cmd-1","status":"accepted"}
```

### Message: `result` (sidecar -> LK)
```json
{"kind":"result","command_id":"cmd-1","status":"done","akt_url":"file:///artifacts/akt/cmd-1-node-1-1730000000.json","details":{"note":"command executed"}}
```

### Message: `checklist_update` (sidecar -> LK)
```json
{"kind":"checklist_update","payload":{"checklist":[{"id":"sync-configmap","status":"done"}],"akt":{"summary":"..."}}}
```

### Message: `log` (sidecar -> LK)
```json
{"kind":"log","payload":{"level":"info","message":"apply_configmap started"}}
```

## REST endpoints
- `POST /api/nodes/register` — fallback registration when WS unavailable.
- `POST /api/nodes/heartbeat` — fallback heartbeat endpoint.
- `POST /api/nodes/configsync` — accept generated clients/config status.
- `POST /api/nodes/{id}/command` — enqueue command for node.
- `GET /api/nodes/{id}/command` — inspect latest command state.
- `GET /api/nodes/{id}/checklist` — fetch current checklist + akt URL/content metadata.


## Command lifecycle
- LK writes command to queue with `status=queued`.
- LK WS dispatcher marks command as `sent` when pushed to connected sidecar.
- Sidecar replies `ack` immediately, LK stores `status=ack` + `acked_at`.
- During execution LK may store `in_progress` heartbeats for long jobs.
- Final sidecar `result` sets `done` or `failed` with `result_payload` and `akt_url`.
- If `ack` timeout elapses, LK returns command to `queued` and increments retry counter (`retry_count`).

## Persistence contract (LK)
- `node_commands`: queue, delivery state, retries, ack timeout timestamps.
- `node_checklists`: latest external checklist snapshot + `akt` metadata by node.
- `node_clients`: generated clients (`bulkGenerateForNode`), capped by `max_clients`.

## REST schema notes
- `POST /api/nodes/{id}/command` body:
  ```json
  {"type":"regenerate_clients|drain|apply_configmap","payload":{},"requested_by":"admin@lk"}
  ```
- `GET /api/nodes/{id}/checklist` response:
  ```json
  {"node_id":"node-1","checklist":[{"id":"register-node","status":"done"}],"akt_url":"file:///artifacts/akt/cmd-1-node-1-1730000000.json"}
  ```
