# TASK_TT_04 — Helm chart

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
- Helm chart для sidecar
- RBAC для ConfigMap
- Values: lk.wsEndpoint, tokenSecretName, maxClients

## Acceptance criteria
- helm install запускает sidecar с env и rbac

## Progress
- [x] Added sidecar chart values for `lk.wsEndpoint`, `tokenSecretName`, `tokenSecretKey`, `maxClients`.
- [x] Added sidecar ConfigMap sync values (`clientsConfigMap.{name,namespace,key}`).
- [x] Added ServiceAccount + Role + RoleBinding templates for ConfigMap permissions.
- [x] Wired deployment env vars to new sidecar values and removed legacy `existingSecretName`.
