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

Current implementation for epic `epic/sidecar-core` in this repo covers TASK_TT_01 baseline:
- bootstrap checklist creation at sidecar startup;
- local akt artifact writer (`artifacts/akt/*.json` + `.txt`);
- heartbeat now includes `clients_count`, `checklist_url`, `akt_url`.

Pending for follow-up tasks:
- WS command bus (`command/ack/result` realtime loop);
- ConfigMap k8s API writer (currently local file-backed path);
- Helm/RBAC and CI smoke scenarios.
