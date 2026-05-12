#!/usr/bin/env bash
# Folio backup driver. See docs/install/backup.md for the full rationale.
#
# Usage:
#   scripts/backup.sh nightly   # postgres dump only
#   scripts/backup.sh weekly    # postgres dump + comic_data tar
#   scripts/backup.sh full      # both, regardless of day
#
# Environment overrides (all optional):
#   FOLIO_HOME       — where compose + .env live      (default: /opt/folio)
#   BACKUP_DIR       — where to write artifacts       (default: /var/backups/folio)
#   COMPOSE_FILE     — compose file path              (default: $FOLIO_HOME/compose.prod.yml)
#   POSTGRES_RETAIN  — days of postgres dumps to keep (default: 14)
#   DATA_RETAIN      — days of comic_data tarballs to keep (default: 56)

set -euo pipefail

MODE="${1:-nightly}"
FOLIO_HOME="${FOLIO_HOME:-/opt/folio}"
BACKUP_DIR="${BACKUP_DIR:-/var/backups/folio}"
COMPOSE_FILE="${COMPOSE_FILE:-$FOLIO_HOME/compose.prod.yml}"
POSTGRES_RETAIN="${POSTGRES_RETAIN:-14}"
DATA_RETAIN="${DATA_RETAIN:-56}"

DATE="$(date +%F)"
TS="$(date +%FT%H%M%S)"

log() { printf '[%s] %s\n' "$(date +%FT%T)" "$*"; }
die() { log "ERROR: $*" >&2; exit 1; }

[[ -f "$COMPOSE_FILE" ]] || die "compose file not found: $COMPOSE_FILE"
command -v docker >/dev/null || die "docker not in PATH"

mkdir -p "$BACKUP_DIR"

# ───────────── Postgres dump ─────────────
postgres_dump() {
    local out="$BACKUP_DIR/postgres-$DATE.dump"
    log "postgres: pg_dump → $out"
    docker compose -f "$COMPOSE_FILE" exec -T postgres \
        pg_dump -U comic -Fc comic_reader > "$out.tmp"
    mv "$out.tmp" "$out"
    log "postgres: $(stat -c%s "$out" 2>/dev/null || stat -f%z "$out") bytes"

    # Prune older dumps.
    find "$BACKUP_DIR" -maxdepth 1 -name 'postgres-*.dump' -mtime "+$POSTGRES_RETAIN" -delete
}

# ───────────── comic_data volume tar ─────────────
data_tar() {
    local out="$BACKUP_DIR/data-$DATE.tgz"
    log "data:     tar → $out"
    # `:ro` on the volume avoids any in-flight writes corrupting the tar.
    # alpine + tar is ~5MB, fast to pull, no host deps.
    docker run --rm \
        -v folio_comic_data:/d:ro \
        -v "$BACKUP_DIR:/b" \
        alpine \
        tar czf "/b/data-$DATE.tgz.tmp" -C /d .
    mv "$out.tmp" "$out"
    log "data:     $(stat -c%s "$out" 2>/dev/null || stat -f%z "$out") bytes"

    # Prune older tarballs.
    find "$BACKUP_DIR" -maxdepth 1 -name 'data-*.tgz' -mtime "+$DATA_RETAIN" -delete
}

case "$MODE" in
    nightly) postgres_dump ;;
    weekly)  postgres_dump; data_tar ;;
    full)    postgres_dump; data_tar ;;
    *)       die "unknown mode: $MODE (use: nightly | weekly | full)" ;;
esac

log "done ($MODE)"
