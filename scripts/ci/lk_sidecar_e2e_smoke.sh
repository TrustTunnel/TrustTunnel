#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP_DIR="$(mktemp -d)"
MOCK_PORT="${MOCK_PORT:-18080}"
ENDPOINT_ADDR="${ENDPOINT_ADDR:-127.0.0.1:18443}"
SYNC_INTERVAL_SECONDS="${SYNC_INTERVAL_SECONDS:-3}"
METRICS_PUSH_INTERVAL="${METRICS_PUSH_INTERVAL:-10}"
NODE_ID="${NODE_ID:-node-e2e}"
INTERNAL_AGENT_TOKEN="${INTERNAL_AGENT_TOKEN:-e2e-token}"

MOCK_LOG="${TMP_DIR}/mock-lk.log"
SIDECAR_LOG="${TMP_DIR}/sidecar.log"
RUNTIME_DIR="${TMP_DIR}/runtime"
SCENARIO_FILE="${TMP_DIR}/scenario.json"

require_cmd() { command -v "$1" >/dev/null 2>&1 || { echo "missing cmd: $1" >&2; exit 1; }; }
require_cmd cargo
require_cmd python3
require_cmd curl
require_cmd rg

cleanup() {
  set +e
  [[ -n "${SIDECAR_PID:-}" ]] && kill "${SIDECAR_PID}" >/dev/null 2>&1
  [[ -n "${MOCK_PID:-}" ]] && kill "${MOCK_PID}" >/dev/null 2>&1
  [[ -n "${ENDPOINT_EMU_PID:-}" ]] && kill "${ENDPOINT_EMU_PID}" >/dev/null 2>&1
  [[ -n "${RELOAD_PID:-}" ]] && kill "${RELOAD_PID}" >/dev/null 2>&1
  wait "${SIDECAR_PID:-}" "${MOCK_PID:-}" "${ENDPOINT_EMU_PID:-}" "${RELOAD_PID:-}" >/dev/null 2>&1
  echo "=== sidecar log ==="
  [[ -f "${SIDECAR_LOG}" ]] && tail -n 120 "${SIDECAR_LOG}" || true
  echo "=== mock lk log ==="
  [[ -f "${MOCK_LOG}" ]] && tail -n 120 "${MOCK_LOG}" || true
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

mkdir -p "${RUNTIME_DIR}"
(
  cd "${ROOT_DIR}"
  cargo build --quiet -p trusttunnel_sidecar_agent --bin trusttunnel_sidecar_agent
)
SIDECAR_BIN="${ROOT_DIR}/target/debug/trusttunnel_sidecar_agent"

cat > "${SCENARIO_FILE}" <<JSON
{"mode":"normal","version":"v1"}
JSON

python3 -u - "${MOCK_PORT}" "${SCENARIO_FILE}" "${TMP_DIR}" >"${MOCK_LOG}" 2>&1 <<'PY' &
import datetime, hashlib, json, socket, socketserver, sys, time
from http.server import BaseHTTPRequestHandler
from pathlib import Path
port = int(sys.argv[1]); scenario_file = Path(sys.argv[2]); out_dir = Path(sys.argv[3]); out_dir.mkdir(parents=True, exist_ok=True)
sync_reports = out_dir / "sync_reports.ndjson"; heartbeats = out_dir / "heartbeats.ndjson"
def read_scenario(): return json.loads(scenario_file.read_text(encoding="utf-8"))
def now_iso(): return datetime.datetime.now(datetime.timezone.utc).isoformat()
def snapshot(version):
    creds=[{"username":"user1","password":f"pass-{version}"}]
    raw='{"credentials":['+json.dumps(creds[0], separators=(",", ":"))+']}'; checksum=hashlib.sha256(raw.encode()).hexdigest()
    return {"version":version,"credentials":creds,"checksum":checksum}
class S(socketserver.ThreadingMixIn, socketserver.TCPServer): allow_reuse_address=True
class H(BaseHTTPRequestHandler):
    def _json(self, code, payload):
        body=json.dumps(payload).encode(); self.send_response(code); self.send_header("content-type","application/json"); self.send_header("content-length",str(len(body))); self.end_headers(); self.wfile.write(body)
    def _in(self):
        n=int(self.headers.get("content-length","0")); return json.loads(self.rfile.read(n).decode()) if n else {}
    def _append(self, path, payload):
        with path.open("a", encoding="utf-8") as f: f.write(json.dumps(payload, ensure_ascii=False)+"\n")
    def do_POST(self):
        if self.path == "/__admin/scenario":
            data=self._in(); scenario_file.write_text(json.dumps(data), encoding="utf-8"); self._json(200,{"ok":True}); return
        if self.path.endswith("/sync-report"):
            self._append(sync_reports,{"received_at":now_iso(),"body":self._in()}); self._json(200,{"ok":True}); return
        if self.path.endswith("/metrics") or self.path.endswith("/heartbeat"):
            self._append(heartbeats,{"received_at":now_iso(),"body":self._in()}); self._json(200,{"ok":True}); return
        self._json(404,{"error":"not found"})
    def do_GET(self):
        if self.path.endswith("/credentials-snapshot") or self.path.endswith("/accounts"):
            sc=read_scenario(); mode=sc.get("mode","normal")
            if mode=="delayed": time.sleep(20)
            elif mode=="drop": self.connection.shutdown(socket.SHUT_RDWR); self.connection.close(); return
            self._json(200,snapshot(sc.get("version","v1"))); return
        self._json(404,{"error":"not found"})
    def log_message(self, fmt, *args): print(f"[{now_iso()}] {self.client_address[0]} {fmt % args}")
with S(("127.0.0.1", port), H) as srv: print(f"mock lk listening on 127.0.0.1:{port}"); srv.serve_forever()
PY
MOCK_PID=$!

ENDPOINT_HOST="${ENDPOINT_ADDR%%:*}"; ENDPOINT_PORT="${ENDPOINT_ADDR##*:}"
python3 -u - "${ENDPOINT_HOST}" "${ENDPOINT_PORT}" <<'PY' >/dev/null 2>&1 &
import socket, sys
host=sys.argv[1]; port=int(sys.argv[2])
s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind((host, port)); s.listen()
while True:
    c,_=s.accept(); c.close()
PY
ENDPOINT_EMU_PID=$!

bash -c 'trap "" HUP USR1 USR2; while true; do sleep 3600; done' &
RELOAD_PID=$!

for _ in $(seq 1 40); do
  curl -fsS "http://127.0.0.1:${MOCK_PORT}/internal/trusttunnel/nodes/${NODE_ID}/credentials-snapshot" >/dev/null 2>&1 && break
  sleep 0.2
done

(
  cd "${ROOT_DIR}"
  exec env \
    RUST_LOG=info HTTP_PROXY= HTTPS_PROXY= ALL_PROXY= NO_PROXY=127.0.0.1,localhost \
    LK_INTERNAL_BASE_URL="http://127.0.0.1:${MOCK_PORT}" \
    INTERNAL_AGENT_TOKEN="${INTERNAL_AGENT_TOKEN}" NODE_ID="${NODE_ID}" \
    SYNC_INTERVAL_SECONDS="${SYNC_INTERVAL_SECONDS}" METRICS_PUSH_INTERVAL="${METRICS_PUSH_INTERVAL}" \
    CREDENTIALS_PATH="${RUNTIME_DIR}/accounts.toml" TRUSTTUNNEL_RELOAD_SIGNAL="SIGHUP" \
    TRUSTTUNNEL_PID="${RELOAD_PID}" TRUSTTUNNEL_HEALTH_ADDR="${ENDPOINT_ADDR}" \
    "${SIDECAR_BIN}"
) >"${SIDECAR_LOG}" 2>&1 &
SIDECAR_PID=$!

wait_for_pattern() {
  local file="$1" pattern="$2" timeout_s="$3" started
  started="$(date +%s)"
  while true; do
    [[ -f "$file" ]] && rg -q "$pattern" "$file" && return 0
    (( $(date +%s) - started > timeout_s )) && { echo "error: timeout waiting '$pattern' in $file" >&2; return 1; }
    sleep 0.5
  done
}

wait_for_pattern "${RUNTIME_DIR}/accounts.toml" "user1" 30
wait_for_pattern "${TMP_DIR}/sync_reports.ndjson" '"status": "success"|"status":"success"' 30
sleep 25
python3 - <<'PY' "${TMP_DIR}/heartbeats.ndjson" "${METRICS_PUSH_INTERVAL}"
import datetime, json, sys
rows=[json.loads(x) for x in open(sys.argv[1], encoding='utf-8') if x.strip()]
if len(rows) < 3: raise SystemExit(f"expected >=3 heartbeats, got {len(rows)}")
ts=[datetime.datetime.fromisoformat(x['received_at']) for x in rows[-3:]]
exp=int(sys.argv[2]); deltas=[(ts[i+1]-ts[i]).total_seconds() for i in range(2)]
assert all(exp-1 <= d <= exp+3 for d in deltas), f"bad deltas: {deltas}"
print('heartbeat interval check passed', deltas)
PY

curl -fsS -X POST "http://127.0.0.1:${MOCK_PORT}/__admin/scenario" -H 'content-type: application/json' -d '{"mode":"delayed","version":"v1"}' >/dev/null
wait_for_pattern "${SIDECAR_LOG}" "retrying in 1s" 60
wait_for_pattern "${SIDECAR_LOG}" "retrying in 2s" 60
wait_for_pattern "${SIDECAR_LOG}" "retrying in 4s" 60

curl -fsS -X POST "http://127.0.0.1:${MOCK_PORT}/__admin/scenario" -H 'content-type: application/json' -d '{"mode":"drop","version":"v1"}' >/dev/null
wait_for_pattern "${SIDECAR_LOG}" "Connection reset|connection closed before message completed|error sending request" 60

curl -fsS -X POST "http://127.0.0.1:${MOCK_PORT}/__admin/scenario" -H 'content-type: application/json' -d '{"mode":"normal","version":"v2"}' >/dev/null
wait_for_pattern "${RUNTIME_DIR}/accounts.toml" "pass-v2" 60
wait_for_pattern "${TMP_DIR}/sync_reports.ndjson" '"version": "v2"|"version":"v2"' 60

echo "E2E smoke passed: sync-report, heartbeat, retry/backoff 1s/2s/4s and recovery are verified."
