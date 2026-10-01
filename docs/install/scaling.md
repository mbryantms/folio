# Scaling (single instance)

Folio runs as **one app instance**. That is a design decision (owner
decision, 2026-09-29), not a temporary limitation: several core pieces
of the server are process-local and have no cross-process coordination,
so a second replica would not share state with the first — it would
race it. Scale vertically. This page lists the knobs that matter and
what a multi-replica deployment would need if that decision is ever
revisited.

## Why exactly one replica

Each of these lives in the app process's memory and nothing else:

| Component | Where | What breaks with two replicas |
|---|---|---|
| Scan-event WebSocket fan-out | [`library/events.rs`](../../crates/server/src/library/events.rs) `Broadcaster` — a `tokio::sync::broadcast` channel consumed by [`api/ws_scan_events.rs`](../../crates/server/src/api/ws_scan_events.rs) | A browser connected to replica A never sees scan progress emitted by a worker on replica B. There is no Redis pub/sub relay. |
| Cron scheduler | [`jobs/scheduler.rs`](../../crates/server/src/jobs/scheduler.rs) (`tokio_cron_scheduler`) | Every replica runs every cron: scheduled scans, reconcile sweeps, prunes, the weekly metadata refresh — all fire N times. There is no leader lease. |
| Per-process mutexes and semaphores | [`state.rs`](../../crates/server/src/state.rs): `archive_work_semaphore`, `thumb_inline_semaphore`, `thumb_job_inflight`, `library_scan_job_ids`, the zip LRU | Concurrency caps are per process, so two replicas double the archive I/O load; in-flight thumbnail dedupe and scan-job bookkeeping are invisible across replicas. |
| Auto-migrate at boot | `COMIC_AUTO_MIGRATE` in [`app.rs`](../../crates/server/src/app.rs) | Two replicas racing `Migrator::up()` contend on the migration lock. |
| Secrets under `/data/secrets/` | auto-generated on first boot | Two replicas with separate data volumes mint different JWT keys and invalidate each other's sessions. |

The apalis job queue, the rate-limit / metadata token buckets, and the
per-issue archive-rewrite lock
([`archive_rewrite/mutex.rs`](../../crates/server/src/archive_rewrite/mutex.rs))
do live in Redis, so *those* would survive a second consumer — but they
are the exception, not the rule.

## Vertical scaling knobs

All worker knobs are runtime settings on `/admin/server` (DB-backed
`app_setting` rows; the `COMIC_*` env vars are the deprecated fallback)
and apply on the next restart. Defaults are derived from
`available_parallelism()` in [`config.rs`](../../crates/server/src/config.rs).

| Setting (env fallback) | Default | Governs |
|---|---|---|
| `workers.scan_count` (`COMIC_SCAN_WORKER_COUNT`) | `min(cpu, 8)` | Concurrent `scan` / `scan_series` jobs. Scans are IO-bound; raise on NVMe, lower on spinning disks or NFS. |
| `workers.post_scan_count` (`COMIC_POST_SCAN_WORKER_COUNT`) | `clamp(cpu/2, 2, 8)` | Concurrent thumbnail / search / dictionary jobs. CPU-bound (decode + encode). |
| `workers.archive_work_parallel` (`COMIC_ARCHIVE_WORK_PARALLEL`) | `clamp(cpu, 2, 8)` | Global cap on blocking archive open / hash / decode work across scanner **and** thumbnail workers. This is the real ceiling on disk pressure — several queued scans can't multiply past it. |
| `workers.thumb_inline_parallel` (`COMIC_THUMB_INLINE_PARALLEL`) | `8` | On-demand thumbnail generation when a reader hits an issue the post-scan worker hasn't reached yet. |
| `workers.scan_batch_size` (`COMIC_SCAN_BATCH_SIZE`) | `100` | Issues per DB transaction inside a series. |
| `cache.zip_lru_capacity` (`COMIC_ZIP_LRU_CAPACITY`) | `64` | Open-archive handles kept warm for page serving. |
| `COMIC_PAGE_VARIANT_CACHE_BYTES` (env only) | `2 GiB` | Byte budget of the on-disk reader page-variant cache under `/data` ([`page_variants.rs`](../../crates/server/src/library/page_variants.rs)). `0` disables caching (variants are recomputed per request). Size it to your hottest working set — this is the single biggest win for reader latency on large libraries. |

**Importing a big collection from a NAS or spinning disk?** Tick *Fast
first import* when creating the library (or *Trust fingerprint on first
import* in its settings before the first scan). The first scan then skips
the full-file BLAKE3 of every archive and the hashes are computed
afterwards by a background job, with progress on the library settings
page; duplicate copies are flagged once hashing finishes. See
[library-scanner.md § First-import lazy-hash mode](../dev/library-scanner.md#first-import-lazy-hash-mode).

Postgres connection pool: fixed at `max_connections(30)` /
`min_connections(2)` in [`app.rs`](../../crates/server/src/app.rs); there
is no env knob. A default Postgres (`max_connections = 100`) has ample
headroom for one app instance.

CPU and memory: the app is a single Rust binary plus the Next.js SSR
upstream. Give it as many cores as your scan concurrency wants; memory
is dominated by the zip LRU and in-flight thumbnail encodes (the
page-variant cache is on disk, not in RAM).

## Postgres

A single Postgres instance comfortably serves the largest realistic
install (≤100k issues, ≤50 concurrent readers — see §18 of the spec).
Read replicas and read/write splitting are not implemented.

## Redis

Single Redis instance per deployment. The app uses Redis for: the
apalis job queue, rate limiting, per-provider metadata token buckets,
WebSocket auth tickets, and the search dictionary cache. Sentinel /
Cluster modes are not tested. Redis is treated as ephemeral — see
[`backup.md`](./backup.md) for what is lost on Redis loss and how to
recover.

## Migrations

With one replica, `COMIC_AUTO_MIGRATE=true` (the default) is fine: the
binary migrates at boot before it serves traffic. If you prefer to run
migrations as a separate step (CI/CD, Kubernetes Job, or just to see the
output), set `COMIC_AUTO_MIGRATE=false` and run the migrator by hand:

```bash
docker compose -f compose.prod.yml run --rm app /app/migration up
docker compose -f compose.prod.yml up -d --no-deps app
```

On Kubernetes use `strategy: Recreate` for the app Deployment so a
rolling update never has two app pods alive at once — see
[`kubernetes.md`](./kubernetes.md).

## What multi-replica would need (not supported)

For the record, a horizontally-scaled Folio would need at least:

1. **Leader election** for the cron scheduler (a Redis lease with TTL
   and heartbeat, or a Postgres advisory lock) so scheduled scans and
   sweeps fire once.
2. **Cross-process event fan-out** — the `Broadcaster` would publish
   to a Redis pub/sub channel and every replica's WebSocket handler
   would subscribe, instead of the local `broadcast` channel.
3. **Shared concurrency caps** — the archive-work and thumbnail
   semaphores would move to Redis-backed counters, and the in-flight
   job dedupe sets with them.
4. **Shared secrets provisioning** — pre-seed `/data/secrets/` once and
   mount it into every replica (a shared RWX volume or a secret store).
5. **A read-write-many library mount** if any replica rewrites archives
   (sidecar writeback, page edits, CBR/CB7→CBZ conversion).

None of this exists in the codebase today, and there is no plan to add
it. If a single instance runs out of headroom, the intended path is the
same as before: extract the scanner into a separate worker process that
consumes the same apalis queue (the architecture in §2 of the spec was
designed to make that a small refactor). That is a *split*, not a
replica — the API/web process stays singular.
