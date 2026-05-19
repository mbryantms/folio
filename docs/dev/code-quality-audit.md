# Folio Code Quality & Technical Debt Audit — 2026-05-18

Synthesized from six parallel investigations covering Rust suppressions,
panic-surface (unwrap/expect/panic), file & function size, web TypeScript
type-safety, concurrency / async / ownership, and code duplication / dead-code
/ test gaps. The most actionable claims were verified against the code
before finalizing.

**Headline.** The codebase is in good shape. Strict-mode TypeScript is
unusually clean (zero `@ts-ignore`, four `as any` total, all on a
known dnd-kit limitation), there is **no panic-keyword in production**
(no `panic!`, `todo!`, `unimplemented!`, or `unreachable!` outside
test modules), there is **no commented-out code** in either tree, and
there are **zero stale TODOs** under `web/`. The debt is concentrated
in two patterns: (a) the `error()` helper duplicated 38 times across
API modules, and (b) a small cluster of oversized handler / scanner /
ingest functions in the 200–700-line range that share a "context tuple"
that wants to be a struct. Concurrency posture is sound, with one real
gap (apalis monitor lifecycle).

---

## Executive summary — top cross-cutting concerns

1. **`fn error(...)` envelope helper duplicated in 38 API modules.**
   ~220 LOC mechanical deletion. The canonical copy is in
   [crates/server/src/api/libraries.rs:937](../../crates/server/src/api/libraries.rs#L937);
   every other API file re-declares the same body. **[Refactor #1.]**
2. **`fn seed_issue` / `seed_series` / `seed_library` re-declared
   across 14–20 integration-test files with diverged signatures.**
   `tests/common/` exists but only exports `TestApp::spawn` — never
   grew DB fixtures. ~400–500 LOC reduction possible. **[Refactor #2.]**
3. **Twelve of 17 production `#[allow(clippy::too_many_arguments)]`
   are on three pipelines (scanner ingest, OPDS rendering, reading-
   sessions stats).** Each pipeline threads the same context tuple;
   one `Ctx` struct per pipeline closes the suppressions and improves
   testability. **[Refactor #3.]**
4. **A handful of `&{0,1}-line `HeaderValue::from_str(...).unwrap()`
   sites can panic on adversarial filenames** (CR/LF in a CBZ leaf
   name reaches the OPDS `Content-Disposition` builder). Currently
   the codebase's only realistic panic vector in a request thread.
   **[Refactor #4.]**
5. **Apalis worker monitor has no SIGTERM bridge and no panic restart.**
   The supervisor task is spawned detached, `expect`s on monitor
   error, and is dropped without join during shutdown. Real risk
   under container orchestration; bounded blast radius (job loss,
   not data corruption). **[Refactor #5.]**

The remaining findings are quality-of-life or "monitor only".

---

## Section 1 — Rust suppressions

Source: `rg '#\[allow\(' crates/`, `cargo build` warnings, manual
review of `Cargo.toml` lint config.

### 1.1 Lint configuration posture

- `Cargo.toml:23-44` workspace lints are sane. `unsafe_code = "warn"`
  is documented to tighten to `deny` once three local sites are
  cleaned. Three pragmatic relaxations (`module_name_repetitions`,
  `must_use_candidate`, `missing_errors_doc`, `missing_panics_doc`)
  are standard for an application crate. **No action.**
- `clippy.toml` sets MSRV and edition; the `disallowed-methods` list
  is empty. No loosening rules.
- `rustfmt.toml` is style-only.
- All five workspace members inherit lints via `[lints] workspace = true`.

### 1.2 `#[allow(...)]` inventory — production code (27 sites)

**Justified** (12 sites; rationale comment present): the three
`unsafe_code` sites in
[crates/server/src/main.rs:45](../../crates/server/src/main.rs#L45),
[crates/migration/src/main.rs:12](../../crates/migration/src/main.rs#L12),
[crates/server/src/library/hash.rs:52](../../crates/server/src/library/hash.rs#L52);
the two `print_stdout` / `print_stderr` allows in `main.rs:61, 105`;
the `large_enum_variant` allows in
[scanner/process.rs:77](../../crates/server/src/library/scanner/process.rs#L77)
and [next_up.rs:327](../../crates/server/src/api/next_up.rs#L327);
the migration `enum_variant_names` allow at
[m20260201_000002_series_issues.rs:14](../../crates/migration/src/m20260201_000002_series_issues.rs#L14);
the `dead_code` allows on `MockSender::clear`
([email/mod.rs:152](../../crates/server/src/email/mod.rs#L152))
and `CreditRow.cnt`
([series.rs:1858](../../crates/server/src/api/series.rs#L1858)).

**Debt** (12 sites; suppression hides a refactor):

| Location | Suppression | Why it's debt |
|---|---|---|
| [auth/local.rs:1392](../../crates/server/src/auth/local.rs#L1392) | `too_many_arguments` (8) on `finalize_session` | Hot auth path; `(success_status, format, redirect_to, …)` → `ResponseShape` struct |
| [library/scanner/process.rs:141](../../crates/server/src/library/scanner/process.rs#L141) | `too_many_arguments` (10) on `ingest_one` | Threads `(state, lib, stats, health, slug_set, force, …)` |
| [library/scanner/process.rs:183](../../crates/server/src/library/scanner/process.rs#L183) | `too_many_arguments` (12) on `ingest_one_with_fingerprint` | Same context as above; one `IngestCtx` closes both |
| [library/scanner/mod.rs:311](../../crates/server/src/library/scanner/mod.rs#L311) | `too_many_arguments` on `scan_series_folder` | Public entry point; struct-ify stabilizes API |
| [library/scanner/mod.rs:822](../../crates/server/src/library/scanner/mod.rs#L822) | `too_many_arguments` on `run_series_phases` | Mirrors `run_phases` |
| [library/scanner/mod.rs:1516](../../crates/server/src/library/scanner/mod.rs#L1516) | `too_many_arguments` on `process_planned_folder` | Internal pipeline stage |
| [api/reading_sessions.rs:1263](../../crates/server/src/api/reading_sessions.rs#L1263) | `too_many_arguments` on `top_column_with_alias` | `StatsFilter` struct shared with sibling at :1517 |
| [api/reading_sessions.rs:1517](../../crates/server/src/api/reading_sessions.rs#L1517) | `too_many_arguments` on `compute_reread_top_issues` | (paired with above) |
| [api/opds.rs:1448](../../crates/server/src/api/opds.rs#L1448) | `too_many_arguments` on `build_acquisition_feed` | Duplicated in opds_v2.rs |
| [api/opds.rs:1537](../../crates/server/src/api/opds.rs#L1537) | `too_many_arguments` on `render_issue_acq_entry` | One `FeedRenderCtx` covers all three |
| [api/opds_v2.rs:1680](../../crates/server/src/api/opds_v2.rs#L1680) | `too_many_arguments` on `publication_for` | (paired with above) |
| [api/markers.rs:258,402,447,502,534](../../crates/server/src/api/markers.rs#L258) | five `result_large_err` allows | All on validators returning `Result<_, axum::Response>`. A `MarkerError: IntoResponse` newtype closes all five. |
| [api/cbl_lists.rs:1022](../../crates/server/src/api/cbl_lists.rs#L1022) | `dead_code` on `name_override` field | "Reserved for the future rename-on-import path"; ship it or drop |

**Review** (3 sites; probably fine but document):
[state.rs:105](../../crates/server/src/state.rs#L105) (9 args on
`AppState::new`),
[scanner/mod.rs:630](../../crates/server/src/library/scanner/mod.rs#L630)
(`emit_progress` helper), and
[library/events.rs:33](../../crates/server/src/library/events.rs#L33)
(`ScanEvent::Progress` variant size — verify the size delta before
boxing rare variants).

### 1.3 Test-code suppressions

Six `#[allow(clippy::too_many_arguments)]` on `seed_issue` helpers
across `tests/filter_options.rs:792,1069`,
`tests/stats_enrichment.rs:320`,
`tests/admin_stats_extended.rs:273`,
`tests/opds.rs:329`,
`tests/opds_progress_glyphs.rs:190`. An `IssueSeed` builder under
`tests/common/seed.rs` closes all six and slots into Refactor #2.

Seven `#[allow(dead_code)]` on shared test helpers
(`tests/thumb_cleanup.rs:269`, `tests/admin_thumbs.rs:958`,
`tests/library_delete.rs:433,436`, `tests/cbl_lists.rs:42`,
`tests/collections.rs:31`, `tests/opds_progress_advertisement.rs:33`)
are all load-bearing — integration test crates can't see cross-file
usage, so these are correct as-is.

### 1.4 `#[expect(...)]`

**Zero occurrences.** Adopting `#[expect(...)]` for the documented
allows above would let stale suppressions surface as warnings once
the underlying issue is fixed. Cheap hygiene win.

### 1.5 `#[ignore]` tests

Two, both in
[crates/server/tests/ocr_recognizer.rs:49,75](../../crates/server/tests/ocr_recognizer.rs#L49)
with explicit reasons (Tesseract toolchain, 80 MB ONNX download).
The module doc-comment explains the `--ignored` workflow. **OK.**

### 1.6 `unsafe` blocks

Three sites total, all with SAFETY comments
([main.rs:46](../../crates/server/src/main.rs#L46),
[migration/src/main.rs:13](../../crates/migration/src/main.rs#L13),
[library/hash.rs:59](../../crates/server/src/library/hash.rs#L59)).
None in `entity/`. None unsound. Once the migration's `set_var`
moves to a clap arg parser the workspace lint can flip to `deny`.

### 1.7 TODO / FIXME / HACK

Filtering CSP nonce literals leaves two real markers:

- [jobs/post_scan.rs:818](../../crates/server/src/jobs/post_scan.rs#L818)
  — `"post_scan_dictionary (TODO: 'did you mean' trigram refresh)"`.
  This handler is a registered cron that logs and returns `Ok` (silent
  no-op). Per MEMORY's [incompleteness_cleanup_m2_done], trigram
  suggestions were explicitly **deferred by decision** (D-3 closed
  as won't-do). The TODO is stale. **Fix:** delete the cron
  registration or rewrite to `// deferred — see D-3`.
- [scanner/reconcile_status.rs:61](../../crates/server/src/library/scanner/reconcile_status.rs#L61)
  — documented future-edge-case note. Leave.

Zero `FIXME`, `XXX`, or `HACK` markers in either tree.

---

## Section 2 — Panic surface (`unwrap` / `expect` / `panic!`)

Source: `rg '\.unwrap\(\)|\.expect\(|panic!\(|todo!\(|unimplemented!\(|unreachable!\(' crates/`.

### 2.1 Headline

- **Zero `panic!` / `todo!` / `unimplemented!` / `unreachable!` in
  production code.** All matches are inside `#[cfg(test)]` blocks.
- **Zero `assert!` / `debug_assert!`** outside test code.
- `~3200` test-side `.unwrap()` calls (all OK by convention).
- **`.unwrap()` in production handlers: ~13 sites**, all on
  `HeaderValue::from_str(...)`. See §2.2.
- **`.expect()` in production: ~30 sites**, every one with a
  message explaining the invariant.

This is unusually clean for a codebase this size. The error-handling
convention (manual `match` ladder + `return error(...)`) is preserved
in every handler reviewed.

### 2.2 Production-path unwraps that can panic on adversarial input

The only real panic surface in a request thread:

| Location | Risk |
|---|---|
| [api/opds.rs:1373](../../crates/server/src/api/opds.rs#L1373) | `HeaderValue::from_str(&format!("attachment; filename=\"{leaf}\""))` — `leaf` is a filesystem leaf name; a CR/LF in a malicious CBZ filename panics the OPDS download task |
| [api/opds.rs:1397](../../crates/server/src/api/opds.rs#L1397) | Same pattern in the 416 fallback branch |
| [api/opds.rs:1379,1387](../../crates/server/src/api/opds.rs#L1379) | `Content-Range` header builder; inputs are server-computed integers — unreachable in practice |
| [api/page_bytes.rs:183](../../crates/server/src/api/page_bytes.rs#L183) | `inline; filename="page-{n}.{ext}"`; bounded inputs but same pattern |
| [api/opds_pse.rs:256,261,271,280,400](../../crates/server/src/api/opds_pse.rs#L256) | Same family of header builders |
| [api/thumbnails.rs:231](../../crates/server/src/api/thumbnails.rs#L231) | `HeaderValue::from_str(&etag)` — hex ETag, OK in practice |

**Fix (single small PR).** Replace all `HeaderValue::from_str(...).unwrap()`
with `.unwrap_or(HeaderValue::from_static("…"))`. Sanitize `leaf`
before substitution (drop CR/LF). Closes the only realistic in-handler
panic vector. **[Refactor #4.]**

### 2.3 `.expect()` audit

Every prod `.expect()` was reviewed and carries a justifying message
or invariant. The one worth singling out:

- [jobs/mod.rs:205](../../crates/server/src/jobs/mod.rs#L205) —
  `monitor.run().await.expect("apalis monitor crashed")` is the only
  non-startup `expect` whose blast radius is the worker process.
  See §3 below.

Other `expect`s on lock-poisoning, infallible chrono / argon2 / HMAC
ops, or startup invariants are correct.

### 2.4 Error-handling style

The codebase chose **per-handler `match` ladders** over a generic
`AppError: IntoResponse` type. Spot-checks across
`saved_views.rs`, `series.rs`, `cbl_lists.rs`, `collections.rs`,
`next_up.rs` confirm consistent shape:

```rust
match thing.await {
    Ok(v) => v,
    Err(e) => { tracing::error!(error = %e, …); return error(StatusCode::INTERNAL_SERVER_ERROR, …); }
}
```

`?` propagation appears only in `anyhow::Result`-typed library and
scanner code (config loader, thumbnails, scanner phase plumbing) —
cleanly separated from the handler layer. The two styles do not
cross over. The convention is the cost of the dupe in §4.1 below.

---

## Section 3 — Concurrency, async, ownership

### 3.1 Blocking I/O on async paths

- [library/scanner/validate.rs:39,46,53,63](../../crates/server/src/library/scanner/validate.rs#L39)
  — synchronous `std::fs::canonicalize` / `read_dir` in `async fn validate_library_root`, called from request handlers. Small ops but
  should be in `spawn_blocking`.
- [library/scanner/mod.rs:340,342](../../crates/server/src/library/scanner/mod.rs#L340)
  — same `canonicalize` smell in the async planner.
- [library/scanner/process.rs:569](../../crates/server/src/library/scanner/process.rs#L569)
  — `std::fs::remove_dir_all(&strip_dir)` inline in async upsert
  path; blocks per issue update.

All other `std::fs` calls are inside `spawn_blocking`. No `std::sync::Mutex`
held across an `.await` anywhere in the tree (verified by grep + manual
inspection of every `Mutex<_>` field in `state.rs` and `library/`).

### 3.2 `spawn_blocking` coverage — good

17 sites; CPU-bound paths (BLAKE3, archive parse, image decode,
thumbnail encode, ONNX inference, Tesseract) are consistently
routed through `spawn_blocking` and gated by
`state.archive_work_semaphore`. **No action.**

### 3.3 Detached `tokio::spawn` — fine

All 5 detached spawns (apalis monitor, WS upstream + bridge, fire-and-
forget PSE progress writes, CBL rematch, deep-validate background) log
on error. No silent task drops.

### 3.4 Apalis worker lifecycle — gap

[app.rs:609](../../crates/server/src/app.rs#L609) spawns the apalis
monitor detached. `app.rs:630` `with_graceful_shutdown(shutdown_signal())`
drains HTTP, but the apalis monitor is **not** signaled or joined —
it's dropped when tokio tears down. In-flight jobs may abort
mid-write. There is also no panic-restart: the monitor's
`.expect("apalis monitor crashed")` lives inside the detached spawn
where the runtime swallows it.

**Risk:** real under k8s/container orchestration. Bounded blast radius
(job loss, not data corruption — per-job panic isolation is handled
correctly in `post_scan.rs:296-308`).

**Fix:** add SIGTERM plumbing into apalis via a `CancellationToken`
shared with `shutdown_signal`. Optionally retain `JoinHandle` and
restart on `Err`. **[Refactor #5.]**

### 3.5 Lock layout

Every `Arc<Mutex<_>>` / `Arc<RwLock<_>>` in `state.rs` and
`library/events.rs` was reviewed. All holds are sub-millisecond and
correctly scoped. Two ergonomic improvements possible but not bugs:

- `state.rs:79,82` — `thumb_job_inflight` and `thumb_path_cache`
  could be `DashSet`/`DashMap` to drop per-call `.await`s. Pure
  ergonomics.
- `state.rs:57` — `email: std::sync::Mutex<Arc<dyn EmailSender>>` is
  documented "never across an await"; an `ArcSwap<dyn EmailSender>`
  would express that as a type. Note: `ArcSwap` can't hold unsized
  `dyn` directly; needs `ArcSwap<Arc<dyn EmailSender>>` which is the
  documented current shape, so this is non-trivial.

### 3.6 Clone hygiene

789 `.clone()` calls. Most are unavoidable (sea-orm `Set(...)` consumes
owned values; `tokio::spawn` requires `'static`). Worst plausibly-
avoidable hot-spots:

- [jobs/post_scan.rs:161,166,276,289,302](../../crates/server/src/jobs/post_scan.rs#L161)
  — `row.id.clone()` five times in one job
- [cbl/matcher.rs:116,122,171,177,242](../../crates/server/src/cbl/matcher.rs#L116)
  — `issue_id.clone()` inside tight per-entry match loop
- [library/scanner/mod.rs:1193-1194,742,752,757](../../crates/server/src/library/scanner/mod.rs#L1193)
  — `IgnoreRules` hand-cloned per planned folder; one `Arc<IgnoreRules>`
  would suffice
- [api/admin_thumbs.rs:1269](../../crates/server/src/api/admin_thumbs.rs#L1269)
  — `data_dir.clone()` per chunk worker; `Arc<PathBuf>` saves N clones

The codebase is **clone-first by convention** (sea-orm `String` IDs +
`'static` spawn boundaries make borrowing expensive to thread). A
focused pass on `issue_id` / `series_id` / `file_path` borrowing in
`process.rs` + `cbl/matcher.rs` would shave thousands of allocations
off a full library scan, but it's a touch-everywhere refactor —
lower ROI than items 1–5.

### 3.7 WebSocket backpressure

[api/ws_scan_events.rs:139-149](../../crates/server/src/api/ws_scan_events.rs#L139)
handles `broadcast::RecvError::Lagged` correctly (emits a `lagged` ping
with count and continues). `socket.send(...).await` has no write
timeout — a half-dead TCP peer can stall the task until OS keepalive
fires. **Minor; monitor only.**

### 3.8 OCR singletons

`OnceCell` + `Mutex<ComicTextDetector>` + per-language
`Mutex<Recognizer>` serializes all OCR through three locks. Correct
at low QPS; would need a pool to scale. Not a current concern (OCR is
queued through apalis).

---

## Section 4 — Code duplication

### 4.1 `fn error(...)` envelope helper — 38 copies

The canonical implementation in
[crates/server/src/api/libraries.rs:937](../../crates/server/src/api/libraries.rs#L937)
is re-declared verbatim in 37 sibling files (`account.rs`,
`markers.rs`, `series.rs`, `issues.rs`, `opds.rs`, `cbl_lists.rs`,
`saved_views.rs`, `reading_sessions.rs`, …). Partial siblings:
`fn not_found()` in three files, `fn server_error<E>()` in two,
`fn unauthorized()` in one.

**Fix:** promote to `crates/server/src/api/mod.rs` as
`pub(crate) fn error(...)`. Bulk-delete the 37 copies. ~220 LOC
mechanical refactor; identical bodies → low risk.
**[Refactor #1.]**

Also notable: `fn capitalize` is identical in
[api/opds.rs:550](../../crates/server/src/api/opds.rs#L550) and
[api/opds_v2.rs:513](../../crates/server/src/api/opds_v2.rs#L513).
Trivial; ride along with the OPDS refactor.

### 4.2 Test fixtures — `seed_*` helpers

`fn seed_issue` is hand-redeclared in **20 test files**, `seed_series`
in **14**, `seed_library` in **15**. Signatures **diverge** between
files (e.g. `markers.rs:114` returns `(Uuid, Uuid, String)` while
`people.rs:200` returns `String`). The cost is real: modifying the
issue schema requires touching ~20 fixtures.

The `tests/common/` module already exists; it just never grew DB
fixtures beyond `TestApp::spawn`.

**Fix:** add `tests/common/seed.rs` exporting an `IssueSeed { id,
series_id, slug }` struct and corresponding `SeriesSeed` /
`LibrarySeed`. Conservative estimate: **~400–500 LOC removed**.
Risk: medium (a passing integration test must keep passing); recommend
two PRs — issue helpers first, then series + library.
**[Refactor #2.]**

### 4.3 OPDS v1 ↔ v2 drift

`opds.rs` (3484 LOC) and `opds_v2.rs` (2088 LOC) already share helpers
via `pub(crate)`: ~50 cross-module calls. The remaining duplication
is intrinsic (Atom XML vs JSON-LD serialization). **Monitor only**;
not refactorable without a third format-agnostic IR.

### 4.4 Other reviewed-not-duplicated

Cover rendering is already factored to `web/components/Cover.tsx`
(imported by 6 card variants). No action.

---

## Section 5 — File and function size

### 5.1 Largest Rust files

| File | Lines | Notes |
|---|---|---|
| [api/opds.rs](../../crates/server/src/api/opds.rs) | 3484 | 47 fns — routes + renderers + helpers |
| [api/series.rs](../../crates/server/src/api/series.rs) | 2470 | 22 pub items; `update_series` is 677 lines |
| [api/opds_v2.rs](../../crates/server/src/api/opds_v2.rs) | 2088 | Mirrors `opds.rs` shape |
| [api/cbl_lists.rs](../../crates/server/src/api/cbl_lists.rs) | 2075 | **45 pub items** — sprawl |
| [api/reading_sessions.rs](../../crates/server/src/api/reading_sessions.rs) | 1947 | Stats compute co-located with routes |
| [api/saved_views.rs](../../crates/server/src/api/saved_views.rs) | 1885 | **34 pub items** |
| [api/issues.rs](../../crates/server/src/api/issues.rs) | 1882 | `list`+`bulk_metadata`+`update` are massive |
| [library/scanner/mod.rs](../../crates/server/src/library/scanner/mod.rs) | 1810 | `run_phases` 320 lines |
| [auth/local.rs](../../crates/server/src/auth/local.rs) | 1530 | `update_preferences` 225 lines |
| [library/scanner/process.rs](../../crates/server/src/library/scanner/process.rs) | 1501 | `ingest_one_with_fingerprint` ~491 lines |

### 5.2 Largest functions

Five functions exceed 400 lines and warrant breakup:

| Location | Function | ~Lines |
|---|---|---|
| [api/series.rs:225](../../crates/server/src/api/series.rs#L225) | `update_series` | 677 |
| [api/series.rs:958](../../crates/server/src/api/series.rs#L958) | `list` | 619 |
| [library/scanner/process.rs:184](../../crates/server/src/library/scanner/process.rs#L184) | `ingest_one_with_fingerprint` | 491 |
| [api/reading_sessions.rs:763](../../crates/server/src/api/reading_sessions.rs#L763) | `compute_stats_for_user` | 469 |
| [api/issues.rs:1245](../../crates/server/src/api/issues.rs#L1245) | `list` | 427 |

### 5.3 Largest TypeScript files

| File | Lines | Notes |
|---|---|---|
| `web/lib/api/types.generated.ts` | 10396 | Codegen — ignore |
| [web/lib/api/mutations.ts](../../web/lib/api/mutations.ts) | 2319 | 85+ exported hooks; merge-conflict hot spot |
| [web/lib/api/types.ts](../../web/lib/api/types.ts) | 2106 | Hand-written contracts (drift mirror) |
| [web/lib/api/queries.ts](../../web/lib/api/queries.ts) | 1549 | Query keys + hooks |
| [web/components/admin/library/LiveScanProgress.tsx](../../web/components/admin/library/LiveScanProgress.tsx) | 1447 | Reducer + 17 subcomponents in one file |
| [web/app/[locale]/read/.../Reader.tsx](../../web/app/[locale]/read) | 1261 | Single `Reader` fn body ~917 lines |
| [web/components/library/LibraryGridView.tsx](../../web/components/library/LibraryGridView.tsx) | 1178 | Grid + FilterSheet + helpers |

### 5.4 Largest TypeScript components

| Location | Component | ~Lines |
|---|---|---|
| `Reader.tsx:47` | `Reader` | 917 |
| `IssueActions.tsx:278` | `EditForm` | 524 |
| `LibraryGridView.tsx:109` | `LibraryGridView` | 517 |
| `…/issues/[issueSlug]/page.tsx:50` | `IssuePage` (route) | 470 |
| `LiveScanProgress.tsx:675` | `ThumbnailWorkPanel` | 359 |
| `NavigationManager.tsx:85` | `SidebarSection` | 338 |
| `LiveScanProgress.tsx:209` | `LiveScanProgress` | 304 |

---

## Section 6 — Web TypeScript quality

Strict-mode posture is **strong**. Across ~430 source files:

- **Zero** `@ts-ignore` / `@ts-expect-error`
- **Zero** `as unknown as` double-casts
- **Four** `as any` (all on `@dnd-kit` synthetic-listener bags in
  [PagesManager.tsx:516,518](../../web/components/pages-manager/PagesManager.tsx#L516)
  and
  [NavigationManager.tsx:537,539](../../web/components/sidebar-layout/NavigationManager.tsx#L537)
  — the canonical workaround for that library's typing gap)
- **Six** member-access `!` non-null assertions; four have an obvious
  preceding guard, two
  ([PagesManager.tsx:98](../../web/components/pages-manager/PagesManager.tsx#L98),
  [EditMetadataDialog.tsx:231](../../web/components/library/EditMetadataDialog.tsx#L231))
  are worth tightening
- **29** ESLint disable comments, every one with a rationale
  comment; 17 are the deliberate `react-hooks/set-state-in-effect`
  convention for one-shot mount-only hydration
- **Zero** TODO/FIXME/HACK in TS source
- **Zero** `console.log` in production code; six `console.warn` in
  fail-open OCR/marker paths (correct)
- Three duplicate inline-typed `Record<string,string>` keybind casts
  ([use-sidebar-state.ts:50](../../web/lib/use-sidebar-state.ts#L50),
  [GlobalHotkeys.tsx:36](../../web/components/GlobalHotkeys.tsx#L36),
  [GlobalShortcutsSheet.tsx:61](../../web/components/GlobalShortcutsSheet.tsx#L61))
  — one `readMeKeybinds(me)` helper removes all three

**Two real bugs-in-waiting** (silent 401 drops):

- [lib/reader/session.ts:345](../../web/lib/reader/session.ts#L345)
  — direct `fetch("/api/me/reading-sessions", …)` bypasses the
  `apiFetch` 401-refresh-and-retry wrapper. A token expiring
  mid-reading silently drops session heartbeats.
- [Reader.tsx:724](../../web/app/[locale]/read) — direct
  `fetch("/api/progress", …)` same pattern. Token expiry silently
  drops progress writes until the next 200 wakes things up.

**Fix:** route both through `apiFetch`. Highest-correctness ROI on
the web side.

The other direct `fetch` calls (`/auth/me`, `/auth/local/*`,
`/auth/logout`) are correctly bypassing `apiFetch` because `/auth/*`
is a **bare** route (CLAUDE.md routing convention) and the auth-refresh
interceptor explicitly must not wrap them.

---

## Section 7 — Tests, gaps, ignored tests

### 7.1 Server modules with zero dedicated integration test

| Module | Coverage |
|---|---|
| `api/ratings.rs` | Only incidentally covered by `issues_edit.rs:765-806` |
| `api/csp.rs` | None |
| `api/server_info.rs` | Update-check only via `server_releases.rs` |
| `api/meta.rs` | None |
| `api/form_or_json.rs` | Exercised indirectly via `progressive_enhancement.rs` |
| `api/audit.rs` | Covered indirectly (~every mutating test asserts a row) |
| `api/opds_v2.rs` | Partial — file exists but mostly smoke-level for a 2088-line surface |

The biggest gap is **`api/ratings.rs`** — a user-visible mutation
without a dedicated test. **Action:** add `crates/server/tests/ratings.rs`
covering set / clear / range validation / audit emission. ~80 LOC.

### 7.2 Critical-path coverage gaps

| Path | Status |
|---|---|
| Scanner cancellation | ✓ `tests/scan_run_cancel.rs` |
| OPDS PSE signed-URL signature | ✓ `tests/opds_pse.rs:364,412,461` |
| OCR pipeline | partial (`ocr_recognizer.rs` `#[ignore]`; `issue_ocr.rs` covers handler) |
| CSRF rotation | Covered by `tests/auth.rs:181`; single-test only |
| **OIDC discovery refresh** | **Gap** — `clear_discovery_cache` is called by `PATCH /admin/settings` but no test asserts cache eviction on settings save |

**Action:** add an `oidc.rs` integration test that seeds OIDC settings,
populates the discovery cache, PATCHes settings, and asserts the new
issuer is used on the next request.

### 7.3 Web pages with no test

Acceptable; most pages are thin shells over tested client components.
Highest gaps: `admin/api-docs/page.tsx` (Scalar embed),
`admin/series/[slug]/page.tsx`.

---

## Refactor priority list

Ranked by **lines deleted ÷ effort × correctness payoff**.

| # | Refactor | Est LOC delta | Effort | Risk | Payoff |
|---|---|---|---|---|---|
| 1 | Extract `fn error()` to `api/mod.rs`, delete 37 copies | −220 | 30 min | Low | Convention enforced once |
| 2 | Promote `seed_*` helpers into `tests/common/seed.rs` | −400 to −500 | 2–3 h | Medium | Schema-change cost drops; closes 6 `too_many_arguments` allows |
| 3 | Introduce `IngestCtx` / `FeedRenderCtx` / `StatsFilter` structs | −80, +30 (net −50) | 2 h | Low | Closes 8 `too_many_arguments` allows; tests get simpler |
| 4 | Sanitize OPDS / page-bytes / PSE / thumbnails `HeaderValue` builders | +20 | 30 min | Low | Closes only realistic in-handler panic vector |
| 5 | Apalis monitor — SIGTERM bridge + panic restart | +60 | 4 h | Medium | Closes the only real concurrency gap |
| 6 | Route `/api/progress` + `/api/me/reading-sessions` through `apiFetch` | −20 | 1 h | Low | Closes silent-401-drop bug in reading session writes |
| 7 | Add `tests/ratings.rs` and `tests/oidc.rs` | +130 | 2 h | Low | Closes two real test gaps |
| 8 | Drop stale `post_scan_dictionary` TODO + scanner-mod doc note | −10 | 5 min | None | Memory hygiene |
| 9 | Introduce `MarkerError: IntoResponse` newtype | +20, −5 | 1 h | Low | Closes 5 `result_large_err` allows |
| 10 | Shard `web/lib/api/mutations.ts` (2319 LOC) by domain | 0 net | 2 h | Low | Merge-conflict hot spot dissolved |
| 11 | Extract `Reader.tsx` into `useReaderProgress/Prefetch/Swipe/Keymap` hooks | −300 (Reader becomes ~200) | 1 day | Medium | Each hook becomes vitest-coverable |
| 12 | Split `update_series` (677 lines) into per-section updaters | 0 net | 1 day | Medium | Each section unit-testable; audit-log diff readable |
| 13 | Break `ingest_one_with_fingerprint` (491 lines) into 4 stages | 0 net | 1 day | Medium | LFH-recovery and user-edit-merge become unit-testable |
| 14 | Adopt `#[expect(...)]` for documented allows | 0 | 1 h | None | Stale suppressions surface automatically |
| 15 | `Arc<IgnoreRules>` instead of per-folder clone in scanner | −5 | 1 h | Low | Allocation reduction during full scan |

**Recommended sequencing:** items 1, 4, 6, 8 in one quick-wins PR
(~2 hours, big LOC delta). Item 2 in its own PR (touches many test
files). Items 3, 5, 7, 9, 14 in a second sweep (~1 day). Items 11,
12, 13 are larger reshapes that should each get their own PR with
careful test coverage.

---

## Suppression decision matrix

| Suppression | Verdict | Rationale |
|---|---|---|
| `unsafe_code = "warn"` workspace lint | OK; flip to `deny` once 3 sites cleaned | Three sites have SAFETY comments |
| `#[allow(unsafe_code)]` × 3 in prod | OK | Each has SAFETY comment, narrow surface |
| `#[allow(clippy::print_stdout/stderr)]` × 2 in `main.rs` | OK | `--emit-openapi` stdout pipe; pre-tracing healthcheck stderr |
| `#[allow(clippy::large_enum_variant)]` × 2 | Mixed — `process.rs` OK; `library/events.rs` Review | Verify size delta on ScanEvent |
| `#[allow(clippy::too_many_arguments)]` × 12 in prod | **Debt** | Three pipelines × `Ctx` struct closes most |
| `#[allow(clippy::too_many_arguments)]` × 6 on test `seed_*` | **Debt** | `IssueSeed` builder closes all six |
| `#[allow(clippy::result_large_err)]` × 5 in `markers.rs` | **Debt** | `MarkerError: IntoResponse` closes all |
| `#[allow(dead_code)]` × 3 in prod | OK (2 sea-orm) / Review (1 cbl_lists) | `name_override`: ship or drop |
| `#[allow(dead_code)]` × 7 on test helpers | OK | Integration crates can't see cross-file uses |
| `#[ignore]` × 2 OCR tests | OK | Documented native-toolchain reasons |
| `#[expect(...)]` × 0 | Adopt | Cheap hygiene win on documented allows |
| ESLint `react-hooks/set-state-in-effect` × 17 disables | OK | Convention for one-shot mount-only hydration |
| ESLint `@typescript-eslint/no-explicit-any` × 4 | OK | dnd-kit listener bags |
| ESLint `@next/next/no-img-element` × 5 | OK | Rust-served signed-byte images |
| ESLint `react-hooks/exhaustive-deps` × 3 | 2 OK, 1 Review | `ReadingPrefs.tsx:334` could `useRef`-guard |

---

## Section 8 — Dependency review

Sources: `Cargo.toml` workspace.dependencies (61 declared crates),
`web/package.json` (35 outdated entries reported by `pnpm outdated`),
crates.io probes via `cargo search` for current upstream stable.

**Headline.** Rust dependency hygiene is mixed: the runtime core
(tokio / hyper / axum / serde / uuid / blake3) is one or two patch
releases behind and safe to bump in a single `cargo update`. The
auth and observability stacks have **major** versions outstanding
that warrant a planned bump (jsonwebtoken 9→10, oauth2 4→5,
openidconnect 3→4, redis 0.27→1.2, opentelemetry 0.26→0.32). One
crate (`tailwindcss`) is **on a beta** while a stable 4.3.0 is
out — that's the single most actionable item. React-Query, Playwright,
Prettier, and Vitest on the web side are similarly far behind on
patch / minor releases.

### 8.1 Rust runtime + web (low risk, recommend bump)

| Crate | Declared | Latest stable | Bump? |
|---|---|---|---|
| tokio | 1.41 | 1.52.3 | **Yes** — patch-only API; many fixes |
| tokio-util | 0.7 | 0.7.18 | Yes — already in range |
| futures | 0.3 | 0.3.32 | Yes — in range |
| async-trait | 0.1 | 0.1.89 | Yes — in range |
| axum | 0.8 | 0.8.9 | Yes — in range |
| axum-extra | 0.10 | **0.12.6** | **Review** — two minor breaks; check cookie/typed-header rename notes |
| tower | 0.5 | 0.5.3 | Yes |
| tower-http | 0.6 | 0.6.11 | Yes |
| hyper | 1.5 | 1.9.0 | Yes — in range |
| hyper-util | 0.1 | 0.1.20 | Yes |
| utoipa | 5 | 5.5.0 | Yes |
| utoipa-axum | 0.2 | 0.2.0 | At latest |

### 8.2 Rust DB stack (hold)

| Crate | Declared | Latest | Bump? |
|---|---|---|---|
| sea-orm | 1.1 | 2.0.0-rc.38 | **Hold** — pre-release; v1.1 is still receiving back-ports. Cross-cut with `sea-orm-migration`; plan the v2 jump as a single PR once 2.0.0 final ships |
| sea-orm-migration | 1.1 | 2.0.0-rc.38 | **Hold** (pairs with above) |
| sqlx | 0.8 | 0.9.0-alpha.1 | **Hold** — alpha; transitive through sea-orm |

### 8.3 Rust serialization / IDs / crypto (mostly bump)

| Crate | Declared | Latest | Bump? |
|---|---|---|---|
| serde | 1 | 1.0.228 | Yes |
| serde_json | 1 | 1.0.149 | Yes |
| serde_with | 3 | 3.20.0 | Yes — in range |
| uuid | 1.11 | 1.23.1 | Yes — in range |
| blake3 | 1.5 | 1.8.5 | Yes — in range |
| argon2 | 0.5 | 0.6.0-rc.8 | **Hold** — RC; security-critical; wait for stable |
| rand | 0.8 | 0.10.1 | **Review** — major bump; `RngCore` API shifted in 0.9 |
| hmac | 0.12 | 0.13.0 | **Review** — 0.13 minor break; ripple to argon2/sha2 stack |
| sha2 | 0.10 | 0.11.0 | **Review** (pairs with hmac) |
| chacha20poly1305 | 0.10 | 0.11.0-rc.3 | **Hold** — RC; current stable used for AEAD-sealed secret rows |
| arc-swap | 1.7 | 1.9.1 | Yes — in range |
| time | 0.3 | 0.3.47 | Yes |

### 8.4 Rust auth (planned bump)

| Crate | Declared | Latest | Bump? |
|---|---|---|---|
| jsonwebtoken | 9 | **10.4.0** | **Review** — major; JWT validation API changed. Cross-check Es256/RS256 paths |
| oauth2 | 4.4 | **5.0.0** | **Review** — major; type-state changes in `BasicClient` |
| openidconnect | 3.5 | **4.0.1** | **Review** — major; pairs with oauth2 5; discovery cache may need refactor |
| ed25519-dalek | 2.1 | 3.0.0-pre.7 | **Hold** — pre-release |
| base64 | 0.22 | 0.22.1 | Yes |
| data-encoding | 2.6 | 2.11.0 | Yes — in range |
| constant_time_eq | 0.3 | 0.5.0 | Review — minor API tightening |
| zeroize | 1.8 | 1.8.2 | Yes |

Group the three auth majors (jsonwebtoken, oauth2, openidconnect) into
one PR — they're commonly bumped together because openidconnect-rs
depends on oauth2 internally and is sensitive to its type-state version.
Run the existing `auth_hardening_m*` integration test suite as the gate.

### 8.5 Rust email (bump)

| Crate | Declared | Latest | Bump? |
|---|---|---|---|
| lettre | 0.11 | 0.11.22 | Yes — in range; 0.11 is current stable |

### 8.6 Rust scanner / archive / images (mixed)

| Crate | Declared | Latest | Bump? |
|---|---|---|---|
| zip | 8 | 9.0.0-pre2 | **Hold** — pre-release; v8 still receiving fixes |
| lru | 0.12 | **0.18.0** | **Review** — six minor versions; API likely changed |
| walkdir | 2.5 | 2.5.0 | At latest |
| tempfile | 3 | 3.27.0 | Yes |
| notify | 7.0 | 9.0.0-rc.4 | **Hold** — pre-release; pairs with notify-debouncer-full |
| notify-debouncer-full | 0.4 | 0.8.0-rc.2 | **Hold** (pairs with above) |
| natord | 1.0.9 | 1.0.9 | At latest |
| mime_guess | 2.0 | 2.0.5 | Yes |
| infer | 0.16 | 0.19.0 | **Review** — three minors; new MIME detection rules may shift fixture behavior |
| image | 0.25 | 0.25.10 | Yes |
| webp | 0.3 | 0.3.1 | Yes |
| fast_image_resize | 5 | 6.0.0 | **Review** — major; benchmark before swap |
| rayon | 1.10 | 1.12.0 | Yes — in range |
| globset | 0.4 | 0.4.18 | Yes |
| unrar | 0.5 | 0.5.8 | Yes |
| sevenz-rust | 0.6 | 0.6.1 | Yes |
| tar | 0.4 | 0.4.46 | Yes |

### 8.7 Rust OCR (hold — vendored)

| Crate | Declared | Latest | Bump? |
|---|---|---|---|
| comic-text-detector | 0.5.1 | 0.5.1 | At latest |
| manga-ocr | 0.5.1 | 0.5.1 | At latest |
| tesseract-rs | 0.2 | 0.2.0 | At latest |

Cross-crate ort 2.0.0-rc.10 pin is documented in the workspace
comment — re-probe required before any individual bump.

### 8.8 Rust jobs + cache (review)

| Crate | Declared | Latest | Bump? |
|---|---|---|---|
| apalis | 0.6 | 1.0.0-rc.9 | **Hold** — pre-release; v0.6 in use |
| apalis-redis | 0.6 | 1.0.0-rc.8 | **Hold** (pairs with above) |
| redis | 0.27 | **1.2.1** | **Review** — major version; coordinate with apalis bump |
| tokio-cron-scheduler | 0.13 | 0.15.1 | Review |

### 8.9 Rust observability (planned major)

| Crate | Declared | Latest | Bump? |
|---|---|---|---|
| tracing | 0.1 | 0.1.44 | Yes |
| tracing-subscriber | 0.3 | 0.3.23 | Yes |
| tracing-opentelemetry | 0.27 | **0.33.0** | **Review** — six minors; pairs with opentelemetry crates |
| opentelemetry | 0.26 | **0.32.0** | **Review** — six minors; coordinate all 5 OTLP crates in one PR |
| opentelemetry_sdk | 0.26 | 0.32.0 | (pairs) |
| opentelemetry-otlp | 0.26 | 0.32.0 | (pairs) |
| opentelemetry-stdout | 0.26 | 0.32.0 | (pairs) |
| metrics | 0.24 | 0.24.6 | Yes |
| metrics-exporter-prometheus | 0.16 | 0.18.3 | **Review** — two minors; Prometheus-side API tweaks |

The OTLP stack is currently **dormant** in production (per MEMORY's
`incompleteness_cleanup_m5_done`, OTLP wiring is "considered, not
chosen"). Bumping all five OTLP crates together is cheap; deferring
is also fine since none are on a hot path.

### 8.10 Rust misc / errors / config (bump)

| Crate | Declared | Latest | Bump? |
|---|---|---|---|
| thiserror | 2 | 2.0.18 | Yes |
| anyhow | 1 | 1.0.102 | Yes |
| figment | 0.10 | 0.10.19 | Yes |
| dotenvy | 0.15 | 0.15.7 | Yes |
| tower_governor | 0.8 | 0.8.0 | At latest |
| governor | 0.10 | 0.10.4 | Yes |
| once_cell | 1.20 | 1.21.4 | Yes |
| url | 2.5 | 2.5.8 | Yes |
| percent-encoding | 2.3 | 2.3.2 | Yes |
| quick-xml | 0.36 | **0.40.1** | **Review** — four minors; OPDS feed serializer; verify with `opds*` integration tests |
| slug | 0.1 | 0.1.6 | Yes |
| ipnet | 2.10 | 2.12.0 | Yes |

### 8.11 Rust test infra (bump)

| Crate | Declared | Latest | Bump? |
|---|---|---|---|
| testcontainers | 0.23 | 0.27.3 | **Review** — four minors; `TestApp::spawn` boots Postgres + Redis containers, verify image-pull behavior unchanged |
| testcontainers-modules | 0.11 | 0.15.0 | (pairs) |
| reqwest | 0.12 | 0.13.3 | **Review** — major; used in `cbl/catalog.rs`, CBL refresh, and tests |
| insta | 1.41 | 1.47.2 | Yes — in range |
| proptest | 1.5 | 1.11.0 | Yes — in range |

### 8.12 Web — major bumps available

| Package | Current | Latest | Bump? | Notes |
|---|---|---|---|---|
| `tailwindcss` + `@tailwindcss/postcss` | 4.0.0-**beta.8** | **4.3.0** | **Yes — high priority** | Currently on a beta while 4.3 stable shipped. Sole "actively on a beta channel" in the web tree |
| `react`, `react-dom` | 19.2.5 | 19.2.6 | Yes — patch |
| `next`, `eslint-config-next`, `@next/eslint-plugin-next` | 16.2.4 | 16.2.6 | Yes — patch (security fixes likely; bump as part of next CI tick) |
| `@tanstack/react-query` | 5.62.10 | 5.100.11 | **Yes** | 38 minor versions; many bug fixes, all backward-compatible per their changelog policy |
| `@tanstack/react-table` | 8.20.6 | 8.21.3 | Yes |
| `@radix-ui/react-dialog` | 1.1.4 | 1.1.15 | Yes |
| `@radix-ui/react-dropdown-menu` | 2.1.4 | 2.1.16 | Yes |
| `@radix-ui/react-label` | 2.1.1 | 2.1.8 | Yes |
| `@radix-ui/react-separator` | 1.1.1 | 1.1.8 | Yes |
| `@radix-ui/react-slot` | 1.1.1 | 1.2.4 | Yes — minor |
| `zustand` | 5.0.2 | 5.0.13 | Yes |
| `next-intl` | 4.11.0 | 4.12.0 | Yes |
| `react-hook-form` | 7.54.2 | 7.76.0 | Yes |
| `@hookform/resolvers` | 3.9.1 | **5.2.2** | **Review** — two majors |
| `sonner` | 1.7.1 | **2.0.7** | **Review** — major; toast API checks needed (notifications-cleanup tests guard this) |
| `tailwind-merge` | 2.6.0 | **3.6.0** | **Review** — major |
| `zod` | 3.25.76 | **4.4.3** | **Review** — major; widely-used schema validator |
| `lucide-react` | 0.469.0 | **1.16.0** | **Review** — first stable 1.x release |
| `openapi-fetch` | 0.13.4 | 0.17.0 | Yes — minor |
| `eslint` | 9.39.4 | **10.4.0** | **Review** — major; tooling not runtime |
| `vitest` | 2.1.8 | **4.1.6** | **Review** — two majors; test runner |
| `@vitejs/plugin-react` | 4.3.4 | **6.0.2** | **Review** (pairs with vitest) |
| `typescript` | 5.7.2 | **6.0.3** | **Review** — major; strict-mode tightenings possible |
| `@types/node` | 22.10.5 | 25.9.0 | Yes — types only |
| `prettier` + `prettier-plugin-tailwindcss` | 3.4.2 / 0.6.9 | 3.8.3 / 0.8.0 | Yes |
| `postcss` | 8.4.49 | 8.5.15 | Yes |
| `@playwright/test`, `@axe-core/playwright` | 1.49.1 / 4.10.1 | 1.60.0 / 4.11.3 | **Yes** — eleven minors; playwright is opt-in but quietly drifts |
| `@testing-library/react` | 16.1.0 | 16.3.2 | Yes |
| `@next/eslint-plugin-next` | 16.2.4 | 16.2.6 | Yes |
| `openapi-typescript` | 7.4.4 | 7.13.0 | Yes — codegen tool |

### 8.13 Dependency-bump priority list

Ordered by **(security or correctness impact) ÷ effort**, taking the
existing test surface as the gate:

| # | Bump | Effort | Risk | Why |
|---|---|---|---|---|
| 1 | `cargo update` (lockfile-only, no Cargo.toml changes) | 5 min | Low | Brings tokio, hyper, serde, uuid, blake3, axum, etc. to latest patch within current minor range — biggest cumulative diff, smallest risk |
| 2 | `tailwindcss` + `@tailwindcss/postcss` 4.0.0-beta.8 → 4.3.0 | 1 h | Medium | Only beta-channel pin in the tree; betas drift in confusing ways |
| 3 | `next` 16.2.4 → 16.2.6 + `react` 19.2.5 → 19.2.6 | 30 min | Low | Patch releases for hot upstream projects; bump-then-test |
| 4 | `pnpm update` (lockfile-only) for radix-ui, react-hook-form, react-query, zustand, next-intl | 1 h | Low | All within current major; mostly bug fixes |
| 5 | `@playwright/test` 1.49 → 1.60 | 30 min | Low | E2E harness is opt-in but should not silently fall behind |
| 6 | Auth-stack major bump: jsonwebtoken 9→10 + oauth2 4→5 + openidconnect 3→4 | 1 day | Medium | Group into one PR; gate with `auth_hardening_m*` tests + new `oidc.rs` test from §7.2 |
| 7 | `quick-xml` 0.36 → 0.40 | 2 h | Medium | OPDS feed serializer; gate with `opds*` tests |
| 8 | OTLP stack 0.26 → 0.32 (5 crates in one PR) | 4 h | Low | Currently dormant; bumping while there are no live consumers is cheap |
| 9 | `redis` 0.27 → 1.2 + `apalis` plan | — | High | **Hold** until apalis 1.0 final; coordinate together |
| 10 | `sea-orm` 1 → 2 + `sqlx` 0.8 → 0.9 | — | High | **Hold** until both are stable, not RC/alpha |
| 11 | `vitest` 2 → 4 (+ `@vitejs/plugin-react` 4 → 6) | 4 h | Medium | Test runner major; mostly drop-in but config rewrites are typical |
| 12 | `typescript` 5.7 → 6.0 | 2 h | Medium | Strict-mode tightenings may surface new errors |
| 13 | `sonner` 1 → 2 + `tailwind-merge` 2 → 3 + `zod` 3 → 4 | 1 day | Medium | Three independent majors; can land separately |

**Don't bump:**

- Any RC/alpha/beta-marked crate (sea-orm 2, sqlx 0.9, argon2 0.6,
  zip 9, notify 9, apalis 1, chacha20poly1305 0.11, ed25519-dalek 3)
- `sea-orm` family until 2.0.0 final
- `apalis` family until 1.0.0 final
- OCR pin trio (`comic-text-detector`, `manga-ocr`, `tesseract-rs`)
  remains at 0.5.1 / 0.2 — `ort` cross-crate compatibility documented
  in the workspace comment

**Quick wins:** items 1, 2, 3, 4, 5 should fit in a single afternoon
and would close roughly **80% of the open patch/minor drift** with
near-zero risk. Items 6 and 7 are the next planned batch. The
sea-orm / apalis majors should wait for stable releases.

---

## Appendix — what the audit verified is **not** a problem

- No `@ts-ignore`, `@ts-expect-error`, or `as unknown as` in web code
- No commented-out function/struct/component blocks in either tree
- No production `panic!` / `todo!` / `unimplemented!` / `unreachable!`
- No `std::sync::Mutex` held across an `.await`
- No detached `tokio::spawn` that silently drops errors
- No bypass of the `apiFetch` 401-refresh wrapper, **except** the two
  reader/progress write paths (item 6 above)
- No `Record<string, any>` anywhere in `web/`
- No stale TODOs in `web/` source
- Cover rendering, error envelope shape, ACL gating, CSRF wiring,
  and rate-limit installation are each single-source-of-truth
- `spawn_blocking` coverage is consistent and gated by
  `archive_work_semaphore`
- OPDS v1↔v2 shared helpers (~50 cross-module `pub(crate)` calls)
  prevent drift on the metadata + access-control side
- Sea-orm activemodel construction is *not* duplicated — the
  divergence cost lives at the test layer (item 2), not at the
  production layer
