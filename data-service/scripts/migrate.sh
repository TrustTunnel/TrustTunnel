#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${DATABASE_URL:-}" ]]; then
  echo "DATABASE_URL is required" >&2
  exit 1
fi

MIGRATIONS_DIR="${MIGRATIONS_DIR:-/app/migrations}"
MIGRATION_TARGET="${MIGRATION_TARGET:-}"

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 <<'SQL'
CREATE TABLE IF NOT EXISTS schema_migrations (
    version TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
SQL

mapfile -t migration_files < <(find "$MIGRATIONS_DIR" -maxdepth 1 -type f -name '*.sql' | sort)

for migration_file in "${migration_files[@]}"; do
  migration_version="$(basename "$migration_file")"

  if [[ -n "$MIGRATION_TARGET" && "$migration_version" > "$MIGRATION_TARGET" ]]; then
    continue
  fi

  already_applied="$(psql "$DATABASE_URL" -tA -v ON_ERROR_STOP=1 -c "SELECT 1 FROM schema_migrations WHERE version = '$migration_version' LIMIT 1")"

  if [[ "$already_applied" == "1" ]]; then
    echo "skip $migration_version"
    continue
  fi

  echo "apply $migration_version"
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -f "$migration_file"
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c "INSERT INTO schema_migrations(version) VALUES ('$migration_version')"
done
