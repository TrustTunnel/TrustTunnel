#!/usr/bin/env bash
set -euo pipefail

canonical_endpoints=(
  "/internal/vpn/classic/accounts"
  "/internal/vpn/classic/sync-report"
  "/internal/nodes/heartbeat"
)

code_file="agent/src/main.rs"
doc_files=(
  "docs/LK_INTEGRATION.md"
  "docs/DEPLOYMENT.md"
  "agent/README.md"
)

for endpoint in "${canonical_endpoints[@]}"; do
  if ! rg -F --quiet "$endpoint" "$code_file"; then
    echo "Missing endpoint '$endpoint' in $code_file"
    exit 1
  fi

  if ! rg -F --quiet "$endpoint" "${doc_files[@]}"; then
    echo "Missing endpoint '$endpoint' in docs set (${doc_files[*]})"
    exit 1
  fi
done

mapfile -t found_doc_endpoints < <(rg --no-filename -o '/internal/[a-z0-9/_-]+' "${doc_files[@]}" | sort -u)

for endpoint in "${found_doc_endpoints[@]}"; do
  is_canonical=false
  for canonical in "${canonical_endpoints[@]}"; do
    if [[ "$endpoint" == "$canonical" ]]; then
      is_canonical=true
      break
    fi
  done

  if [[ "$is_canonical" == "false" ]]; then
    echo "Non-canonical LK endpoint path found in docs: $endpoint"
    exit 1
  fi
done

echo "LK endpoints in code/docs match canonical set."
