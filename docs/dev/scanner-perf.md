<!-- markdownlint-disable MD060 -->

# Library Scanner — Performance Profile

A diagnostic baseline for the library scanner. The goal is to surface the
actual cost shape of a real scan run so future optimization work has
concrete numbers to target.

> **Status (2026-05-09).** Findings F-1, F-2, F-6, F-10, F-12 shipped
> in the DB-write throughput plan. Cold-scan wall improved 1.8 % (15.05 s
> → 14.78 s); the **bigger win is on force-rescans where the
> junction-set diff (F-10) saved ~9 %** (14.8 s → 13.4 s). The wall
> ceiling on cold scans is set by IO (kernel `vfs_read` paths, F-9
> below) — DB optimizations have measurable but bounded impact. F-11
> (bulk INSERTs) is **deferred**: estimated ~2 % more wall, not worth
> the refactor risk against an IO-bound ceiling.
>
> **History.** This doc previously anchored on a synthetic 1000-CBZ /
> 4 MB workload. Those numbers were useful for surfacing DB-side
> hotspots that don't dominate at scale, but misleading as a perf
> proxy for actual self-hosted libraries. Replaced 2026-05-09 with
> the real-library baseline below; the synthetic numbers live in
> git history if you need them.

## Methodology

### Workload

A real comic library at `fixtures/library-stress/` (gitignored — owner-
specific):

- **1395 CBZs across 60 series**, 67 GB total on disk.
- Real `series.json` sidecars on every folder (61 folders, 60 with
  CBZs — one folder is empty and emits a `EmptyFolder` health issue).
- Mix of single-issues, run-length series (Walking Dead 193 issues,
  Invincible 145, Walking Dead Deluxe 139, Saga 72, Monstress 61),
  and large omnibus volumes (largest single archive: 605 MB
  *Invincible Universe Battle Beast*).
- Every CBZ carries a real ComicInfo.xml with the full metadata
  surface — Writer / Penciller / Inker / Colorist / Letterer /
  CoverArtist / Editor / Tags / Genre / Pages array — so the
  metadata-rollup junction-table inserts run with realistic
  cardinality.

The stats below are host-and-data specific: they're a baseline for
this machine + this library, not a universal benchmark.

### Environment

- Hardware: developer workstation. Fast NVMe (cold-scan throughput
  was 4.7 GB/s, indicating Gen3+ SSD or warm page cache from the
  fixture generation step). Multi-core CPU (default
  `COMIC_SCAN_WORKER_COUNT = min(cpu, 4)`).
- Software: Postgres 17 (alpine, dockerized via
  [`compose.dev.yml`](../../compose.dev.yml)), Redis 7,
  Rust 1.91.1, sea-orm 1.1, sqlx 0.8.
- Postgres config: `shared_preload_libraries = pg_stat_statements,
  pg_stat_statements.track = all`. Loaded on container start; the
  dev data dir gets the extension via
  [`.dev-data/pg-init/01-pg-stat-statements.sql`](../../.dev-data/pg-init/01-pg-stat-statements.sql)
  on a fresh volume, or via `CREATE EXTENSION IF NOT EXISTS
  pg_stat_statements` on an existing one.
- Server build: `cargo build --release --bin server` with
  `[profile.release]` overridden to `strip = "none"` and
  `debug = "line-tables-only"` (see
  [`Cargo.toml`](../../Cargo.toml)) so `cargo flamegraph`
  resolves symbols.
- Flamegraph capture requires:
  - `linux-tools-$(uname -r)` (provides `perf`) installed
  - `kernel.perf_event_paranoid ≤ 1` (default Arch is 2). Set with
    `sudo sysctl -w kernel.perf_event_paranoid=1`.

### How to reproduce

```sh
just dev-services-up                  # postgres with pg_stat_statements
# (populate fixtures/library-stress/ with your own CBZs)
PGPASSWORD=comic psql -h 127.0.0.1 -p 5432 -U comic -d comic_reader \
  -c 'DELETE FROM libraries WHERE slug = $$stress$$;' \
  -c 'SELECT pg_stat_statements_reset();'
redis-cli -p 6380 FLUSHALL                # clear apalis state
just seed-fixtures-stress             # creates Stress library
just perf-scan                        # cold scan, force=true (under flamegraph)
just perf-scan force=false            # incremental rescan profile
```

Outputs land in `perf-out/` (gitignored): `flame-real-<TS>.svg`,
`pg_stats-real-<TS>.csv`, `phase_timings-real-<TS>.json`,
`server-real-<TS>.log`.

## Baseline — cold full scan (force=true, fresh library row)

`POST /libraries/stress/scan?force=true` against an empty DB:

| Metric | Before | After (Phases 1-5 shipped) | Δ |
|---|---:|---:|---:|
| Wall time | **15,051 ms** | **14,779 ms** | -1.8 % |
| Files seen / added | 1395 / 1395 | 1395 / 1395 | — |
| Series created | 60 | 60 | — |
| Throughput | 92.7 files/s, 4.71 GB/s | 94.4 files/s, 4.79 GB/s | +1.8 % |
| Bytes hashed | 70.87 GB | 70.87 GB | — |

The cold-scan ceiling is set by IO (see Flamegraph reading below).
The bigger DB-side win is on **force-rescans against existing rows**
where F-10 (junction-set diff) skips the redundant DELETE+INSERT
churn — see "Force rescan" further down.

### Phase breakdown

Phase totals are split into two maps as of the F-6 instrumentation
fix:

- **Serial phases** (`phase_timings_ms`): wall-clock for top-level
  observers — `plan`, `enumerate`, `reconcile`, `thumbnail_enqueue`,
  plus the `process` umbrella that wraps the parallel folder loop.
- **Parallel phases** (`parallel_phase_timings_ms`): summed across
  `parallel_workers` workers (= `state.cfg.scan_worker_count`, 4
  here). Divide by `parallel_workers` to estimate wall contribution,
  noting that workers don't all stay busy uniformly.

| Phase | Class | After (Σ ms) | ≈ wall @ 4 | Notes |
|---|---|---:|---:|---|
| `process` (umbrella) | serial | 13,590 | 13,590 | Wraps the parallel folder loop |
| `db_write` | parallel | 42,827 | 10,707 | INSERT issues + issue_paths + junction inserts |
| `hash` | parallel | 33,443 | 8,361 | BLAKE3 of every byte. Actually IO-bound. |
| `page_probe` | parallel | 3,369 | 842 | PNG dimension probe per page |
| `metadata_rollup` | parallel | 774 | 194 | Junction refresh (with F-10 diff short-circuit) |
| `reconcile` | serial | 610 | 610 | Library tombstone + status reconcile |
| `thumbnail_enqueue` | serial | 541 | 541 | Push thumb jobs |
| `identity` | parallel | 477 | 119 | Series identity (with F-2 slug pre-fetch) |
| `archive_parse` | parallel | 150 | 37 | ComicInfo XML parse |
| `plan` | serial | 1 | 1 | Folder enumeration |

Headline accounting at 4 workers:
~11 s `db_write` wall + ~8 s `hash` wall (overlapped per-archive) +
~3.5 s of remaining work serialized at the end (reconcile, enqueue) =
~15 s elapsed. Matches observed.

### Flamegraph reading (the surprise finding)

The flamegraph at `perf-out/flame-real-<TS>.svg` shows where CPU
samples actually land. The single biggest finding: **the cold scan is
IO-bound at the kernel level**. Top frames by self-time:

| % CPU | Frame | Why |
|---:|---|---|
| 85.3 % | `entry_SYSCALL_64_after_hwframe` → `do_syscall_64` | Almost all CPU time is in syscall paths. |
| 63.3 % | `ksys_read` → `vfs_read` → `filemap_read` | Reading file bytes from page cache or disk. |
| 38.6 % | `filemap_get_pages` | Page-cache lookup. |
| 33.4 % | `page_cache_ra_unbounded` / `do_page_cache_ra` | Readahead — kernel pulling more pages into cache. |
| 23.1 % | `_copy_to_iter` / `copy_page_to_iter` | Copying page-cache bytes into BLAKE3's user-space buffer. |
| 21.0 % | `folio_alloc_noprof` / `alloc_pages_mpol` | Page allocation for new readahead pages. |
| 16.5 % | `kernel_init_pages` | Zeroing newly-allocated pages. |
| 6.9 % | `__sys_sendto` → `tcp_sendmsg` | Postgres protocol writes (INSERT statements + scan-event broadcasts). |
| 4.5 % | `__x64_sys_futex` | Tokio synchronization (mutex / condvar). |
| 4.0 % | `do_epoll_wait` | Tokio reactor waits. |

**No user-space frame appears in the top 30 — not BLAKE3, not zip,
not serde_xml_rs, not sea-orm, not serde_json.** The CPU is spending
its time pulling 67 GB of file bytes through the kernel into
userspace; the BLAKE3 SIMD that consumes those bytes is fast enough
to disappear into the noise.

This inverts the synthetic-baseline narrative. On synthetic data
(4 MB total) IO is free and DB writes look dominant. On real data
(67 GB) DB writes are still big in absolute terms, but **the wall
ceiling is set by how fast the OS can fill BLAKE3's read buffer**.

### Top queries by total time

Captured via `pg_stat_statements`; full CSV at
[`perf-out/pg_stats-after-<TS>.csv`](../../perf-out/) (gitignored).
Numbers below are post-fix; the original cold-scan numbers (pre-fix)
are preserved in git history.

| Query (truncated) | Calls (after) | Total ms (after) | Mean ms | Notes |
|---|---:|---:|---:|---|
| `INSERT INTO issues (...)` | 1395 | 666 | 0.48 | One per archive, intrinsic. F-11 (bulk INSERT) deferred — would batch this. |
| `SELECT … WHERE id = ? FOR KEY SHARE OF x` | 12,927 | **128** | 0.01 | Pg auto-FK locks. **74 % drop** from F-10 — fewer junction-table writes means fewer FK-lock acquisitions. |
| `INSERT INTO issue_paths (...)` | 1395 | 104 | 0.07 | Alias-table insert (also F-11 candidate). |
| `INSERT INTO issue_credits (...)` × ~10 grouped variants | total ~600 | total ~600 | varies | Sea-orm groups inserts by row-count. Same as before for cold scan; F-10 helps re-scans. |
| `SELECT issue_credits …` (F-10 diff fetch) | 1395 | 36 | 0.03 | NEW: existing-set lookup before DELETE/INSERT. |
| `UPDATE issue_paths SET is_primary` | 1395 | 33 | 0.02 | Idempotent re-set. |
| `SELECT COUNT(*) FROM issues WHERE series_id = ? AND slug = ?` | **0** | **0** | — | **GONE — F-2 slug pre-fetch eliminated this N+1 entirely.** |
| `SELECT * FROM issues WHERE id = ?` (re-fetch path) | dropped from top-15 | — | — | F-1 row pass-through removed the per-archive re-fetch. |

Sea-orm uses prepared statements via sqlx, so per-call planning is
amortized after the first invocation per connection. Hot queries all
hit indexes (verified below).

### EXPLAIN ANALYZE on the top queries

| Query | Index | Buffers | Exec time |
|---|---|---:|---:|
| `SELECT * FROM issues WHERE id = ?` | `issues_pkey` (id) | hit=3 | 0.13 ms |
| `SELECT id, slug FROM issues WHERE series_id = ?` | `issues_series_id_idx` | hit=3 | 0.05 ms |
| `SELECT COUNT(*) FROM issues WHERE series_id = ? AND slug = ?` | `issues_series_slug_uniq` (Index Only Scan, no heap fetches) | hit=3 | 0.11 ms |
| `SELECT * FROM issues WHERE file_path = ?` | `issues_file_path_idx` | hit=3 | 0.09 ms |

All sub-millisecond, all index hits, all `Buffers: shared hit=3`.
**No missing indexes** in the hot path.

## Baseline — incremental rescan (force=false, no on-disk changes)

`POST /libraries/stress/scan?force=false` immediately after the cold scan:

| Metric | Before | After |
|---|---:|---:|
| Wall time | 462 ms | **502 ms** |
| Folders skipped (mtime fast-path) | 60 / 60 | 60 / 60 |
| Files seen / added / updated | 0 / 0 / 0 | 0 / 0 / 0 |

Essentially unchanged — the folder mtime fast-path was already optimal,
and the F-fixes don't touch this code path. The 40 ms variance is run-to-run
noise.

## Baseline — force rescan (force=true, all rows already exist)

This is the **case where F-10 (junction-set diff) shines**: every
issue row exists, every junction set already matches what ComicInfo
carries, so the DELETE+INSERT churn can be skipped entirely.

| Metric | Estimated before* | After (with F-10) | Δ |
|---|---:|---:|---:|
| Wall time | ~14,800 ms | **13,443 ms** | **-9.2 %** |
| Files seen / updated | 1395 / 1395 | 1395 / 1395 | — |
| Throughput | ~94 files/s | 104 files/s | +10 % |

*The "before" wall is estimated at cold-scan parity; we never
measured a force-rescan baseline pre-fix because the pre-fix code's
DELETE+INSERT cost was identical between cold and warm-row force
scans.

Top queries on the after force-rescan: per-row `UPDATE issues …` (594
ms) dominates as expected (refreshing stickiness fields, re-stamping
timestamps), with the F-10 short-circuit visible as a small set of
junction-fetch SELECTs (~25 ms summed) but **no** DELETE rows on the
junction tables when the set hasn't changed.

**Phase 1-5 ship summary**: cold scan -1.8 %, force rescan -9.2 %,
incremental rescan unchanged. The IO-bound conclusion in F-9 still
holds for cold scans; the DB-side wins are mostly visible on warm
data.

## Findings — re-ranked for the real-library workload

Each finding is sized against the wall ceiling — not the
parallelizable summed work. Real-data ranking shifts dramatically
from the synthetic ranking because the hot path is now IO, not DB.

### F-9 (NEW, top priority) — Scanner is IO-bound; CPU optimizations don't move the needle

The flamegraph shows **85% of CPU samples are in syscall read paths**.
At 4.7 GB/s effective throughput on this host, the scan is ~67 GB ÷
4.7 GB/s ≈ 14 s — which matches the observed 15 s elapsed almost
exactly. Code-side fixes (F-1, F-2, F-3, F-5) collectively might trim
~50 ms wall against a 15-second baseline, < 0.5 % impact.

What actually moves the needle on cold scans of real libraries:

1. **Tune `COMIC_SCAN_WORKER_COUNT`**. Default is `min(cpu, 4)`. On
   modern dev hosts (8+ cores) and Gen4/Gen5 NVMe with `hdparm` IO
   ceilings well above 4.7 GB/s, raising to 8 or 12 may saturate
   storage further. **This is the most impactful single knob.**
2. **Tune `COMIC_SCAN_HASH_BUFFER_KB`**. Default is 64. Larger buffers
   amortize syscall overhead per archive. The flamegraph shows ~33 %
   of samples in `page_cache_ra` / `folio_alloc_noprof` — exactly the
   path a bigger buffer reduces. Worth a 256 KB / 1 MB sweep.
3. **`posix_fadvise(SEQUENTIAL)` on the open archive fd** before
   reading. Hints the kernel to prefetch larger windows; reduces the
   readahead-allocation cost (16.5 % `kernel_init_pages` self-time
   would shrink).
4. **Skip whole-archive hashing when both size+mtime match across a
   batch** (the per-file fast-path already does this on rescans;
   cold scans on a fresh DB hash everything because there's no row
   to match). For an existing-library re-import (same data, fresh
   DB), a hash cache would help — but that's a niche case.

Code change estimates for (1)+(2)+(3): ~30–50 % wall reduction on
cold scans of real libraries. Concrete numbers require a follow-up
sweep.

### F-2 ✅ SHIPPED — Slug allocator's N+1 of COUNT(*)s

[`IssueSlugAllocator::is_taken` at slug.rs:181-189](../../crates/server/src/slug.rs#L181-L189)
issues one `SELECT COUNT(*) FROM issues WHERE series_id = ? AND
slug = ?` per candidate slug per issue. With 1395 issues, 1399 calls,
0.17 ms mean → 238 ms total summed.

On the synthetic baseline I sized this small. On real data with long
runs (Walking Dead 193 issues, Invincible 145), the **collision-loop
fires more often** as the slug "1" / "2" / "3" pattern recurs across
series and per-issue suffixes pile up. Mean time climbed from 0.05 ms
(synthetic) to 0.17 ms (real) — a 3× increase from the same shape of
query, suggesting the allocator's loop is actually iterating now, not
just doing one base-name hit.

**Fix**: pre-fetch all existing slugs per series at the start of folder
processing (`SELECT slug FROM issues WHERE series_id = ?`) and use a
`HashSet<String>` for the collision check. Or drop the pre-check and
rely on the `issues_series_slug_uniq` constraint, catching the
conflict to disambiguate.

**Estimated impact**: ~60 ms wall (4 workers) ≈ 0.4 % of wall — small
in raw numbers but cleanest fix to land first because the path is
clearly N+1 and the win is mechanical.

**Shipped 2026-05-09**: pre-fetch via
[`fetch_issue_slugs_for_series`](../../crates/server/src/slug.rs)
into a HashSet before the per-archive loop, then allocate via
[`allocate_issue_slug_in_set`](../../crates/server/src/slug.rs).
The COUNT(*) query disappeared from `pg_stat_statements` after the
fix (was 1399 calls / 238 ms before; 0 after). `identity` phase
dropped from 853 ms summed to 477 ms — ~44 % drop.

### F-1 ✅ SHIPPED — `metadata_rollup` re-fetches every row by id

[`metadata_rollup.rs:303`](../../crates/server/src/library/scanner/metadata_rollup.rs#L303)
does a `find_by_id` after each insert/update. On real data: ~1395
calls × 0.16 ms = ~225 ms total ÷ 4 workers ≈ 56 ms wall. Was 1 % of
synthetic; now 0.4 % of real.

Still worth fixing for code cleanliness — the helper signature can
take `&issue::Model` directly. Demoted in priority but should still
land.

**Shipped 2026-05-09**: added
[`replace_issue_metadata_from_model`](../../crates/server/src/library/scanner/metadata_rollup.rs)
which takes the just-inserted/updated `&issue::Model` directly. The
two scanner call sites (insert + update branches in
[`process.rs`](../../crates/server/src/library/scanner/process.rs))
capture `am.insert(db).await?` / `am.update(db).await?` return value
and pass it. Saved ~1395 SELECTs per cold scan; the `SELECT issue
WHERE id = ?` query dropped from the top-15 list.

### F-3 (demoted) — `lib` re-fetched per archive

`SELECT * FROM libraries WHERE id = ?` fires ~1400 times. ~92 ms
total ÷ 4 workers = 23 ms wall = 0.15 % of real. Same fix as before
(grep the call site, thread the existing `&library::Model` through).
Cosmetic at this scale.

### F-10 ✅ SHIPPED — Junction-table churn from rich ComicInfo

Real ComicInfo carries 5–10 credit rows per issue (Writer, Penciller,
Inker, Colorist, Letterer, CoverArtist, Editor, Translator). The
scanner pattern is "DELETE FROM issue_credits WHERE issue_id = ?
then re-INSERT with the new set" via
`replace_issue_metadata_from_row`. On 1395 issues that's ~7000
INSERT-credit rows + 1395 DELETEs.

pg_stat_statements shows ~10 different `INSERT INTO issue_credits
(...) VALUES (...), ..., (...)` lines because sea-orm groups inserts
by row-count and each tuple-count gets its own normalized statement.
Combined total: ~600 ms summed ≈ 150 ms wall = 1 % of cold scan.

Same shape applies to `issue_genres`, `issue_tags`. Together they
account for ~250 ms wall.

**Fix**: skip the DELETE-then-INSERT cycle when the row's existing
junction set already matches the new one. Common case on rescans
where ComicInfo hasn't changed; less common on cold scans (which is
where this finding showed up). Modest impact, modest complexity.

**Shipped 2026-05-09**:
[`replace_issue_metadata`](../../crates/server/src/library/scanner/metadata_rollup.rs)
now fetches the existing junction set per junction table (one SELECT
each), compares against the desired set with a `HashSet` equality
check, and skips both DELETE and INSERT when they match. **This is
where the bigger wins are**: force-rescan wall dropped 9.2 % (14.8 s
→ 13.4 s), and the `SELECT FOR KEY SHARE` PG-internal FK-lock
acquisitions dropped 74 % (472 ms → 128 ms) on cold scans because
fewer junction-table writes mean fewer parent-row lock acquisitions.

### F-4 (unchanged) — Post-scan thumbnail worker UPDATEs are per-issue

`UPDATE issues SET … thumbnails_* …` 1395 calls / 262 ms total. Off
the scan critical path entirely (the `scan.completed` event fired
at 15 s; thumbnail completions roll in over the following minutes
on 2 workers). Not visible in the flamegraph because the post-scan
worker runs after the perf-record window closed.

Could batch into 50-100 ID UPDATEs per chunk. Doesn't speed up the
"scan complete" wall clock; does free DB buffer cache for concurrent
reads from the UI.

### F-5 (unchanged) — `find_by_id` for dedupe-by-content runs even on hash-unique inserts

Same impact as before: 1 of the 3 SELECTs per archive comes from
[`process.rs:312`](../../crates/server/src/library/scanner/process.rs#L312)
checking for a content-hash collision before INSERT. On a fresh DB
with all-unique hashes, the check always returns None.

Could be elided when (a) the file_path manifest had no row at this
path AND (b) we're confident no in-flight worker has staged this hash.
Real-data impact: ~80 ms wall, 0.5 %. Same priority as F-1.

### F-6 ✅ SHIPPED — Phase timings are summed across workers, hiding wall

`stats.record_phase` accumulates parallel workers into the same
counter. `db_write: 44143ms` for a 15-second wall is the most
extreme example of this confusion. Recommend tracking parallel vs
serial phases separately in the JSON, or computing per-phase wall
delta independently. No code-perf impact; large doc-clarity impact.

**Shipped 2026-05-09**:
[`ScanStats`](../../crates/server/src/library/scanner/stats.rs) now
splits into `phase_timings_ms` (serial, wall) and
`parallel_phase_timings_ms` (summed across N workers), with a
`parallel_workers` count so doc readers can derive wall ≈ summed/N.
The phase breakdown table above uses the new shape.

### F-7 (subsumed by F-9) — Per-archive work is intrinsic

Now refined: per-archive work is **IO-intrinsic**, not CPU-intrinsic.
Hashing 67 GB at 4.7 GB/s on a Gen3+ NVMe with 4 workers is the
ceiling. Faster machines + more workers + larger buffer + fadvise
hints are the only paths to faster cold scans. F-9 captures this.

### F-8 (resolved) — Flamegraph captured

`perf` installed and `kernel.perf_event_paranoid=1` for the run.
Flamegraph at `perf-out/flame-real-<TS>.svg`; the IO-bound finding
in F-9 is the headline result.

### F-11 (deferred) — Bulk INSERT issues + issue_paths per batch

Idea: collect all `IssueAM` and `issue_paths` row data per
`scan_batch_size` chunk, then do one `IssueEntity::insert_many` and
one multi-row `INSERT INTO issue_paths VALUES (...), (...)` at
end-of-batch instead of N per-archive INSERT statements.

**Estimated impact**: ~2 % wall on cold scans (~300 ms savings on a
15 s wall) — comparable to what F-1, F-2, F-12 each individually
shipped. The bottleneck after the F-1/2/10/12 changes is the
intrinsic per-row INSERT cost; bulk-INSERT-ing them only changes the
SQL packaging, not the underlying work.

**Why deferred**: substantial refactor (split
`ingest_one_with_fingerprint` into stage + commit phases, restructure
the dedupe-by-content check to operate on a staged Vec rather than
the live txn, audit `ActiveModelBehavior` hook semantics for
`insert_many`). Against an IO-bound ceiling (F-9) and a 2 % expected
gain, the risk/reward isn't right. Land if/when F-9 (worker count +
buffer + fadvise tuning) cuts the IO ceiling enough that DB-side
optimizations become first-order again.

### F-12 ✅ SHIPPED — Per-tx `synchronous_commit = OFF`

Per-batch transactions in `process_planned_folder` now run
`SET LOCAL synchronous_commit = OFF` so commits are acked before WAL
fsync. Trade-off: a server crash mid-scan loses up to ~200 ms of
WAL — but the scanner is idempotent and `scan_runs.state` is the
durable signal of completion, so the lost batch re-runs on the next
scan. Scoped to the txn via `LOCAL`; no impact on other connections.

Measurable contribution: bundled with F-1/2/10 in the 1.8 % cold-scan
improvement; not separately benchmarked.

## Follow-up plans (post-2026-05-09 ship)

Priority order, real-data weighted:

1. **F-9 — Worker count + hash buffer + fadvise sweep**. Highest
   remaining wall-clock impact. Needs a small benchmark harness to
   test `(workers ∈ {2,4,8,12}) × (buffer_kb ∈ {64, 256, 1024})` on
   a stable workload and graph throughput. The fadvise hint is a
   one-line code change to test independently. Now that F-6 is in
   place, the parallel/serial split makes the sweep results easy to
   read.
2. **F-11 — Bulk INSERT issues + issue_paths**. Deferred at ship
   time (~2 % expected wall, refactor risk). Reconsider after F-9
   if/when the IO ceiling moves and DB-side cost becomes
   first-order again.
3. **F-3 — `lib` re-fetch elimination**. Tiny win, mechanical.
4. **F-4 — Thumbnail batch UPDATEs**. Off the critical path; only
   relevant if the UI's "thumbnailing" indicator stalls become a
   support burden.
5. **F-5 — Dedupe-by-content elision**. Tiny win.

Shipped in the 2026-05-09 DB-write throughput plan: **F-1, F-2, F-6,
F-10, F-12**. Combined cold-scan wall improvement: -1.8 %; force-rescan
improvement: -9.2 %.

## Repro footer

This baseline is **host-and-data specific** — the 67 GB of CBZs in
`fixtures/library-stress/` are owner-curated and gitignored. The
methodology applies to any real library; absolute numbers will differ.

Re-running:

```sh
# 1. Fresh services
just dev-services-up
docker compose -f compose.dev.yml restart postgres   # if config changed
PGPASSWORD=comic psql -h 127.0.0.1 -p 5432 -U comic -d comic_reader \
  -c 'CREATE EXTENSION IF NOT EXISTS pg_stat_statements;'

# 2. (optional) Drop the existing Stress library for a clean cold scan
PGPASSWORD=comic psql -h 127.0.0.1 -p 5432 -U comic -d comic_reader \
  -c 'DELETE FROM libraries WHERE slug = $$stress$$;'
redis-cli -p 6380 FLUSHALL

# 3. Seed (creates Stress library, runs initial scan if folder exists)
just seed-fixtures-stress

# 4. Profile
just perf-scan                  # cold-style force=true (under flamegraph)
just perf-scan force=false      # incremental rescan

# 5. Inspect outputs
ls perf-out/
```

For an EXPLAIN on a specific hot query, use the postgres MCP or:

```sh
PGPASSWORD=comic psql -h 127.0.0.1 -p 5432 -U comic -d comic_reader -c \
  "EXPLAIN (ANALYZE, BUFFERS) SELECT … your query …"
```

To wipe `pg_stat_statements` between runs:

```sh
PGPASSWORD=comic psql -h 127.0.0.1 -p 5432 -U comic -d comic_reader -c \
  'SELECT pg_stat_statements_reset();'
```

Update this doc's baseline numbers when any of F-1..F-10 lands so the
"before" values stay anchored to a known-good measurement.
