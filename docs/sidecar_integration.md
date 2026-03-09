____________________________________________________________________
## Sidecar external checklist requirement
1. Sidecar обязан при старте создать внешний чек-лист задач (файл/ConfigMap/объект в хранилище), где каждая задача по умолчанию отмечена **красным крестиком** (не выполнено).
2. По выполнении каждой задачи sidecar отмечает её **зелёным** и в том же месте добавляет рапорт под названием **«актработа»** — сводку выполненных задач (human-readable + JSON).
3. Чек-лист и акт должны быть доступны из LK (админка) по ссылке на ноду.
Пример JSON:
{
 "checklist":[{"id":"sync-runtime","title":"Apply runtime payload","status":"pending"}],
 "akt": {"generated_at":"...","tasks_completed":["sync-runtime"],"summary":"..."}
}
Acceptance: в админке доступен checklist и akt ссылка после выполнения задач.
____________________________________________________________________

# Sidecar integration notes

Epic branch: `epic/sidecar-core`

## Implemented baseline chain
`register -> heartbeat -> command ack/result -> runtime payload apply -> akt artifact`

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

### Runtime payload behavior
- Sidecar applies runtime config only from `runtime_payload.runtime_config`.
- Runtime file is written atomically into `CREDENTIALS_PATH` and then endpoint reload is triggered.
- Legacy fallback from `runtime_payload.credentials` is gated by `SIDECAR_ENABLE_LEGACY_CREDENTIALS_FLOW=true` and is disabled by default in production.
- `clients_count` is derived from rendered runtime config and reported in heartbeat.

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

## Helm deployment (sidecar core)
- Chart: `deploy/helm/trusttunnel-classic`.
- Sidecar values for epic:
  - `sidecar.lk.wsEndpoint`
  - `sidecar.tokenSecretName` + `sidecar.tokenSecretKey`
  - `sidecar.maxClients`
  - `nodeConfigMap.{enabled,name}` with fixed technical keys (`node_name/public_host/endpoint_ip/desired_pool_size/weight/stage/is_enabled`)
  - `runtime.enableLegacyCredentialsFlow` for migration-only fallback


## Epic coverage matrix
- ✅ Registration: WS register frame + REST fallback contracts documented.
- ✅ Heartbeat: 30s loop with clients/checklist/akt metadata.
- ✅ Command bus: queued/sent/ack/in_progress/done/failed state machine in API spec.
- ✅ Runtime sync: `runtime_payload.runtime_config` atomic apply + reload.
- ✅ Legacy flow: optional feature-flagged credentials fallback.
- ✅ Akt writer: human+json artifacts and `akt_url` propagation.
- ✅ Helm/Docker/CI: tracked in `TASK_TT_04` and `TASK_TT_05` progress sections.

## QA quick commands
```bash
# unit (sidecar-go prototype)
cd docs/tasks/sidecar-agent-sync-lk/sidecar && go test ./...

# integration smoke
bash scripts/ci/lk_sidecar_e2e_smoke.sh
```
