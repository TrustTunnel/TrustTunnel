#!/usr/bin/env bash
set -euo pipefail

: "${DB_HOST:=127.0.0.1}"
: "${DB_PORT:=5432}"
: "${DB_USER:=postgres}"
: "${DB_PASSWORD:=postgres}"
: "${DB_NAME:=postgres}"

export PGPASSWORD="$DB_PASSWORD"
PSQL_BASE=(psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" -v ON_ERROR_STOP=1)

wait_for_db() {
  echo "waiting for postgres ${DB_HOST}:${DB_PORT}"
  for _ in $(seq 1 30); do
    if "${PSQL_BASE[@]}" -c 'SELECT 1' >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "postgres is not ready" >&2
  exit 1
}

recreate_db() {
  local db_name="$1"
  "${PSQL_BASE[@]}" -c "DROP DATABASE IF EXISTS ${db_name};"
  "${PSQL_BASE[@]}" -c "CREATE DATABASE ${db_name};"
}

run_migrator() {
  local db_name="$1"
  local target="${2:-}"
  local db_url="postgresql://${DB_USER}:${DB_PASSWORD}@${DB_HOST}:${DB_PORT}/${db_name}"

  if [[ -n "$target" ]]; then
    DATABASE_URL="$db_url" MIGRATIONS_DIR="$(pwd)/data-service/migrations" MIGRATION_TARGET="$target" ./data-service/scripts/migrate.sh
  else
    DATABASE_URL="$db_url" MIGRATIONS_DIR="$(pwd)/data-service/migrations" ./data-service/scripts/migrate.sh
  fi
}

wait_for_db

# Scenario 1: clean DB -> migrations -> repeated migrations
recreate_db "dbverify_clean"
run_migrator "dbverify_clean"
run_migrator "dbverify_clean"
run_migrator "dbverify_clean"

clean_migration_count="$(psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d dbverify_clean -tA -v ON_ERROR_STOP=1 -c 'SELECT COUNT(*) FROM schema_migrations;')"
[[ "$clean_migration_count" == "2" ]] || { echo "expected 2 migrations, got $clean_migration_count" >&2; exit 1; }

# Scenario 2: partially applied DB -> migrate to target state without data corruption
recreate_db "dbverify_partial"
run_migrator "dbverify_partial" "001_init_classic_state.sql"

psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d dbverify_partial -v ON_ERROR_STOP=1 <<'SQL'
INSERT INTO classic_state_accounts(lk_username, lk_password, legacy_username, legacy_password)
VALUES
  ('alice', 'pw1', NULL, NULL),
  ('bob', 'pw2', 'legacy-bob', NULL),
  ('carol', 'pw3', 'legacy-carol', 'legacy-pass-carol');
SQL

run_migrator "dbverify_partial"
run_migrator "dbverify_partial"

partial_migration_count="$(psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d dbverify_partial -tA -v ON_ERROR_STOP=1 -c 'SELECT COUNT(*) FROM schema_migrations;')"
[[ "$partial_migration_count" == "2" ]] || { echo "expected 2 migrations, got $partial_migration_count" >&2; exit 1; }

alice_row="$(psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d dbverify_partial -tA -F '|' -v ON_ERROR_STOP=1 -c "SELECT legacy_username, legacy_password FROM classic_state_accounts WHERE lk_username='alice';")"
bob_row="$(psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d dbverify_partial -tA -F '|' -v ON_ERROR_STOP=1 -c "SELECT legacy_username, legacy_password FROM classic_state_accounts WHERE lk_username='bob';")"
carol_row="$(psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d dbverify_partial -tA -F '|' -v ON_ERROR_STOP=1 -c "SELECT legacy_username, legacy_password FROM classic_state_accounts WHERE lk_username='carol';")"

[[ "$alice_row" == "alice|pw1" ]] || { echo "alice corrupted: $alice_row" >&2; exit 1; }
[[ "$bob_row" == "legacy-bob|pw2" ]] || { echo "bob corrupted: $bob_row" >&2; exit 1; }
[[ "$carol_row" == "legacy-carol|legacy-pass-carol" ]] || { echo "carol corrupted: $carol_row" >&2; exit 1; }

echo "dbverify passed"
