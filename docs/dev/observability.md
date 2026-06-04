# Observability: the two streams

Folio's admin observability is split into **two distinct, non-overlapping
streams**. The guiding rule: a given event belongs to exactly one stream and is
never shown in both.

- **Server stream** — how the application runs and how users work within it:
  app-runtime logs, the durable audit trail (who-did-what), and user activity
  (reading volume, sessions). Triage-oriented.
- **Library stream** — everything the library subsystem *does*: scans,
  files, series/issues, thumbnails, covers, metadata, archive rewrites,
  health. Durable and itemized.

This document is the map. For the `tracing`/`#[handler]` conventions and
secret-redaction rules, see [logging.md](logging.md).

## Server stream

| Surface | Route | Source | Durability |
| --- | --- | --- | --- |
| Server log | `/admin/logs` | in-process ring buffer (`observability.rs`) | ephemeral (lost on restart, 5k cap) |
| Server activity | `/admin/activity` | `audit_log` + per-hour `reading_sessions` aggregate (`api/admin_activity.rs`) | durable |
| Server info | `/admin/server` | build/uptime/deps/probes | live |

**Ring buffer domain classification.** Every captured `tracing` event is tagged
with a `domain` (`server` | `library`) at capture time by `classify_domain` in
[`observability.rs`](../../crates/server/src/observability.rs): an event that
inherited `library_id`/`scan_id` span context is **library** (scanner/workers);
everything else is **server**. The Server-log view defaults to `domain=server`;
the raw library-domain tracing is still reachable via the toggle, but the
*canonical* library record is the durable manifest below.

**Error-code capture.** `api::respond` / `api::error` (the only error
construction sites) funnel through `log_api_error`, which drops every API error
into the ring buffer with `error_code` + `status` fields — 5xx at `error`
level, 4xx at `debug`. The Server-log view lifts `error_code` into a chip so an
operator can scan for `internal`, `validation`, etc.

**Activity feed** is `audit` + `reading` only. Scan + health were removed from
it (M13) — they're the Library stream.

## Library stream

| Surface | Route | Source | Durability |
| --- | --- | --- | --- |
| Library activity | `/admin/findings` | `library_events` manifest + `library_health_issues` + `scan_runs` (tabs) | durable |
| Scan dashboard | `/admin/scan-dashboard` | live `/ws/scan-events` + `scan_batch` rollup | durable + live |

### The `library_events` manifest

The canonical, durable, itemized record of every library-subsystem fact. One
row per change.

- **Table**: `library_events` (migration `m20270112…`), entity
  `crates/entity/src/library_event.rs`. Columns: `library_id`, `scan_run_id?`,
  `batch_id?`, `category`, `entity_type?`/`entity_id?`/`entity_label?`,
  `action`, `severity`, `summary`, `detail` (jsonb), `created_at`. Only
  `severity` is CHECK-constrained; `category`/`action` are free text so new
  event kinds never need a migration.
- **Writer**: [`crate::library::event_log`](../../crates/server/src/library/event_log.rs).
  `record` (single) / `record_many` (bulk) — fire-and-forget, mirroring
  `audit::record`. **Observational only**: logging an event never mutates
  provider-touched data — that's the audited `metadata/writers` surface.
- **Collector**: `EventCollector` mirrors `HealthCollector` — each parallel
  scan-folder worker buffers events locally, the consume loop `merge`s them,
  and `finalize_run` flushes the whole batch in one insert.
- **Retention**: daily prune (`event_log::prune`, scheduler 03:30 UTC) bounds
  the table by age (90d) **and** a per-library cap (50k), since a large scan
  writes thousands of rows.

### Where events come from

- **Scanner** (in-scan, carries `scan_run_id`): issue added/updated/removed/
  restored, series added/removed, file converted (CBR→CBZ), scan
  started/completed/errored. Emitted via the threaded `EventCollector`.
- **Out-of-scan jobs** (one-off `record`, no `scan_run_id`): thumbnail
  failures (`jobs/post_scan.rs`), metadata apply (`jobs/metadata_apply.rs`),
  archive edit + sidecar writeback (`jobs/archive_edit.rs`,
  `jobs/rewrite_sidecars.rs`).

Deliberate non-overlap choices: malformed/encrypted/duplicate files stay
**health-issue-only** (not double-logged as events); successful thumbnail
generation is **not** logged (only failures — the "thumbnail issues" an
operator must rectify).

### Scan-all batches

A "Scan all" creates a `scan_batch` row (migration `m20270113…`); each
newly-enqueued per-library `scan_run` adopts its `batch_id`. The batch's
terminal state (`complete` / `partial_failed` / `failed`) is derived in
`scanner::maybe_finalize_batch` once every member run finishes. The Scan
dashboard reads `GET /admin/scan-batches[/{id}]` for the rollup and overlays
live `/ws/scan-events` (tagged with `batch_id` on `Started`/`Completed`/
`Failed`; `Progress` is correlated by `library_id`).

## Reading endpoints

- `GET /admin/library-events` — cursor list over the manifest, filterable by
  `library_id`/`batch_id`/`scan_run_id`/`category`/`action`/`severity` (all
  server-side; never a client `.filter()` over a truncated page).
- `GET /admin/scan-batches` + `/admin/scan-batches/{id}` — batch list +
  per-library rollup (`BatchTotals` summed from each run's `ScanStats`,
  `event_count` for drill-down).

## Adding a new library event

1. Pick a `Category` + `Action` in
   [`event_log.rs`](../../crates/server/src/library/event_log.rs) (add a typed
   variant if needed — Rust-only, no migration).
2. Emit at the site where the change commits: inside the scanner, push onto the
   threaded `EventCollector` (`outputs.events` in the ingest path); in an
   out-of-scan job, call `event_log::record(&db, NewEvent::new(...))`.
3. Put the actionable specifics in `detail` (e.g. `path`, `error`, before/after
   counts) — the Library-activity row renders `detail.error`/`path`/`series`
   in its expandable view.
