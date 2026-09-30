#!/usr/bin/env bash
# Migration down-path gate (WP-3.8, audit AR-9).
#
# Every migration ships a `down`, but nothing exercised them, so a broken
# rollback was only discovered mid-incident. Against a FRESH Postgres this:
#
#   1. applies every migration (`up`) and snapshots the schema   → full
#   2. rolls back the newest $ROUNDTRIP_COUNT (`down -n N`)       → rolled
#   3. migrates a second, empty DB to exactly `total - N`         → baseline
#      and asserts rolled == baseline (the downs really undo their ups —
#      a `down` that "succeeds" but leaves a column behind fails here)
#   4. re-applies (`up`) and asserts the schema == full (the ups are
#      re-runnable after their downs)
#
# Schema snapshots are `pg_dump --schema-only`, run inside the Postgres
# container so the client always matches the server major version.
#
# Env:
#   PG_CONTAINER     docker container running Postgres (required)
#   PG_URL_BASE      host-side URL without db name, e.g.
#                    postgres://comic:comic@localhost:5432 (required)
#   PG_USER          role for pg_dump/psql inside the container (default comic)
#   MIGRATION_BIN    path to the built `migration` CLI (default target/debug/migration)
#   ROUNDTRIP_COUNT  how many trailing migrations to roll back (default 10)
set -euo pipefail

: "${PG_CONTAINER:?PG_CONTAINER is required}"
: "${PG_URL_BASE:?PG_URL_BASE is required}"
PG_USER="${PG_USER:-comic}"
MIGRATION_BIN="${MIGRATION_BIN:-target/debug/migration}"
N="${ROUNDTRIP_COUNT:-10}"

DB=folio_mig_roundtrip
BASE_DB=folio_mig_baseline
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

psql_c() {
  docker exec "$PG_CONTAINER" psql -v ON_ERROR_STOP=1 -U "$PG_USER" -d postgres -tAc "$1"
}

# Normalize a dump so two dumps of the same schema compare equal:
#   - pg_dump 17.6+/18 emits a random per-dump `\restrict` token → drop it.
#   - a `down` that re-adds a dropped column can only append it (Postgres
#     has no ADD COLUMN ... AFTER), so column *order* inside CREATE TABLE
#     legitimately differs → sort the column lines of each table block.
normalize_dump() {
  python3 -c '
import re, sys
out, block = [], None
for line in sys.stdin:
    line = line.rstrip("\n")
    if re.match(r"^\\(un)?restrict ", line):
        continue
    if block is None:
        out.append(line)
        if line.startswith("CREATE TABLE ") and line.endswith("("):
            block = []
    elif line == ");":
        out.extend(sorted(l.rstrip(",") for l in block))
        out.append(line)
        block = None
    else:
        block.append(line)
print("\n".join(out))
'
}

dump_schema() {
  docker exec "$PG_CONTAINER" pg_dump -U "$PG_USER" -d "$1" \
    --schema-only --no-owner --no-privileges --exclude-table=seaql_migrations \
    | normalize_dump >"$2"
}

migrate() {
  local db="$1"
  shift
  DATABASE_URL="$PG_URL_BASE/$db" "$MIGRATION_BIN" "$@"
}

for db in "$DB" "$BASE_DB"; do
  psql_c "DROP DATABASE IF EXISTS $db"
  psql_c "CREATE DATABASE $db"
done

echo "==> up (all)"
migrate "$DB" up
dump_schema "$DB" "$WORK/full.sql"
total="$(docker exec "$PG_CONTAINER" psql -U "$PG_USER" -d "$DB" -tAc 'SELECT count(*) FROM seaql_migrations')"
echo "    $total migrations applied"
if [ "$total" -le "$N" ]; then
  echo "::error::only $total migrations; cannot roll back $N" >&2
  exit 1
fi

echo "==> down -n $N"
migrate "$DB" down -n "$N"
dump_schema "$DB" "$WORK/rolled.sql"

echo "==> baseline: up -n $((total - N)) on a second fresh DB"
migrate "$BASE_DB" up -n "$((total - N))"
dump_schema "$BASE_DB" "$WORK/baseline.sql"

status=0
if ! diff -u "$WORK/baseline.sql" "$WORK/rolled.sql"; then
  echo "::error::schema after 'down -n $N' differs from a DB migrated to $((total - N)) (a down does not fully revert its up; diff above)" >&2
  status=1
fi

echo "==> up (re-apply the rolled-back $N)"
migrate "$DB" up
dump_schema "$DB" "$WORK/reup.sql"
if ! diff -u "$WORK/full.sql" "$WORK/reup.sql"; then
  echo "::error::schema after down+up differs from the original up (diff above)" >&2
  status=1
fi

for db in "$DB" "$BASE_DB"; do
  psql_c "DROP DATABASE IF EXISTS $db" || true
done

if [ "$status" -eq 0 ]; then
  echo "==> OK: last $N migrations round-trip cleanly ($total total)"
fi
exit "$status"
