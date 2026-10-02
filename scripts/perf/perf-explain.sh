#!/usr/bin/env bash
# `just perf-explain` — record EXPLAIN (ANALYZE, BUFFERS) plans for the
# hottest list / filter / sort endpoints against a stress-scale library.
#
# What it does (docs/dev/load-testing.md has the full write-up):
#   1. Generates the stress fixture (`fixtures/build.py --scale stress
#      --series $PERF_SERIES --rich`) unless it already exists.
#   2. Starts a throwaway Postgres 18 + Redis on free ports (never touches
#      the dev services on 5432 / 6380). Postgres preloads `auto_explain`.
#   3. Boots the server binary against them, registers the first (admin)
#      user, creates a library over the fixture and scans it (thumbnails
#      off — the planner, not the thumbnailer, is the subject).
#   4. Seeds per-user activity (progress, a CBL, a collection, a saved
#      view) with scripts/perf/seed_activity.sql, then VACUUM ANALYZE.
#   5. Turns auto_explain on (log_min_duration=0, ANALYZE + BUFFERS) and
#      drives every endpoint in scripts/perf/endpoints.txt over HTTP, so
#      the plans are for the exact SQL the handlers emit — not a
#      hand-maintained copy that can drift.
#   6. Phase 2 (M7, WP-8.3; PERF_M7=0 skips it): seeds arcs, universes,
#      series groups, AlternateSeries and curated links
#      (scripts/perf/seed_relationships.sql), times full
#      `relationship_suggest` job runs over the library, bulk-accepts the
#      high-confidence suggestions, and drives scripts/perf/endpoints-m7.txt
#      (similar cold + warm, relationships, same universe, arc tie-ins, the
#      admin suggestion queue) the same way.
#   7. scripts/perf/explain_report.py splits the Postgres log per
#      endpoint and flags any `Seq Scan` on a table with more than
#      PERF_SEQSCAN_MIN_ROWS rows. Exit status 1 when one is found
#      (`job-*` labels — whole-library batch jobs — are reported, not gated).
#
# Knobs (env):
#   PERF_SERIES            series count (default 2500 → 50,000 issues)
#   PERF_ISSUES_PER_SERIES issues per series (default 20)
#   PERF_KEEP=1            keep the containers after the run and print how
#                          to reuse them (PERF_PG_CONTAINER=… PERF_REDIS_CONTAINER=…)
#   PERF_PG_CONTAINER / PERF_REDIS_CONTAINER
#                          reuse containers a previous PERF_KEEP=1 run left
#                          behind (skips the scan when already seeded)
#   PERF_SEQSCAN_MIN_ROWS  seq-scan gate threshold (default 5000; smaller
#                          tables ≥ 1000 rows are reported as info only)
#   PERF_OHA=1             after the plans, run an `oha` HTTP load pass over
#                          the same endpoints (PERF_OHA_DURATION=15s,
#                          PERF_OHA_CONCURRENCY=16, PERF_OHA_IMAGE when no
#                          local `oha` binary) → oha-<label>.txt
#   PERF_M7=0              skip phase 2 (the M7 relationship / similarity set)
#   PERF_SERVER_BIN        run this server binary instead of building the
#                          working tree (e.g. an origin/main build, for a
#                          before/after comparison against the same DB)
#   CARGO_TARGET_DIR       respected for the server build
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO"

SERIES="${PERF_SERIES:-2500}"
IPS="${PERF_ISSUES_PER_SERIES:-20}"
EXPECTED=$((SERIES * IPS))
FIXTURE="$REPO/fixtures/library-stress-${SERIES}x${IPS}"
TS=$(date -u +%Y%m%dT%H%M%SZ)
OUT="$REPO/perf-out/explain-$TS"
mkdir -p "$OUT"
EMAIL="perf@example.com"
PASSWORD="perf-explain-correct-horse-battery"
PG_IMAGE="${PERF_PG_IMAGE:-postgres:18-alpine}"
REDIS_IMAGE="${PERF_REDIS_IMAGE:-redis:8-alpine}"

log() { printf '==> %s\n' "$*" >&2; }

free_port() {
    python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'
}

# ── containers ──────────────────────────────────────────────────────────
STARTED_PG=""
STARTED_REDIS=""
SERVER_PID=""
cleanup() {
    local rc=$?
    if [ -n "$SERVER_PID" ]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    if [ "${PERF_KEEP:-0}" = "1" ]; then
        [ -n "$STARTED_PG$STARTED_REDIS" ] && log "kept containers — reuse with: PERF_PG_CONTAINER=$PG_CONTAINER PERF_REDIS_CONTAINER=$REDIS_CONTAINER just perf-explain"
    else
        # Only ever remove containers this run started, by exact name.
        [ -n "$STARTED_PG" ] && docker rm -f "$STARTED_PG" >/dev/null 2>&1 || true
        [ -n "$STARTED_REDIS" ] && docker rm -f "$STARTED_REDIS" >/dev/null 2>&1 || true
    fi
    exit $rc
}
trap cleanup EXIT INT TERM

container_port() { docker port "$1" "$2" | head -1 | sed 's/.*://'; }

if [ -n "${PERF_PG_CONTAINER:-}" ]; then
    PG_CONTAINER="$PERF_PG_CONTAINER"
    log "reusing Postgres container $PG_CONTAINER"
else
    PG_CONTAINER="folio-perf-pg-$TS-$$"
    PG_PORT=$(free_port)
    log "starting $PG_IMAGE as $PG_CONTAINER on 127.0.0.1:$PG_PORT"
    docker run -d --name "$PG_CONTAINER" \
        -e POSTGRES_USER=comic -e POSTGRES_PASSWORD=comic -e POSTGRES_DB=comic \
        -p "127.0.0.1:$PG_PORT:5432" \
        "$PG_IMAGE" \
        -c shared_preload_libraries=auto_explain \
        -c shared_buffers=512MB -c work_mem=16MB \
        -c max_connections=200 >/dev/null
    STARTED_PG="$PG_CONTAINER"
fi
if [ -n "${PERF_REDIS_CONTAINER:-}" ]; then
    REDIS_CONTAINER="$PERF_REDIS_CONTAINER"
else
    REDIS_CONTAINER="folio-perf-redis-$TS-$$"
    REDIS_PORT=$(free_port)
    log "starting $REDIS_IMAGE as $REDIS_CONTAINER on 127.0.0.1:$REDIS_PORT"
    docker run -d --name "$REDIS_CONTAINER" -p "127.0.0.1:$REDIS_PORT:6379" "$REDIS_IMAGE" >/dev/null
    STARTED_REDIS="$REDIS_CONTAINER"
fi
PG_PORT=$(container_port "$PG_CONTAINER" 5432)
REDIS_PORT=$(container_port "$REDIS_CONTAINER" 6379)
DB_URL="postgres://comic:comic@127.0.0.1:$PG_PORT/comic"
psqlq() { psql "$DB_URL" -X -q -v ON_ERROR_STOP=1 "$@"; }

for _ in $(seq 1 60); do
    psql "$DB_URL" -X -qtAc 'select 1' >/dev/null 2>&1 && break
    sleep 1
done

# ── fixture ─────────────────────────────────────────────────────────────
have=$(find "$FIXTURE" -name '*.cbz' 2>/dev/null | wc -l || true)
if [ "$have" -ne "$EXPECTED" ]; then
    log "generating stress fixture ($SERIES series × $IPS issues, --rich) → $FIXTURE"
    python3 fixtures/build.py --scale stress --series "$SERIES" \
        --issues-per-series "$IPS" --rich --out "$FIXTURE" >/dev/null
fi

# ── server ──────────────────────────────────────────────────────────────
if [ -n "${PERF_SERVER_BIN:-}" ]; then
    BIN="$PERF_SERVER_BIN"
    log "using server binary $BIN"
else
    log "building server"
    cargo build -q -p server --bin server
    BIN="${CARGO_TARGET_DIR:-$REPO/target}/debug/server"
fi
APP_PORT=$(free_port)
API="http://127.0.0.1:$APP_PORT"
# Secrets (pepper, JWT key) must survive container reuse → keyed by container.
DATA="$REPO/perf-out/data-$PG_CONTAINER"
mkdir -p "$DATA"
start_server() {
    # cwd = $OUT so the debug binary's dotenvy never picks up a repo .env.
    # `exec` makes $SERVER_PID the server itself: killing only the wrapping
    # subshell used to orphan the binary, which kept consuming jobs from a
    # reused Redis on the next PERF_KEEP run.
    (
        cd "$OUT"
        exec env -i PATH="$PATH" HOME="$HOME" \
            COMIC_DATABASE_URL="$DB_URL" \
            COMIC_REDIS_URL="redis://127.0.0.1:$REDIS_PORT" \
            COMIC_LIBRARY_PATH="$REPO/fixtures" \
            COMIC_DATA_PATH="$DATA" \
            COMIC_PUBLIC_URL="$API" \
            COMIC_BIND_ADDR="127.0.0.1:$APP_PORT" \
            COMIC_AUTH_MODE=local \
            COMIC_LOCAL_REGISTRATION_OPEN=true \
            COMIC_RATE_LIMIT_ENABLED=false \
            COMIC_WEB_UPSTREAM_URL="http://127.0.0.1:9" \
            COMIC_LOG_LEVEL="warn,server::relationships::suggestions=debug" \
            RUST_LOG=warn \
            "$BIN" >"$OUT/server.log" 2>&1
    ) &
    SERVER_PID=$!
    for _ in $(seq 1 180); do
        curl -sf "$API/healthz" >/dev/null 2>&1 && return 0
        kill -0 "$SERVER_PID" 2>/dev/null || { tail -40 "$OUT/server.log" >&2; return 1; }
        sleep 1
    done
    log "server did not come up"; tail -40 "$OUT/server.log" >&2; return 1
}
log "starting server on $API"
start_server

COOKIES="$OUT/cookies.txt"
BODY=$(printf '{"email":"%s","password":"%s"}' "$EMAIL" "$PASSWORD")
status=$(curl -sS -o /dev/null -w '%{http_code}' -c "$COOKIES" -X POST "$API/auth/local/login" \
    -H 'Content-Type: application/json' -d "$BODY")
if [ "$status" != "200" ]; then
    status=$(curl -sS -o /dev/null -w '%{http_code}' -c "$COOKIES" -X POST "$API/auth/local/register" \
        -H 'Content-Type: application/json' -d "$BODY")
    [ "$status" = "201" ] || [ "$status" = "200" ] || { log "register failed ($status)"; exit 1; }
    curl -sS -o /dev/null -c "$COOKIES" -X POST "$API/auth/local/login" \
        -H 'Content-Type: application/json' -d "$BODY"
fi
CSRF=$(awk '/comic_csrf/ {print $7}' "$COOKIES" | tail -1)
# Four more readers so per-user tables (progress, ratings, …) aren't 100 %
# the caller's rows — otherwise every per-user query "correctly" seq-scans.
for k in 1 2 3 4; do
    curl -sS -o /dev/null -X POST "$API/auth/local/register" -H 'Content-Type: application/json' \
        -d "$(printf '{"email":"perf-other-%s@example.com","password":"%s"}' "$k" "$PASSWORD")" || true
done
api() { curl -sSf -b "$COOKIES" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' "$@"; }

# ── scan ────────────────────────────────────────────────────────────────
issues=$(psqlq -tAc "select count(*) from issues where removed_at is null")
SCANNED=0
if [ "$issues" -lt "$EXPECTED" ]; then
    SCANNED=1
    LIB_SLUG=$(psqlq -tAc "select slug from libraries where root_path = '$FIXTURE'")
    if [ -z "$LIB_SLUG" ]; then
        LIB_SLUG=$(api -X POST "$API/api/libraries" \
            -d "$(printf '{"name":"Stress","root_path":"%s"}' "$FIXTURE")" | jq -r .slug)
    fi
    psqlq -c "update libraries set thumbnails_enabled = false where slug = '$LIB_SLUG'"
    log "scanning $EXPECTED issues (library $LIB_SLUG) — ~100 files/s"
    SCAN_START=$(date +%s)
    api -X POST "$API/api/libraries/$LIB_SLUG/scan" >/dev/null
    for _ in $(seq 1 7200); do
        sleep 5
        running=$(psqlq -tAc "select count(*) from scan_runs where state in ('queued','running')")
        n=$(psqlq -tAc "select count(*) from issues where removed_at is null")
        printf '\r    %6d / %d issues' "$n" "$EXPECTED" >&2
        if [ "$running" = "0" ] && [ "$n" -ge "$EXPECTED" ]; then break; fi
        if [ "$running" = "0" ] && [ $(( $(date +%s) - SCAN_START )) -gt 60 ]; then
            state=$(psqlq -tAc "select state || ' ' || coalesce(error,'') from scan_runs order by started_at desc limit 1")
            log "scan ended early: $state ($n issues)"; break
        fi
    done
    printf '\n' >&2
    log "scan finished in $(( $(date +%s) - SCAN_START ))s"
fi

# ── activity seed ───────────────────────────────────────────────────────
USER_ID=$(psqlq -tAc "select id from users where email = '$EMAIL'")
log "seeding per-user activity for $USER_ID"
psqlq -v user_id="$USER_ID" -f scripts/perf/seed_activity.sql >/dev/null
psqlq -c "vacuum analyze" >/dev/null

# ── explain run ─────────────────────────────────────────────────────────
psqlq -tAc "select relname, reltuples::bigint from pg_class c join pg_namespace n on n.oid=c.relnamespace
            where n.nspname='public' and c.relkind='r' order by 2 desc" >"$OUT/reltuples.txt"

# Resolve the ids the endpoint templates reference.
SERIES_SLUG=$(psqlq -tAc "select slug from series order by normalized_name offset (select count(*) / 2 from series) limit 1")
ISSUE_ID=$(psqlq -tAc "select i.id from issues i join series s on s.id=i.series_id where s.slug='$SERIES_SLUG' order by sort_number limit 1 offset 3")
VIEW_ID=$(psqlq -tAc "select id from saved_views where user_id='$USER_ID' and kind='filter_series' and name='perf: publisher+year' limit 1")
CBL_ID=$(psqlq -tAc "select id from cbl_lists where parsed_name='perf: event reading order' limit 1")
COLLECTION_ID=$(psqlq -tAc "select id from saved_views where user_id='$USER_ID' and kind='collection' and name='perf: collection' limit 1")
LIB_ID=$(psqlq -tAc "select id from libraries where root_path = '$FIXTURE'")

# Only this run's log lines — a reused container still holds earlier runs.
LOG_SINCE=$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)
psqlq -c "alter system set auto_explain.log_min_duration = 0" \
      -c "alter system set auto_explain.log_analyze = on" \
      -c "alter system set auto_explain.log_buffers = on" \
      -c "alter system set auto_explain.log_timing = on" \
      -c "alter system set auto_explain.log_nested_statements = on" \
      -c "alter system set auto_explain.log_parameter_max_length = -1" \
      -c "select pg_reload_conf()" >/dev/null
sleep 1

marker() { psqlq -tAc "select 'perf-marker:$1'" >/dev/null; }
explain_on() {
    psqlq -c "alter system set auto_explain.log_min_duration = 0" -c "select pg_reload_conf()" >/dev/null
    sleep 1
}
explain_off() {
    psqlq -c "alter system set auto_explain.log_min_duration = -1" -c "select pg_reload_conf()" >/dev/null
    sleep 1
}
# drive_endpoints <file>: `label | path [| cold]` per line. Each endpoint
# gets one warm-up request (catalog / plan caches), then a measured one
# bracketed by markers. `cold` skips the warm-up, so the measured request
# is the first one — e.g. an in-process cache miss.
drive_endpoints() {
    local label path mode url ref cur code t0 t1
    while IFS='|' read -r label path mode; do
        label="${label//[[:space:]]/}"; path="${path//[[:space:]]/}"; mode="${mode//[[:space:]]/}"
        [ -z "$label" ] || [ "${label:0:1}" = "#" ] && continue
        url=${path//\{series_slug\}/$SERIES_SLUG}
        url=${url//\{issue_id\}/$ISSUE_ID}
        url=${url//\{view_id\}/$VIEW_ID}
        url=${url//\{cbl_id\}/$CBL_ID}
        url=${url//\{collection_id\}/$COLLECTION_ID}
        url=${url//\{library_id\}/$LIB_ID}
        url=${url//\{large_slug\}/${LARGE_SLUG:-}}
        url=${url//\{typical_slug\}/${TYPICAL_SLUG:-}}
        url=${url//\{chain_slug\}/${CHAIN_SLUG:-}}
        url=${url//\{hub_slug\}/${HUB_SLUG:-}}
        url=${url//\{mega_arc_slug\}/${MEGA_ARC_SLUG:-}}
        url=${url//\{arc_slug\}/${ARC_SLUG:-}}
        if [[ "$url" == *"{cursor:"* ]]; then
            # {cursor:<label>} → next_cursor of that endpoint's first page.
            ref=$(echo "$url" | sed -E 's/.*\{cursor:([^}]*)\}.*/\1/')
            cur=$(jq -j '.next_cursor // empty' "$OUT/resp-$ref.json" | jq -sRr @uri)
            if [ -z "$cur" ]; then
                log "skipping $label: $ref has no next page"
                continue
            fi
            url=$(echo "$url" | sed -E "s/\{cursor:[^}]*\}/$cur/")
        fi
        [ "$mode" = "cold" ] || api "$API$url" >/dev/null 2>&1 || true
        marker "begin:$label"
        t0=$(date +%s%N)
        code=$(curl -sS -o "$OUT/resp-$label.json" -w '%{http_code}' -b "$COOKIES" "$API$url")
        t1=$(date +%s%N)
        marker "end:$label"
        printf '%s|%s|%s|%s\n' "$label" "$code" "$(( (t1 - t0) / 1000000 ))" "$url" >>"$OUT/requests.txt"
    done <"$1"
}
drive_endpoints scripts/perf/endpoints.txt

# ── phase 2: M7 relationships / similarity (WP-8.3) ─────────────────────
if [ "${PERF_M7:-1}" = "1" ]; then
    explain_off
    job_lines() { grep -c 'relationship suggestions: run complete' "$OUT/server.log" || true; }
    if [ "$SCANNED" = "1" ]; then
        # The scan's finalize step queued a run; let it finish so the
        # measured runs below are the only ones in flight.
        log "M7: waiting for the post-scan relationship_suggest run"
        for _ in $(seq 1 600); do
            [ "$(job_lines)" -ge 1 ] && break
            sleep 1
        done
    fi
    log "M7: seeding arcs, universes, series groups, AlternateSeries, curated links"
    psqlq -f scripts/perf/seed_relationships.sql >/dev/null
    psqlq -c "vacuum analyze" >/dev/null
    psqlq -tAc "select relname, reltuples::bigint from pg_class c join pg_namespace n on n.oid=c.relnamespace
                where n.nspname='public' and c.relkind='r' order by 2 desc" >"$OUT/reltuples.txt"

    # run_job <label> <explain 0|1>: one on-demand relationship_suggest run
    # over the library; wall = trigger → "run complete" log line. The job's
    # own report (elapsed_ms, counts) and per-source timings land in
    # job-<label>.log.
    run_job() {
        local label=$1 before lines t0 t1 elapsed
        before=$(job_lines)
        lines=$(wc -l <"$OUT/server.log")
        if [ "$2" = "1" ]; then explain_on; marker "begin:$label"; fi
        t0=$(date +%s%N)
        api -X POST "$API/api/admin/relationship-suggestions/run?library_id=$LIB_ID" >/dev/null
        for _ in $(seq 1 18000); do
            [ "$(job_lines)" -gt "$before" ] && break
            sleep 0.1
        done
        t1=$(date +%s%N)
        if [ "$2" = "1" ]; then marker "end:$label"; explain_off; fi
        tail -n +"$((lines + 1))" "$OUT/server.log" | grep 'relationship suggestions' >"$OUT/job-$label.log" || true
        elapsed=$(grep 'run complete' "$OUT/job-$label.log" | tail -1 | grep -oE '"elapsed_ms":[0-9]+' | cut -d: -f2)
        printf '%s|202|%s|POST /api/admin/relationship-suggestions/run (job elapsed_ms=%s)\n' \
            "$label" "$(( (t1 - t0) / 1000000 ))" "${elapsed:-?}" >>"$OUT/requests.txt"
        log "M7: $label — $(( (t1 - t0) / 1000000 )) ms wall, job elapsed_ms=${elapsed:-?}"
    }
    run_job job-suggest-first 0
    # A few hundred accepted relationships: accept the high bucket (≤ 500).
    t0=$(date +%s%N)
    api -X POST "$API/api/admin/relationship-suggestions/bulk-accept" -d '{"bucket":"high"}' \
        >"$OUT/resp-bulk-accept-high.json"
    t1=$(date +%s%N)
    printf '%s|200|%s|POST /api/admin/relationship-suggestions/bulk-accept {"bucket":"high"} (created=%s)\n' \
        bulk-accept-high "$(( (t1 - t0) / 1000000 ))" "$(jq -r .created "$OUT/resp-bulk-accept-high.json")" \
        >>"$OUT/requests.txt"
    # …then a reviewer's worth more by explicit ids (mega-event tie-ins
    # first, then by confidence) so the arc / relationships reads have a few
    # hundred accepted edges behind them.
    IDS=$(psqlq -tAc "select coalesce(json_agg(id), '[]') from (
                        select sug.id from series_relationship_suggestion sug
                        left join story_arc a on a.id = sug.to_arc_id
                        where sug.status = 'pending'
                        order by (a.slug = 'perf-arc-1') desc nulls last, sug.confidence desc, sug.id
                        limit 400) x")
    t0=$(date +%s%N)
    api -X POST "$API/api/admin/relationship-suggestions/bulk-accept" -d "{\"ids\":$IDS}" \
        >"$OUT/resp-bulk-accept-ids.json"
    t1=$(date +%s%N)
    printf '%s|200|%s|POST /api/admin/relationship-suggestions/bulk-accept {"ids":[400]} (created=%s)\n' \
        bulk-accept-ids "$(( (t1 - t0) / 1000000 ))" "$(jq -r .created "$OUT/resp-bulk-accept-ids.json")" \
        >>"$OUT/requests.txt"
    run_job job-suggest-rerun 0
    run_job job-suggest-explain 1
    psqlq -c "analyze" >/dev/null

    # Resolve the M7 placeholders (ords match seed_relationships.sql).
    ord_slug() {
        psqlq -tAc "select slug from (select slug, row_number() over (order by normalized_name, id) o
                    from series where removed_at is null) x where o = $1"
    }
    LARGE_SLUG=$(ord_slug 7)
    TYPICAL_SLUG=$(ord_slug 1230)
    CHAIN_SLUG=$(ord_slug 2083)
    HUB_SLUG=$(ord_slug 2450)
    MEGA_ARC_SLUG=perf-arc-1
    ARC_SLUG=perf-arc-50
    psqlq -tAc "select 'relationship rows', count(*) from series_relationship
                union all select 'suggestions ' || status, count(*) from series_relationship_suggestion group by status
                union all select 'issue_arcs', count(*) from issue_arcs
                union all select 'series_universes', count(*) from series_universes" >"$OUT/m7-dataset.txt"
    explain_on
    drive_endpoints scripts/perf/endpoints-m7.txt
fi

psqlq -c "alter system set auto_explain.log_min_duration = -1" -c "select pg_reload_conf()" >/dev/null
docker logs --since "$LOG_SINCE" "$PG_CONTAINER" >"$OUT/postgres.log" 2>&1

rc=0
python3 scripts/perf/explain_report.py "$OUT" --min-rows "${PERF_SEQSCAN_MIN_ROWS:-5000}" || rc=$?

# ── optional HTTP load pass (docs/dev/load-testing.md "oha recipe") ─────
# PERF_OHA=1 hammers every endpoint in endpoints.txt (auto_explain off)
# with `oha` — the local binary if installed, else the upstream container
# image — and writes perf-out/.../oha-<label>.txt. Bearer = the session JWT.
if [ "${PERF_OHA:-0}" = "1" ]; then
    TOKEN=$(awk '/comic_session/ {print $7}' "$COOKIES" | tail -1)
    if command -v oha >/dev/null 2>&1; then
        OHA=(oha)
    else
        OHA=(docker run --rm --network host "${PERF_OHA_IMAGE:-ghcr.io/hatoo/oha:latest}")
    fi
    log "oha: ${PERF_OHA_DURATION:-15s} × ${PERF_OHA_CONCURRENCY:-16} connections per endpoint"
    while IFS='|' read -r label _ _ url; do
        "${OHA[@]}" --no-tui -z "${PERF_OHA_DURATION:-15s}" -c "${PERF_OHA_CONCURRENCY:-16}" \
            -H "Authorization: Bearer $TOKEN" "$API$url" >"$OUT/oha-$label.txt" 2>&1 || true
        printf '    %-22s %s\n' "$label" \
            "$(grep -E 'Requests/sec|Success rate' "$OUT/oha-$label.txt" | tr -s ' ' | tr '\n' ' ')" >&2
    done <"$OUT/requests.txt"
fi
exit $rc
