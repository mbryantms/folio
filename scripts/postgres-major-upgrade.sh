#!/usr/bin/env bash
# Move a Folio compose stack's Postgres volume to a new major version.
# See docs/install/upgrades.md ("Postgres major upgrade") for the walkthrough.
#
# Usage:
#   scripts/postgres-major-upgrade.sh <compose-file>              # upgrade
#   scripts/postgres-major-upgrade.sh <compose-file> --rollback   # undo
#
# Before running, change the postgres `image:` line in <compose-file> to the
# new major (for example postgres:19-alpine@sha256:...). Don't run
# `docker compose up` in between; if you did, the new image is refusing to
# start (it won't touch the old cluster), and this script handles that too.
#
# What it does:
#   1. Finds the new major from the compose file's image and the old one
#      from the volume (the <major>/docker directory with a PG_VERSION).
#   2. Stops the services that write to the database (WRITERS).
#   3. Runs the old major (OLD_IMAGE, default postgres:<old>-alpine), records
#      per-table row counts and takes a pg_dump -Fc into BACKUP_DIR.
#   4. Renames <old>/docker to <old>/docker-pre-<new> inside the same volume
#      (an instant rename, nothing copied or deleted), so the new image starts
#      with an empty cluster of its own.
#   5. Starts the new major, restores the dump, runs ANALYZE, and compares
#      per-table row counts with the ones from step 3.
#   6. Starts the writers again.
#
# A dump and restore rather than pg_upgrade on purpose: the restore rebuilds
# every index and recomputes the generated tsvector columns, which Postgres
# 19's release notes ask for after an upgrade (full-text search, pg_trgm and
# fuzzystrmatch changed behaviour).
#
# --rollback deletes the new major's cluster and renames the old one back.
# Anything written after the upgrade is lost. Afterwards, put the old image
# back in the compose file and run `docker compose up -d`.
#
# Environment overrides (all optional):
#   BACKUP_DIR  where the dump and row counts go
#               (default /var/backups/folio, ./backups for compose.dev.yml)
#   WRITERS     services stopped during the move (default: "app web",
#               only those defined in the compose file)
#   OLD_IMAGE   image used to run the old major for the dump
#               (default postgres:<old major>-alpine)
#   ASSUME_YES  set to 1 to skip the confirmation prompt

set -euo pipefail
umask 077

COMPOSE_FILE="${1:-}"
MODE="${2:-upgrade}"
[[ -n "$COMPOSE_FILE" && -f "$COMPOSE_FILE" ]] || {
    echo "usage: $0 <compose-file> [--rollback]" >&2; exit 2; }
case "$MODE" in upgrade|--rollback) ;; *) echo "unknown mode: $MODE" >&2; exit 2 ;; esac

DB_USER=comic
DB_NAME=comic_reader
PG_ROOT=/var/lib/postgresql
TS="$(date -u +%Y%m%dT%H%M%SZ)"
if [[ "$(basename "$COMPOSE_FILE")" == compose.dev.yml ]]; then
    BACKUP_DIR="${BACKUP_DIR:-./backups}"
else
    BACKUP_DIR="${BACKUP_DIR:-/var/backups/folio}"
fi

log() { printf '==> %s\n' "$*"; }
die() { printf '==> ERROR: %s\n' "$*" >&2; exit 1; }

dc() { docker compose -f "$COMPOSE_FILE" "$@"; }
# A throwaway container on the postgres service's volume, as root, for the
# rename / delete. DAC_OVERRIDE + FOWNER: the cluster directories belong to
# the postgres user and compose.prod.yml drops every capability.
on_volume() {
    dc run --rm --no-deps -T --user root --cap-add DAC_OVERRIDE --cap-add FOWNER \
        --entrypoint sh postgres -c "$1"
}
psql_q() { dc exec -T postgres psql -X -v ON_ERROR_STOP=1 -U "$DB_USER" -d "$DB_NAME" -tAc "$1"; }

wait_ready() {
    # TCP, not the socket: during first-time init the entrypoint runs a
    # temporary server on the socket only, which would look ready too early.
    for _ in $(seq 1 90); do
        if dc exec -T postgres pg_isready -q -h 127.0.0.1 -U "$DB_USER" -d "$DB_NAME" 2>/dev/null; then
            return 0
        fi
        sleep 2
    done
    dc logs --tail 40 postgres >&2 || true
    die "postgres did not become ready"
}

server_major() { psql_q "SELECT current_setting('server_version_num')::int / 10000"; }

ROW_COUNTS_SQL="SELECT table_name || ' ' || (xpath('/row/c/text()', query_to_xml(format('SELECT count(*) AS c FROM %I.%I', table_schema, table_name), false, true, '')))[1]::text FROM information_schema.tables WHERE table_schema = 'public' AND table_type = 'BASE TABLE' ORDER BY table_name"

confirm() {
    [[ "${ASSUME_YES:-}" == 1 ]] && return 0
    read -r -p "    Type '$1' to continue: " answer
    [[ "$answer" == "$1" ]] || die "aborted"
}

# ── what is where ──────────────────────────────────────────────────────────
command -v docker >/dev/null || die "docker not in PATH"
dc config -q || die "docker compose cannot read $COMPOSE_FILE"
NEW_IMAGE="$(dc config postgres | sed -n 's/^    image: //p' | head -n1)"
[[ -n "$NEW_IMAGE" ]] || die "no image for the postgres service in $COMPOSE_FILE"
log "pulling $NEW_IMAGE"
docker pull -q "$NEW_IMAGE" >/dev/null
NEW_MAJOR="$(docker image inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$NEW_IMAGE" | sed -n 's/^PG_MAJOR=//p')"
[[ "$NEW_MAJOR" =~ ^[0-9]+$ ]] || die "cannot read PG_MAJOR from $NEW_IMAGE"

# Every cluster on the volume: "<dir> <PG_VERSION>" per line.
CLUSTERS="$(on_volume "for d in $PG_ROOT/*/docker $PG_ROOT/*/docker-pre-*; do [ -s \"\$d/PG_VERSION\" ] && echo \"\$d \$(cat \"\$d/PG_VERSION\")\"; done; true")"
WRITERS_DEFINED=()
for s in ${WRITERS-app web}; do
    dc config --services | grep -qx "$s" && WRITERS_DEFINED+=("$s")
done

if [[ "$MODE" == --rollback ]]; then
    ASIDE="$(awk '$1 ~ /docker-pre-/ {print $1}' <<<"$CLUSTERS")"
    [[ $(wc -w <<<"$ASIDE") -eq 1 ]] || die "expected exactly one set-aside cluster, found: ${ASIDE:-none}"
    OLD_DIR="${ASIDE%/docker-pre-*}/docker"
    NEW_DIR="$(awk '$1 !~ /docker-pre-/ {print $1}' <<<"$CLUSTERS")"
    log "rollback: delete ${NEW_DIR:-<no new cluster>} and rename $ASIDE back to $OLD_DIR"
    log "anything written since the upgrade is lost"
    confirm rollback
    dc stop "${WRITERS_DEFINED[@]}" postgres
    on_volume "set -e; ${NEW_DIR:+rm -rf \"${NEW_DIR%/docker}\";} mv \"$ASIDE\" \"$OLD_DIR\""
    log "done. Put the old postgres image back in $COMPOSE_FILE, then:"
    log "  docker compose -f $COMPOSE_FILE up -d"
    exit 0
fi

OLD_LINES="$(awk -v n="$NEW_MAJOR" '$1 !~ /docker-pre-/ && $2 != n' <<<"$CLUSTERS")"
if [[ -z "$OLD_LINES" ]]; then
    awk -v n="$NEW_MAJOR" '$1 !~ /docker-pre-/ && $2 == n {found=1} END {exit !found}' <<<"$CLUSTERS" \
        && { log "the volume already holds a Postgres $NEW_MAJOR cluster; nothing to do"; exit 0; }
    die "no cluster found on the postgres volume (the pre-18 /var/lib/postgresql/data layout is not handled here)"
fi
[[ $(wc -l <<<"$OLD_LINES") -eq 1 ]] || die "more than one old cluster on the volume: $OLD_LINES"
read -r OLD_DIR OLD_MAJOR <<<"$OLD_LINES"
(( NEW_MAJOR > OLD_MAJOR )) || die "$NEW_IMAGE is Postgres $NEW_MAJOR, not newer than the volume's $OLD_MAJOR"
grep -q "docker-pre-$NEW_MAJOR" <<<"$CLUSTERS" && die "$OLD_DIR-pre-$NEW_MAJOR already exists; an earlier run was not finished or rolled back"
OLD_IMAGE="${OLD_IMAGE:-postgres:$OLD_MAJOR-alpine}"

mkdir -p "$BACKUP_DIR"
DUMP="$BACKUP_DIR/postgres-pg$OLD_MAJOR-before-pg$NEW_MAJOR-$TS.dump"
COUNTS_BEFORE="$BACKUP_DIR/row-counts-pg$OLD_MAJOR-$TS.txt"
COUNTS_AFTER="$BACKUP_DIR/row-counts-pg$NEW_MAJOR-$TS.txt"

log "Postgres $OLD_MAJOR ($OLD_DIR) → $NEW_MAJOR ($NEW_IMAGE)"
log "stops: ${WRITERS_DEFINED[*]:-nothing} + postgres; dump: $DUMP"
[[ ${#WRITERS_DEFINED[@]} -eq 0 ]] && log "no app services in this file: stop anything else using the database (just dev) first"
confirm upgrade

# ── 1. dump under the old major ────────────────────────────────────────────
[[ ${#WRITERS_DEFINED[@]} -gt 0 ]] && log "stopping ${WRITERS_DEFINED[*]}"
[[ ${#WRITERS_DEFINED[@]} -gt 0 ]] && dc stop "${WRITERS_DEFINED[@]}"
OVERRIDE="$(mktemp "${TMPDIR:-/tmp}/pg-major-upgrade.XXXXXX.yml")"
trap 'rm -f "$OVERRIDE"' EXIT
printf 'services:\n  postgres:\n    image: %s\n' "$OLD_IMAGE" >"$OVERRIDE"
log "running Postgres $OLD_MAJOR ($OLD_IMAGE) for the dump"
docker compose -f "$COMPOSE_FILE" -f "$OVERRIDE" up -d --no-deps postgres
wait_ready
[[ "$(server_major)" == "$OLD_MAJOR" ]] || die "expected Postgres $OLD_MAJOR to be running"

psql_q "$ROW_COUNTS_SQL" >"$COUNTS_BEFORE"
log "$(wc -l <"$COUNTS_BEFORE") tables counted → $COUNTS_BEFORE"
log "pg_dump → $DUMP"
dc exec -T postgres pg_dump -U "$DB_USER" -Fc "$DB_NAME" >"$DUMP.tmp"
mv "$DUMP.tmp" "$DUMP"
ls -lh "$DUMP"

# ── 2. set the old cluster aside ───────────────────────────────────────────
dc stop postgres
log "renaming $OLD_DIR → $OLD_DIR-pre-$NEW_MAJOR"
on_volume "mv \"$OLD_DIR\" \"$OLD_DIR-pre-$NEW_MAJOR\""
trap 'rm -f "$OVERRIDE"; echo "==> FAILED after the old cluster was set aside. To undo: $0 $COMPOSE_FILE --rollback" >&2' EXIT

# ── 3. new major: restore, analyze, compare ────────────────────────────────
log "starting Postgres $NEW_MAJOR"
dc up -d --no-deps --force-recreate postgres
wait_ready
[[ "$(server_major)" == "$NEW_MAJOR" ]] || die "expected Postgres $NEW_MAJOR to be running"
log "pg_restore"
dc exec -T postgres pg_restore --exit-on-error --no-owner -U "$DB_USER" -d "$DB_NAME" <"$DUMP"
log "vacuumdb --analyze-in-stages"
dc exec -T postgres vacuumdb -q -U "$DB_USER" -d "$DB_NAME" --analyze-in-stages
psql_q "$ROW_COUNTS_SQL" >"$COUNTS_AFTER"
if ! diff -u "$COUNTS_BEFORE" "$COUNTS_AFTER"; then
    die "row counts differ (above). The writers are still stopped. To undo: $0 $COMPOSE_FILE --rollback"
fi
log "row counts match ($(wc -l <"$COUNTS_AFTER") tables)"
trap 'rm -f "$OVERRIDE"' EXIT

# ── 4. writers back ────────────────────────────────────────────────────────
if [[ ${#WRITERS_DEFINED[@]} -gt 0 ]]; then
    log "starting ${WRITERS_DEFINED[*]}"
    dc up -d "${WRITERS_DEFINED[@]}"
fi
log "done: Postgres $NEW_MAJOR is serving $DB_NAME"
log "the Postgres $OLD_MAJOR cluster stays at $OLD_DIR-pre-$NEW_MAJOR (rollback: $0 $COMPOSE_FILE --rollback)"
log "once you are happy, free its space with:"
log "  docker compose -f $COMPOSE_FILE run --rm --no-deps --user root --cap-add DAC_OVERRIDE --entrypoint rm postgres -rf ${OLD_DIR%/docker}"
