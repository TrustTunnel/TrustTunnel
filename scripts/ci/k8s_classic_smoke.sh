#!/usr/bin/env bash
set -euo pipefail

MANIFEST_PATH="${MANIFEST_PATH:-deploy/k8s/trusttunnel-classic.yaml}"
NAMESPACE="${NAMESPACE:-trusttunnel-smoke}"
WAIT_TIMEOUT="${WAIT_TIMEOUT:-180s}"
ENDPOINT_IMAGE="${ENDPOINT_IMAGE:-}"
SIDECAR_IMAGE="${SIDECAR_IMAGE:-}"

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "error: required command not found: $1" >&2
    exit 1
  }
}

require_cmd kubectl
require_cmd curl

if ! kubectl config current-context >/dev/null 2>&1; then
  echo "error: kubectl context is not configured" >&2
  exit 1
fi

context="$(kubectl config current-context)"
echo "Using kubectl context: ${context}"

if [[ -n "${ENDPOINT_IMAGE}" || -n "${SIDECAR_IMAGE}" ]]; then
  require_cmd python3
fi

if [[ -n "${ENDPOINT_IMAGE}" && "${context}" == kind-* ]]; then
  require_cmd kind
  kind load docker-image "${ENDPOINT_IMAGE}" --name "${context#kind-}"
fi

if [[ -n "${SIDECAR_IMAGE}" && "${context}" == kind-* ]]; then
  require_cmd kind
  kind load docker-image "${SIDECAR_IMAGE}" --name "${context#kind-}"
fi

if [[ -n "${ENDPOINT_IMAGE}" && "${context}" == minikube ]]; then
  require_cmd minikube
  minikube image load "${ENDPOINT_IMAGE}"
fi

if [[ -n "${SIDECAR_IMAGE}" && "${context}" == minikube ]]; then
  require_cmd minikube
  minikube image load "${SIDECAR_IMAGE}"
fi

cleanup() {
  kubectl delete namespace "${NAMESPACE}" --wait=false >/dev/null 2>&1 || true
}
trap cleanup EXIT

kubectl create namespace "${NAMESPACE}" >/dev/null 2>&1 || true

manifest_to_apply="${MANIFEST_PATH}"
if [[ -n "${ENDPOINT_IMAGE}" || -n "${SIDECAR_IMAGE}" ]]; then
  tmp_manifest="$(mktemp)"
  python3 - "$MANIFEST_PATH" "$tmp_manifest" "$ENDPOINT_IMAGE" "$SIDECAR_IMAGE" <<'PY'
import re
import sys

src, dst, endpoint, sidecar = sys.argv[1:5]
with open(src, "r", encoding="utf-8") as f:
    text = f.read()

if endpoint:
    text = re.sub(
        r"(name: trusttunnel_endpoint\n(?:[\t ]+#[^\n]*\n)*[\t ]+image: )[^\n]+",
        rf"\g<1>{endpoint}",
        text,
        flags=re.MULTILINE,
    )
if sidecar:
    text = re.sub(
        r"(name: trusttunnel_sidecar_agent\n(?:[\t ]+#[^\n]*\n)*[\t ]+image: )[^\n]+",
        rf"\g<1>{sidecar}",
        text,
        flags=re.MULTILINE,
    )

with open(dst, "w", encoding="utf-8") as f:
    f.write(text)
PY
  manifest_to_apply="${tmp_manifest}"
fi

kubectl -n "${NAMESPACE}" apply -f "${manifest_to_apply}"
kubectl -n "${NAMESPACE}" rollout status deploy/trusttunnel-classic --timeout="${WAIT_TIMEOUT}"
kubectl -n "${NAMESPACE}" wait --for=condition=ready pod -l app.kubernetes.io/name=trusttunnel --timeout="${WAIT_TIMEOUT}"

pod_name="$(kubectl -n "${NAMESPACE}" get pod -l app.kubernetes.io/name=trusttunnel -o jsonpath='{.items[0].metadata.name}')"

kubectl -n "${NAMESPACE}" port-forward "pod/${pod_name}" 19105:9105 18443:8443 >/tmp/trusttunnel-smoke-port-forward.log 2>&1 &
pf_pid=$!

stop_pf() {
  kill "${pf_pid}" >/dev/null 2>&1 || true
}
trap 'stop_pf; cleanup' EXIT

for _ in $(seq 1 20); do
  if curl -fsS "http://127.0.0.1:19105/healthz" >/dev/null; then
    break
  fi
  sleep 1
done

curl -fsS "http://127.0.0.1:19105/healthz" >/dev/null
curl -fsS "http://127.0.0.1:19105/metrics" >/dev/null

timeout 3 bash -c 'cat < /dev/null > /dev/tcp/127.0.0.1/18443'
timeout 3 bash -c 'cat < /dev/null > /dev/tcp/127.0.0.1/19105'

echo "Smoke checks passed."
