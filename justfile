# Comic Reader — task runner
# Run `just` (no args) to list commands.

set shell := ["bash", "-cu"]

# Auto-load .env into every recipe's environment. Without this, COMIC_*
# variables like COMIC_DATABASE_URL aren't visible to `migrate`/`run-server`/etc.
set dotenv-load := true

# Default: list commands
default:
    @just --list

# ───── one-time setup ─────

bootstrap:
    @echo "==> Bootstrapping dev environment"
    @command -v rustup >/dev/null 2>&1 || (echo "Install rustup: https://rustup.rs" && exit 1)
    @command -v pnpm >/dev/null 2>&1   || (echo "Install pnpm: https://pnpm.io" && exit 1)
    @command -v just  >/dev/null 2>&1  || (echo "Install just: cargo install just" && exit 1)
    @command -v docker >/dev/null 2>&1 || (echo "Install docker" && exit 1)
    @test -f .env || (cp .env.example .env && echo "==> Created .env from .env.example — edit it now")
    pnpm install
    cargo fetch
    @echo "==> Bootstrap complete. Next: just dev-services-up && just migrate && just dev"

# ───── dev services ─────

# `--wait` blocks until each container's healthcheck passes (Postgres takes
# ~3-5s to initialize on first run); without it, `just migrate` immediately
# after will hit "connection reset by peer".
#
# The `[ -f ... ]` guard restores the committed dex-config.yaml when missing —
# otherwise Docker auto-creates it as a directory on first `up`, which puts
# dex into a restart loop with "is a directory" errors.
dev-services-up:
    @[ -f .dev-data/dex-config.yaml ] || git checkout .dev-data/dex-config.yaml
    docker compose -f compose.dev.yml up -d --wait

dev-services-down:
    docker compose -f compose.dev.yml down

dev-services-reset:
    docker compose -f compose.dev.yml down -v --remove-orphans
    mkdir -p .dev-data
    # Postgres bind-mount files are owned by the container user, so a host
    # `rm -rf` often leaves the "fresh" DB behind. Remove dev state from a
    # disposable container running as root, and include app data so thumbnails,
    # generated secrets, and stale cookies are invalidated too.
    docker run --rm -v "$PWD/.dev-data:/data" postgres:17-alpine sh -c 'rm -rf /data/postgres /data/redis /data/app'
    @[ -f .dev-data/dex-config.yaml ] || git checkout .dev-data/dex-config.yaml

# Stop the app, wipe all persisted dev state, recreate services, and run
# migrations. After this, register/login again; the first local user becomes
# admin.
dev-fresh: dev-stop dev-services-reset dev-services-up migrate
    @echo "==> Fresh dev environment ready"
    @echo "    Start the app with: just dev"
    @echo "    Then clear browser site data for localhost:8080 or use a private window."

dev-services-logs:
    docker compose -f compose.dev.yml logs -f

# Stop application workers and clear all Redis-backed dev queues. This drops
# pending scans, queued page-map thumbnail jobs, scan coalescing markers, and
# short-lived WebSocket tickets. It does not touch Postgres rows or on-disk
# thumbnails. Use when a scan/thumb backlog was queued by mistake and you want
# the app to restart idle.
dev-queues-clear: dev-stop
    docker compose -f compose.dev.yml exec -T redis redis-cli FLUSHDB
    @echo "==> Cleared Redis dev queues. Restart with: just dev"

# Inspect Redis queue state directly. `just dev-status` shows running
# processes; this shows whether Redis still has background-job keys.
dev-queues-status:
    docker compose -f compose.dev.yml exec -T redis redis-cli DBSIZE
    docker compose -f compose.dev.yml exec -T redis redis-cli --scan --pattern '*Job*'
    docker compose -f compose.dev.yml exec -T redis redis-cli --scan --pattern 'scan:*'

# ───── migrations ─────

migrate:
    cargo run --bin migration -- up

migrate-down:
    cargo run --bin migration -- down

migrate-fresh:
    cargo run --bin migration -- fresh

migrate-new name:
    cargo run --bin migration -- generate {{name}}

# ───── seed fixtures ─────

# Regenerate the placeholder CBZ fixtures from fixtures/build.py. Real
# public-domain comics (per spec §13.4) live behind Git LFS — these are colored
# panels stamped with series + issue + page number, enough to verify scan,
# thumbnails, search, and the future reader.
regen-fixtures:
    python3 fixtures/build.py

# Generate the perf-profiling stress fixture (~50 series × ~20 issues = ~1000
# CBZs). Output goes to fixtures/library-stress/ (gitignored). Idempotent —
# wipes the prior set before regenerating. See docs/dev/scanner-perf.md.
regen-fixtures-stress:
    python3 fixtures/build.py --scale stress

# Create the Fixtures library (idempotent) and run a scan.
# Defaults to the spec's test admin creds; pass `email=... password=...` to override.
# Requires `just dev` (or just `just run-server`) to be running.
seed-fixtures email='first@example.com' password='Atvvasa22Atvvasa22' api='http://127.0.0.1:8080':
    #!/usr/bin/env bash
    set -euo pipefail

    ROOT="$PWD/fixtures/library"
    COOKIES=$(mktemp -t comic-seed.XXXXXX)
    trap 'rm -f "$COOKIES"' EXIT

    if [ ! -d "$ROOT" ]; then
        echo "==> $ROOT does not exist; create CBZ fixtures there first." >&2
        exit 1
    fi

    BODY=$(printf '{"email":"%s","password":"%s"}' '{{email}}' '{{password}}')

    echo "==> Logging in as {{email}} ({{api}})"
    LOGIN_STATUS=$(curl -sS -o /dev/null -w '%{http_code}' -c "$COOKIES" \
        -X POST "{{api}}/auth/local/login" \
        -H 'Content-Type: application/json' -d "$BODY")
    if [ "$LOGIN_STATUS" = "401" ] || [ "$LOGIN_STATUS" = "403" ]; then
        # Fresh DB: nobody is registered yet. Register and let the first-user
        # admin bootstrap kick in. On a non-fresh DB this 409s and we fall
        # through to the generic failure path below.
        echo "==> Login returned $LOGIN_STATUS; trying register (first-user-admin bootstrap)"
        REG_STATUS=$(curl -sS -o /dev/null -w '%{http_code}' -c "$COOKIES" \
            -X POST "{{api}}/auth/local/register" \
            -H 'Content-Type: application/json' -d "$BODY")
        if [ "$REG_STATUS" != "201" ]; then
            echo "==> Register also failed (HTTP $REG_STATUS)." >&2
            echo "    If a different account owns the DB, pass email=... password=..." >&2
            exit 1
        fi
    elif [ "$LOGIN_STATUS" != "200" ]; then
        echo "==> Login failed (HTTP $LOGIN_STATUS)." >&2
        exit 1
    fi

    CSRF=$(awk '/__Host-comic_csrf/ {print $7}' "$COOKIES")
    if [ -z "$CSRF" ]; then
        echo "==> No CSRF cookie returned — server response unexpected." >&2
        exit 1
    fi

    echo "==> Ensuring library exists at $ROOT"
    STATUS=$(curl -sS -o /dev/null -w '%{http_code}' \
        -b "$COOKIES" -X POST "{{api}}/libraries" \
        -H 'Content-Type: application/json' -H "X-CSRF-Token: $CSRF" \
        -d "$(printf '{"name":"Fixtures","root_path":"%s"}' "$ROOT")")
    case "$STATUS" in
        201) echo "    created" ;;
        409) echo "    already exists" ;;
        403)
            echo "    forbidden — {{email}} is not an admin" >&2
            echo "    The DB already has an admin (the first registered user)." >&2
            echo "    Either pass that admin's credentials (email=... password=...)" >&2
            echo "    or wipe the DB with 'just dev-services-reset' and re-run." >&2
            exit 1
            ;;
        *)   echo "    unexpected status $STATUS" >&2; exit 1 ;;
    esac

    LIB_ID=$(ROOT="$ROOT" curl -sSf -b "$COOKIES" "{{api}}/libraries" \
        | ROOT="$ROOT" python3 -c "import json,sys,os; r=os.environ['ROOT']; print(next(l['id'] for l in json.load(sys.stdin) if l['root_path']==r))")

    echo "==> Triggering scan ($LIB_ID)"
    curl -sSf -b "$COOKIES" -X POST "{{api}}/libraries/$LIB_ID/scan" \
        -H "X-CSRF-Token: $CSRF" | python3 -m json.tool

# ───── perf-profiling ─────

# Register the Stress fixture as a library named "Stress" and trigger an
# initial baseline scan (so a subsequent `just perf-scan` measures incremental
# behavior on a warmed DB rather than first-import). Mirrors `seed-fixtures`
# but points at fixtures/library-stress/. Requires `just dev` running and the
# stress fixture already generated (`just regen-fixtures-stress`).
seed-fixtures-stress email='first@example.com' password='Atvvasa22Atvvasa22' api='http://127.0.0.1:8080':
    #!/usr/bin/env bash
    set -euo pipefail

    ROOT="$PWD/fixtures/library-stress"
    COOKIES=$(mktemp -t comic-seed-stress.XXXXXX)
    trap 'rm -f "$COOKIES"' EXIT

    if [ ! -d "$ROOT" ]; then
        echo "==> $ROOT does not exist; run 'just regen-fixtures-stress' first." >&2
        exit 1
    fi

    BODY=$(printf '{"email":"%s","password":"%s"}' '{{email}}' '{{password}}')

    echo "==> Logging in as {{email}}"
    LOGIN_STATUS=$(curl -sS -o /dev/null -w '%{http_code}' -c "$COOKIES" \
        -X POST "{{api}}/auth/local/login" \
        -H 'Content-Type: application/json' -d "$BODY")
    if [ "$LOGIN_STATUS" != "200" ]; then
        echo "==> Login failed (HTTP $LOGIN_STATUS); seed Fixtures first." >&2
        exit 1
    fi
    CSRF=$(awk '/__Host-comic_csrf/ {print $7}' "$COOKIES")

    echo "==> Ensuring Stress library exists at $ROOT"
    STATUS=$(curl -sS -o /dev/null -w '%{http_code}' \
        -b "$COOKIES" -X POST "{{api}}/libraries" \
        -H 'Content-Type: application/json' -H "X-CSRF-Token: $CSRF" \
        -d "$(printf '{"name":"Stress","root_path":"%s"}' "$ROOT")")
    case "$STATUS" in
        201) echo "    created" ;;
        409) echo "    already exists" ;;
        *)   echo "    unexpected status $STATUS" >&2; exit 1 ;;
    esac

    LIB_ID=$(ROOT="$ROOT" curl -sSf -b "$COOKIES" "{{api}}/libraries" \
        | ROOT="$ROOT" python3 -c "import json,sys,os; r=os.environ['ROOT']; print(next(l['id'] for l in json.load(sys.stdin) if l['root_path']==r))")

    echo "==> Baseline scan ($LIB_ID) — wait for completion before perf-scan"
    curl -sSf -b "$COOKIES" -X POST "{{api}}/libraries/$LIB_ID/scan" \
        -H "X-CSRF-Token: $CSRF" | python3 -m json.tool

# Drive a measured scan and capture flamegraph + pg_stat_statements + phase
# timings into perf-out/ for analysis. Assumes:
#   - dev services up (`just dev-services-up`)
#   - Stress library seeded (`just seed-fixtures-stress`) and idle
#   - cargo flamegraph + perf installed (the recipe checks)
# All artifacts land in perf-out/ (gitignored). Re-run after a code change to
# get fresh numbers. The recipe builds a release binary with line-tables-only
# debug symbols (see Cargo.toml [profile.release] override).
perf-scan email='first@example.com' password='Atvvasa22Atvvasa22' api='http://127.0.0.1:8080' force='true':
    #!/usr/bin/env bash
    set -euo pipefail

    OUT_DIR="$PWD/perf-out"
    mkdir -p "$OUT_DIR"
    TS=$(date -u +%Y%m%dT%H%M%SZ)
    FLAME_SVG="$OUT_DIR/flame-$TS.svg"
    PG_DUMP="$OUT_DIR/pg_stats-$TS.csv"
    PHASES_JSON="$OUT_DIR/phase_timings-$TS.json"
    echo "==> perf-scan run $TS — outputs in $OUT_DIR/"

    # Prereqs
    if ! command -v cargo-flamegraph >/dev/null 2>&1; then
        echo "==> cargo flamegraph not installed:" >&2
        echo "    cargo install flamegraph" >&2
        exit 1
    fi
    if ! command -v perf >/dev/null 2>&1; then
        echo "==> perf not on PATH (linux-tools-\$(uname -r)?)" >&2
        exit 1
    fi
    if [ "$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo 4)" -gt 1 ]; then
        echo "==> /proc/sys/kernel/perf_event_paranoid > 1 — perf may be denied." >&2
        echo "    Run once: sudo sysctl -w kernel.perf_event_paranoid=1" >&2
    fi

    # Stop any running dev server so we can launch the release build under perf.
    just dev-stop || true

    # 1. Reset pg_stat_statements (first install gracefully if missing).
    echo "==> Resetting pg_stat_statements"
    PGPASSWORD=comic psql -h 127.0.0.1 -p 5432 -U comic -d comic_reader \
        -c 'CREATE EXTENSION IF NOT EXISTS pg_stat_statements;' \
        -c 'SELECT pg_stat_statements_reset();' >/dev/null

    # 2. Build release binary with debug symbols.
    echo "==> cargo build --release --bin server (this is the slow step)"
    cargo build --release --bin server

    # 3. Login + locate the Stress library + scan_id endpoint shape.
    COOKIES=$(mktemp -t perf-scan.XXXXXX)
    trap 'rm -f "$COOKIES"; kill $FLAMEGRAPH_PID 2>/dev/null || true' EXIT

    BODY=$(printf '{"email":"%s","password":"%s"}' '{{email}}' '{{password}}')

    # 4. Start the server under flamegraph in the background. cargo flamegraph
    # wraps the binary in `perf record` and finalizes the SVG on the wrapper's
    # SIGINT. We record CPU samples for the whole scan window.
    echo "==> Launching server under flamegraph (output: $FLAME_SVG)"
    cargo flamegraph --release --bin server -o "$FLAME_SVG" -- &
    FLAMEGRAPH_PID=$!

    # 5. Wait for the server to come up.
    for i in $(seq 1 60); do
        if curl -sS -o /dev/null "{{api}}/healthz" 2>/dev/null; then break; fi
        sleep 1
    done
    if ! curl -sS -o /dev/null "{{api}}/healthz"; then
        echo "==> server did not become ready in 60s" >&2
        exit 1
    fi

    echo "==> Logging in"
    LOGIN_STATUS=$(curl -sS -o /dev/null -w '%{http_code}' -c "$COOKIES" \
        -X POST "{{api}}/auth/local/login" \
        -H 'Content-Type: application/json' -d "$BODY")
    [ "$LOGIN_STATUS" = "200" ] || { echo "login HTTP $LOGIN_STATUS" >&2; exit 1; }
    CSRF=$(awk '/__Host-comic_csrf/ {print $7}' "$COOKIES")

    LIB_ID=$(curl -sSf -b "$COOKIES" "{{api}}/libraries" \
        | python3 -c "import json,sys; print(next(l['id'] for l in json.load(sys.stdin) if l['name']=='Stress'))")

    # 6. Trigger the scan with force={{force}} (default true → measure the
    # full pipeline; pass force=false to measure the incremental fast-path).
    echo "==> Triggering scan ($LIB_ID, force={{force}})"
    SCAN_RESP=$(curl -sSf -b "$COOKIES" -X POST \
        "{{api}}/libraries/$LIB_ID/scan?force={{force}}" \
        -H "X-CSRF-Token: $CSRF")
    echo "$SCAN_RESP" | python3 -m json.tool
    SCAN_ID=$(echo "$SCAN_RESP" | python3 -c "import json,sys; print(json.load(sys.stdin)['scan_id'])")

    # 7. Poll for completion.
    echo "==> Waiting for scan $SCAN_ID to complete"
    for i in $(seq 1 600); do
        STATE=$(curl -sSf -b "$COOKIES" "{{api}}/libraries/$LIB_ID/scan-runs?limit=1" \
            | python3 -c "import json,sys; rows=json.load(sys.stdin); print(rows[0]['state'] if rows else 'queued')" 2>/dev/null || echo queued)
        case "$STATE" in
            complete|failed) echo "==> scan ended: $STATE"; break ;;
            *) sleep 2 ;;
        esac
    done

    # 8. Capture pg_stat_statements top queries.
    echo "==> Dumping pg_stat_statements → $PG_DUMP"
    PGPASSWORD=comic psql -h 127.0.0.1 -p 5432 -U comic -d comic_reader \
        -c "\copy (
            SELECT calls, total_exec_time::int AS total_ms,
                   mean_exec_time::int AS mean_ms, rows,
                   shared_blks_hit, shared_blks_read,
                   substring(query for 200) AS query
              FROM pg_stat_statements
              WHERE query NOT ILIKE '%pg_stat_statements%'
              ORDER BY total_exec_time DESC LIMIT 30
        ) TO '$PG_DUMP' WITH CSV HEADER"

    # 9. Capture the scan_runs phase timings.
    echo "==> Dumping scan_runs.stats → $PHASES_JSON"
    PGPASSWORD=comic psql -h 127.0.0.1 -p 5432 -U comic -d comic_reader \
        -tAc "SELECT stats FROM scan_runs WHERE id = '$SCAN_ID'" > "$PHASES_JSON"

    # 10. Stop the flamegraph wrapper so it finalizes the SVG.
    echo "==> Stopping flamegraph wrapper"
    kill -INT $FLAMEGRAPH_PID || true
    wait $FLAMEGRAPH_PID 2>/dev/null || true
    trap - EXIT
    rm -f "$COOKIES"

    echo
    echo "==> done. artifacts:"
    echo "    $FLAME_SVG"
    echo "    $PG_DUMP"
    echo "    $PHASES_JSON"

# ───── run ─────

dev: dev-stop
    #!/usr/bin/env bash
    set -euo pipefail
    if ! cargo watch --version >/dev/null 2>&1; then
      echo "==> cargo-watch not installed; falling back to plain 'cargo run'."
      echo "    Install with 'cargo install cargo-watch' for auto-reload on changes."
      RUNNER='cargo run --bin server'
    else
      # `-w crates` scopes the fs watch to source files. Watching the whole
      # workspace from CWD trips on .dev-data/postgres/ (mode 0700, uid 70 —
      # the docker postgres container user) with "Permission denied".
      RUNNER='cargo watch -w crates -x "run --bin server"'
    fi
    echo "==> Starting server and web (pnpm dev) in parallel"
    trap 'kill 0' EXIT
    # Run from workspace root so COMIC_DATA_PATH=./.dev-data/app resolves to
    # the same root .dev-data/ that docker compose uses (postgres, dex). If
    # the server cd's into crates/server/ first, it creates a parallel
    # crates/server/.dev-data/ that diverges from the docker-managed state.
    eval "$RUNNER" 2>&1 | sed 's/^/[server] /' &
    (cd web && pnpm dev 2>&1 | sed 's/^/[web]    /') &
    wait

run-server:
    cargo run --bin server

run-web:
    cd web && pnpm dev

# Kill any stray dev processes (server, cargo-watch, next-server). Runs as a
# precondition to `just dev` so a fresh start can't collide with a leaked
# server from a previous session — the recurring "EADDRINUSE" / "stale binary
# answering on port 8080" footgun. Safe to invoke standalone.
dev-stop:
    #!/usr/bin/env bash
    set -uo pipefail
    killed=0
    # The Rust server first (graceful shutdown via SIGTERM).
    if pgrep -f 'target/debug/server' >/dev/null; then
      pkill -TERM -f 'target/debug/server' || true
      sleep 1
      pkill -KILL -f 'target/debug/server' 2>/dev/null || true
      killed=1
    fi
    # cargo-watch and any cargo-spawned `cargo run --bin server`.
    if pgrep -f 'cargo[- ]watch.*server' >/dev/null; then
      pkill -TERM -f 'cargo[- ]watch.*server' || true
      killed=1
    fi
    if pgrep -f 'cargo run --bin server' >/dev/null; then
      pkill -TERM -f 'cargo run --bin server' || true
      killed=1
    fi
    # Next.js dev (turbopack OR webpack).
    if pgrep -f 'next-server\|next dev' >/dev/null; then
      pkill -TERM -f 'next-server\|next dev' || true
      killed=1
    fi
    if [ "$killed" = "1" ]; then
      echo "==> Stopped existing dev processes"
    fi

# Show the truth about what's running locally: server PIDs, who owns port
# 8080, and whether the running binary matches `git rev-parse HEAD`. Use
# this when something feels off — when health/queue endpoints time out, or
# after a restart that didn't seem to take.
dev-status:
    #!/usr/bin/env bash
    set -uo pipefail
    echo "═══ Rust server processes ═══"
    pids=$(pgrep -f 'target/debug/server' || true)
    if [ -z "$pids" ]; then
      echo "  (none running)"
    else
      ps -o pid,etime,pcpu,rss,cmd -p $pids 2>/dev/null | sed 's/^/  /'
      n=$(echo "$pids" | wc -l)
      if [ "$n" -gt 1 ]; then
        echo "  ⚠ $n server processes running — likely orphans. Use 'just dev-stop'."
      fi
    fi
    echo
    echo "═══ Port 8080 binding ═══"
    if owner=$(ss -tlnp 2>/dev/null | awk '/:8080 /{print}'); then
      if [ -z "$owner" ]; then
        echo "  (port 8080 not bound)"
      else
        echo "$owner" | sed 's/^/  /'
      fi
    fi
    echo
    echo "═══ Live /healthz vs source ═══"
    health=$(curl -sS --max-time 2 http://127.0.0.1:8080/healthz 2>/dev/null || true)
    if [ -z "$health" ]; then
      echo "  ✗ no response from http://127.0.0.1:8080/healthz"
    else
      live_sha=$(printf '%s' "$health" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("build_sha","?"))')
      live_epoch=$(printf '%s' "$health" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("build_epoch",0))')
      uptime=$(printf '%s' "$health" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("uptime_seconds",0))')
      head_sha=$(git rev-parse --short=12 HEAD 2>/dev/null || echo unknown)
      dirty=""
      if [ -n "$(git status --porcelain 2>/dev/null)" ]; then
        dirty="-dirty"
      fi
      head_label="${head_sha}${dirty}"
      built_at=$(date -d "@$live_epoch" '+%Y-%m-%d %H:%M:%S' 2>/dev/null || echo "epoch=$live_epoch")
      echo "  running:  $live_sha  (built $built_at, uptime ${uptime}s)"
      echo "  HEAD:     $head_label"
      if [ "$live_sha" = "$head_label" ]; then
        echo "  ✓ build_sha matches HEAD"
      else
        echo "  ⚠ binary is stale — rebuild + restart to pick up source changes"
      fi
      # Also compare to the on-disk binary's mtime — catches the
      # "I just `cargo build` but forgot to restart" case where the SHA
      # matches but the running process predates the new artifact.
      if [ -f target/debug/server ]; then
        bin_epoch=$(stat -c %Y target/debug/server 2>/dev/null || echo 0)
        if [ "$bin_epoch" -gt "$live_epoch" ]; then
          delta=$(( bin_epoch - live_epoch ))
          echo "  ⚠ target/debug/server is ${delta}s newer than the running process — restart to pick it up"
        fi
      fi
    fi
    echo
    echo "═══ Port 3000 (Next.js dev) ═══"
    if owner=$(ss -tlnp 2>/dev/null | awk '/:3000 /{print}'); then
      if [ -z "$owner" ]; then
        echo "  (port 3000 not bound)"
      else
        echo "$owner" | sed 's/^/  /'
      fi
    fi

# ───── test ─────

test: test-rust test-web

test-rust:
    cargo test --workspace --all-features

test-web:
    cd web && pnpm test

test-e2e:
    cd web && pnpm test:e2e

# ───── lint / format ─────

lint: lint-rust lint-web

lint-rust:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

lint-web:
    cd web && pnpm lint

fmt: fmt-rust fmt-web

fmt-rust:
    cargo fmt --all

fmt-web:
    cd web && pnpm exec prettier --write .

fmt-check:
    cargo fmt --all -- --check
    cd web && pnpm exec prettier --check .

# ───── audit / security ─────

audit:
    cargo audit
    cargo deny check
    cd web && pnpm audit --audit-level=high || true

# ───── OpenAPI ─────

openapi:
    cargo run --bin server -- --emit-openapi > web/lib/api/openapi.json
    cd web && pnpm run openapi:gen

openapi-check:
    cargo run --bin server -- --emit-openapi > /tmp/openapi.json
    diff -q /tmp/openapi.json web/lib/api/openapi.json || \
        (echo "==> OpenAPI spec drifted. Run 'just openapi'." && exit 1)

# ───── docker ─────

# Build both production images locally. Tags them with the `:dev` suffix so
# `compose.test.yml` and `compose.prod.yml` (with `TAG=dev`) pick them up.
docker-build:
    docker build -t folio:dev -f Dockerfile .
    docker build -t folio-web:dev -f web/Dockerfile .

# Smoke-test the locally-built images end-to-end:
#   /healthz, /readyz, and the Next.js sign-in page must all respond.
#
# We bring the stack up with --wait (healthchecks gate readiness) and then
# `docker compose run` the smoke service so its exit code is the only one
# that drives recipe success. --abort-on-container-exit can't be used here
# because the `app-init` chown container is intentionally short-lived.
docker-test:
    @echo "==> Smoke-testing folio:dev + folio-web:dev"
    docker compose -f compose.test.yml up -d --wait --wait-timeout 90 postgres redis app web
    docker compose -f compose.test.yml run --rm smoke
    docker compose -f compose.test.yml down --volumes --remove-orphans
