# Data service migrations

## Run migrator

```bash
DATABASE_URL=postgresql://postgres:postgres@127.0.0.1:5432/postgres \
MIGRATIONS_DIR=$(pwd)/data-service/migrations \
./data-service/scripts/migrate.sh
```

Optional `MIGRATION_TARGET` allows applying only part of the chain.

## Verify scenarios

```bash
DB_HOST=127.0.0.1 DB_PORT=5432 DB_USER=postgres DB_PASSWORD=postgres DB_NAME=postgres \
./data-service/scripts/dbverify.sh
```

The verification script checks:
- clean DB -> migrations -> repeated migrations (idempotency);
- partially applied state -> migration completion without corrupting prefilled legacy values.
