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

# Sidecar integration notes

Epic branch: `epic/sidecar-core`

## Implemented baseline chain
`register -> heartbeat -> command ack/result -> local clients sync stub -> akt artifact`

### Sidecar runtime (TT)
- WS client with reconnect: `wss://lk/api/nodes/ws?node_id=...&token=...`.
- Register message on connect.
- Heartbeat every 30s includes:
  - `clients_count`
  - `checklist_url`
  - `akt_url`
- Command handling:
  - idempotency by `command_id`
  - immediate `ack`
  - command result with `akt_url`
- External checklist file is initialized with pending tasks and updated to done.
- Akt writer stores:
  - human report (`.txt`)
  - JSON report (`.json`)
  - path: `artifacts/akt/{command_id}-{node}-{ts}.json`

### ConfigMap/dev sync behavior
- Dev mode writes generated clients into local file `/tmp/clients.json` (override with `SIDECAR_CLIENTS_EXPORT_PATH`).
- Optional native k8s sync updates a ConfigMap via Kubernetes API when `SIDECAR_CLIENTS_CONFIGMAP` is set.
- ConfigMap sync uses merge-patch with retry on HTTP `409` conflicts.
- `clients_count` is derived from synced credentials payload and reported in heartbeat.

## Manual smoke test
1. Set env:
   - `NODE_ID=node-1`
   - `LK_TOKEN=test-token`
   - `LK_WS_ENDPOINT=ws://localhost:8080/api/nodes/ws`
2. Run sidecar.
3. Send `command` frame from mock LK.
4. Verify:
   - `ack` arrives immediately
   - `result` contains `akt_url`
   - checklist has done statuses
   - `artifacts/akt/*.json` created.
