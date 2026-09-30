# Folio product roadmap

**Source:** [docs/product-audit.md](product-audit.md) (2026-09-29) · **Status:** draft for review · **Owner:** Matthew

This roadmap turns every audit finding into a work package (WP) sized for one Claude Code session, grouped into seven milestones that can ship as independent PRs. §2 records which audit suggestions were excluded and which were pulled back in. Cross-references use the audit's IDs (R# recommendations, DI/UX/AR/OP/AC/SE findings).

---

## 1. Decisions log

Answers given on 2026-09-29. Each decision is binding for the WPs that cite it.

| # | Question | Decision | Consequence |
|---|---|---|---|
| D1 | File watcher | **Build it.** Must be efficient and must not hammer a NAS or re-read files unnecessarily. | WP-3.1 uses inotify only on local mounts, a directory-mtime poll on network mounts, and never hashes on a watcher event alone. |
| D2 | Scaling | **Single instance only.** Docs to say so. | WP-1.5 rewrites `docs/install/scaling.md` and the k8s guide as single-replica; no leases or pub/sub work is planned. |
| D3 | Progress conflicts | **Reading-run model accepted** (2026-09-29): furthest page wins within a run, finished is sticky, explicit "Start re-read" opens a new run; a second device silently follows the new run. | WP-1.3 implements §3.1 as written. |
| D4 | Non-writeback libraries | **Both rules confirmed** (2026-09-29): Folio never writes to an archive in a non-writeback library, and on rescan file-tier values never replace provider- or user-set DB values. | WP-2.4 gates every archive writer; WP-2.5 makes the scanner honour the provider tier. |
| D5 | `curator` role | **Remove it** (2026-09-29). | WP-2.7 includes the migration that drops `library_user_access.role` and the API field; `age_rating_max` stays. |
| D6 | Age cap and unrated content | **Show** unrated content to capped users. | WP-2.7 filters only rows whose rating is above the cap; NULL rating passes. |
| D7 | Metron auth | **Support token auth** if Metron prefers it. | WP-2.9 adds a token setting, prefers it when set, keeps username/password as fallback. |
| D8 | Real-device baseline | No baseline exists; wants an optimized PWA since there is no native app. | WP-4.1 establishes the baseline; M4 treats PWA quality as a first-class goal, including offline reading. |
| D9 | ComicTagger parity | Unclear what it refers to. | Explained in §3.4; small fixture-based WP-6.4 recommended. |
| D10 | Reader bundle budget | No speed issues today; wants it optimized, not excessively large, aligned with modern best practice. | Recommendation in §3.5: code-split the reader, then set the gate to measured size plus 10%. |

---

## 2. Exclusions and pulled-back items

### Pulled back in and planned (2026-09-29)

The audit proposed excluding these; the decision is to build them.

- Series-relationship suggestion engine (spec Phase 7) → **M7, WP-7.1–7.3**
- Recommendations / "similar series" → **M7, WP-7.4**
- GCD provider → **WP-6.1**
- Page-hash marker anchoring → **WP-6.2**

### Confirmed excluded (2026-09-29)

Not scheduled and not to be proposed again without a new reason.

- Guided / panel-by-panel view (needs a panel segmenter; OCR detector is bubble-tuned)
- Automerge / CRDT sync and shared-collection collaboration
- Manga-specific providers (AniList, MangaUpdates) before volume-mode matching exists
- PDF / EPUB readers, loose-image folders
- Meilisearch, pgvector semantic search
- Kavita-compat API (Komga shim already covers clients)
- Multi-replica support, Kobo/KEPUB sync, on-device translation with inpainting
- Gamepad support, AVIF page variants, page-curl animation, configurable tap-zone layouts

**Deferred, not excluded:** CB7 support (WP-6.5).

---

## 3. Design notes for the decisions that needed explanation

### 3.1 Progress: the "reading run" model (D3)

**The conflict.** "Furthest page wins" is right while you read one issue across devices, but wrong the moment you deliberately start a re-read from page 1 on a second device: the server would snap you back to the old furthest page. Today the code is last-write-wins (`crates/server/src/api/progress.rs:220`), which gets the first case wrong instead.

**Proposed model.** Each `progress_records` row carries a `run` counter (new column, default 0) and the client sends the run it is reading in.

- Within the same run the server keeps `max(last_page)`. A stale debounced write from a phone can never move you backwards.
- Reaching the last page sets `finished = true`, which stays sticky within the run. Jumping back to a bookmarked page never un-finishes (existing behaviour, kept).
- An explicit **"Start re-read"** action (reader menu, issue page, and the existing "Mark as unread") increments `run`, resets `last_page` to 0 and `finished` to false, and bumps `reread_count` (stats already track re-reads). Writes tagged with an older run are ignored, so a device that was left open on the old run cannot regress the new one.
- `?peek=1` and incognito stay write-free. `?from=start` becomes "start re-read".
- Sessions and the reading log attribute to the run, so stats show "read twice" correctly.

**Decided (D3):** a second device opening an issue that was re-started elsewhere silently follows the new run (page 0). The existing "Continue from here" banner appears only when the local device still holds unsent writes from the previous run.

### 3.2 Non-writeback libraries stay clean (D4)

Two rules fall out of "files stay clean and original":

1. **Folio never modifies an archive in a library where `allow_archive_writeback` is off.** This already holds for sidecar writeback. WP-2.4 audits the other writers (CBR auto-convert, archive page editor, bulk archive ops, `.bak` creation) and makes the flag the single gate.
2. **On rescan, a file-tier value never replaces a provider- or user-set DB value.** Today `process.rs` re-stamps provider-applied scalars from the file on any content change and rebuilds junctions from CSV columns with no provenance check (DI-6). WP-2.5 applies the audit's attribution rule (user > provider > file) to the scanner itself: file values fill only fields whose provenance is file-tier or unset. The DB is the record for non-writeback libraries; the file is a source, not an override.

**Decided (D4):** both rules confirmed. For non-writeback libraries the database is the record and the file is a source; the "files are canonical, DB is cache" model applies only when writeback is enabled.

### 3.3 The `curator` role (D5)

`library_user_access.role` accepts `reader` or `curator` (`crates/entity/src/library_user_access.rs`). The spec intended `curator` as "a non-admin who may edit metadata and archives in this one library". Nothing reads it; every mutation is admin-gated or owner-scoped. **Decided (D5): remove it.** WP-2.7 ships a migration that drops the column, removes the field from the admin users API and UI, regenerates the OpenAPI types, and keeps `age_rating_max`. A real editor role can be re-added later if a second adult user ever needs it.

### 3.4 ComicTagger parity (D9)

ComicTagger is the community's standard tagging tool; Mylar3, Komga, Kavita, and most hand-tagged libraries carry ComicInfo.xml written by it. "Parity" means: a file tagged by ComicTagger reads into Folio without loss, and a file Folio writes back (ComicInfo + MetronInfo) reads into ComicTagger without loss or spurious diffs. The audit found three ComicInfo fields Folio hard-codes to empty on writeback and that MetronInfo unknown elements are dropped, so round-trips are currently lossy. WP-6.4 adds a fixture pair (a real ComicTagger-tagged CBZ and its Folio rewrite) and a test that the two XMLs are semantically equal for every field both tools know. Small, and it protects the "your archives stay portable" promise.

### 3.5 Reader bundle (D10)

Current: ~190 KB gzip first-load JS against a 150 KB CI gate; `docs/features.md` still claims ~118 KB. **Recommendation:** do not raise the gate first. WP-4.4 code-splits the reader along modern lines, then measures, then sets the gate to measured plus 10%.

- Lazy-load everything not needed to paint page one: marker overlay and editor, OCR capture, settings sheet, shortcuts sheet, end-of-issue card, page strip, first-run overlay.
- Direct icon imports instead of barrel imports; verify no whole-library pulls in the reader chunk.
- `<link rel=preload>` for the first page variant and the strip thumb; `fetchpriority` already set.
- Keep React Compiler on; check the `next build` analyzer output for duplicated vendor chunks between the reader and library routes.
- Add the bundle analyzer report to CI artifacts so regressions are diagnosable, not just gated.

Expected outcome: first-load under 120 KB gzip with the overlays arriving on demand; the gate becomes ~130 KB.

### 3.6 File watcher design (D1)

- **Local mounts:** `notify` + `notify-debouncer-full`, one watcher per library root, recursive. Events are collapsed for 30 s (configurable) into a set of touched directories, then fed to the existing scan path as a **scoped scan** of those directories only, reusing `list_archives_changed_since` and the size+mtime fingerprint. No hashing happens unless the fingerprint changed, exactly as a cron scan.
- **Network mounts (NFS/SMB/CIFS/FUSE):** inotify does not fire for remote writes. Detect via `statfs` filesystem magic at watcher start; on a network mount use a **directory-mtime poll** (default every 5 min, configurable) that stats only directories, not files, and triggers the same scoped scan when a directory mtime advances. This is the cheapest possible probe on a NAS.
- **Safety:** event storms (bulk copy of 10k files) coalesce into one scan through the existing Redis `scan:queued` key; a per-library "watcher paused" state and the admin scan dashboard show the mode (inotify / poll / disabled) and the last trigger.
- **Docs:** the Triggers table in `docs/dev/library-scanner.md` becomes true rather than aspirational.

---

## 4. Session protocol for Claude Code

Every WP is designed to fit one focused session. Conventions that apply to all of them:

- **Branch per WP** from `origin/main`, named `<area>/<wp-id>-<slug>` (e.g. `fix/wp-1.1-provenance-holes`). One PR per WP; auto-merge is never armed.
- **Start** by reading the WP's "Files" and "Evidence" lines and the cited audit finding. Do not widen scope; if a neighbouring bug is found, note it in the PR body and the roadmap's §7 backlog.
- **Tests first** where the WP says "regression test": write the failing test, then the fix.
- **Finish** with `just check`-equivalent: `cargo fmt --check`, clippy `-D warnings`, `cargo nextest run` (or `cargo test -p server --test <file>` while iterating), `pnpm --filter web run lint|typecheck|test`, `just openapi` when the API surface changed, `just audit-check` when an admin handler changed.
- **Docs are part of done.** Any WP that changes behaviour updates the relevant `docs/dev/*.md` or `docs/install/*.md` in the same PR.
- **Plan hygiene:** on completion, tick the WP here, add a one-line memory entry if something non-obvious was learned, and move nothing to `done/` until the whole milestone ships.

Effort key: **S** = half a session, **M** = one session, **L** = two to three sessions, **XL** = a milestone-scale sequence of sessions.

---

## 5. Milestones and work packages

Order within a milestone is the recommended execution order; arrows mark hard dependencies.

### M1 — Integrity (all S/M; target: 2 weeks of sessions)

Exit: every promise in `docs/features.md` and `docs/install/*.md` is true or removed; the silent-overwrite paths have regression tests; CBT reads; progress is correct across devices.

**Status (2026-09-29): all six WPs implemented, in review.** PRs: 1.1 #877 · 1.2 #880 · 1.3 #882 · 1.4 #878 · 1.5 #883 · 1.6 #879 · docs (this file, the audit, `features.md`) in their own PR. Merging is manual. Notes from implementation: WP-1.2 replaced the synthesized health note with the `page-removed` marker tag; WP-1.6 found apalis-redis 0.7.4 has no public retry setter, so the budget is a mirrored constant with a guard test; WP-1.3 deliberately changed page-strip jump-back semantics (documented in `docs/dev/reading-progress.md`).

| WP | Title | Effort | Audit | Scope | Files | Done when |
|---|---|---|---|---|---|---|
| 1.1 | Provenance holes B1–B6 | M | R1, DI-1..5 | Alias `Summary`↔`Description` in `should_apply`; compare provenance not value in `put_external_id` and return `Skipped`; write `SetBy::User` rows in `update_series` for summary/status; read `override_external_id_sources`; gate `number_raw` on its own field; fix `MarkerView.kind` comment | `crates/server/src/metadata/{apply,writers}.rs`, `api/{series,issues}.rs`, `api/markers.rs:113` | Six new failing-then-passing tests in `metadata_apply.rs` / `metadata_writers.rs` / `series_edit`; `override_external_id_sources` has a test that "Use theirs" replaces the user row |
| 1.2 | Marker + progress remap on archive edit | M | R2, DI-19 | Apply the `simulate_ops` old→new map to `markers.page_index` and `progress_records.last_page` in the edit transaction; markers on removed pages get `page_index = removed ordinal's neighbour` and a `tags += ["page-removed"]` marker, not deletion; post-rescan sweep flags `page_index >= page_count` as a synthesized health note | `crates/server/src/jobs/archive_edit.rs`, `api/health_issues.rs` | New `markers_archive_edit.rs` test: reorder, remove, and rotate cases; reader Jump link lands on the right pixels |
| 1.3 | Reading-run progress model | M | R3, UX-1, D3 | Migration adds `run INT NOT NULL DEFAULT 0` to `progress_records`; `upsert_for` keeps `max(last_page)` within a run and ignores older runs; "Start re-read" endpoint + reader/issue-page action; `?from=start` maps to it; sessions/log attribute to run; fix the module doc | `crates/migration/`, `api/progress.rs`, `web/lib/reader/use-progress-write.ts`, `R/page.tsx`, issue page menu | Two-device tests (stale write ignored; re-read on B not regressed by A); finished sticky within run; stats re-read count increments |
| 1.4 | CBT reader path | S | R4 | Route `zip_lru` through `archive::open` with a non-indexed streaming fallback for CBT; keep the zero-lock indexed path for CBZ | `crates/server/src/library/zip_lru.rs`, `api/page_bytes.rs` | `page_bytes` test streams a CBT page and a Range on it |
| 1.5 | Docs truth pass + single-instance declaration | S | R5, OP-1, D2, DI-12, DI-18 | Remove watcher claims pending WP-3.1 (or mark "planned"); fix `backup.md` Redis sentence; rewrite `scaling.md` and `kubernetes.md` as single-replica, delete the `replicas: 3` example and pub/sub sentence; implement `archive_backup_retain_days` prune in the daily sweep (small) or remove the field; fix operator-guide threshold 95→80; refresh `library-scanner.md` health table, `archive_edit.rs` header, `metadata_refresh` scan-mode mention, bundle-size claim | `docs/install/*.md`, `docs/dev/*.md`, `docs/features.md`, `jobs/orphan_sweep.rs` or `api/libraries.rs` | Every sentence changed is backed by a grep or a test; `retain_days` either prunes (test) or is gone (migration) |
| 1.6 | Ops hygiene bundle | S | R22, OP-2, AR-3, AR-5 | Boot guard `server_version_num >= 170000` with actionable error; explicit `set_max_retries` per apalis queue so the "5" is code; `TimeoutLayer` (60 s, excluding page bytes and WS) and `CompressionLayer` for JSON; `just backup` / `just restore` wrapping the documented commands; cron lease helper is **not** needed (D2) | `app.rs`, `jobs/mod.rs`, `justfile`, `docs/install/backup.md` | Boot against PG16 fails with the message; `just backup` produces a dump + `/data` tarball |

### M2 — Ownership and safety (target: 4–5 weeks of sessions)

Exit: any scanner mistake is fixable in-app; hand edits reach the archive safely when writeback is on and never touch files when it is off; kids' caps are enforced everywhere; user data can be exported in one file; provider calls are budget-aware.

**Status (2026-09-29): all nine WPs implemented.** PRs: 2.1 #887 · 2.3 #886 · 2.4 #885 · 2.5 #888 · 2.6 #892 · 2.7 #890 (`breaking-change`: the grant view loses `role`) · 2.8 #891 · 2.9 #889 · 2.10 #895. Merge notes: main was briefly red after #890 merged over #892 (fixed by #894); #888 and #889 were rebased after the later merges. Findings from implementation: #891 fixed a pre-existing bug where the direct apply wrote a publisher's ComicVine id onto the series; #890 fixed a 500 in `/api/people` for restricted users; Metron token auth is `Authorization: Bearer` and conditional requests exist only on detail endpoints (#889); the export carries `run` (#887); WP-2.10 reuses the drift flush's DB-only compose path and mirrors the series-scope apply's fan-out.

| WP | Title | Effort | Audit | Scope | Files | Done when |
|---|---|---|---|---|---|---|
| 2.1 | User-data export | M | R6 | `GET /me/export` streams JSON: progress (with run), markers, collections + entries, ratings, saved views, custom pages, sidebar layout, reading log, keybinds; every issue reference carries `content_hash`, `issue_id`, and `(series name, year, number)`; versioned envelope; "Export my data" button on the account settings page | new `api/account_export.rs`, `app.rs`, account settings page | Export of a seeded library contains every row the user owns (test compares counts per section); documented shape in `docs/dev/export-format.md` |
| ~~2.2~~ | ~~User-data import~~ | — | — | **Removed 2026-09-29 (owner decision: no import feature).** The export (2.1) stays as the durable dump and the base for the notes export (5.1); restoring is `pg_restore` per `docs/install/backup.md`. | — | — |
| 2.3 | Series identity edits | M | R7, UX-6 → 1.1 | `UpdateSeriesReq` gains name, year, volume, publisher, imprint, age_rating, total_issues, language; each write pins `SetBy::User`; scanner and apply honour series pins (`fetch_user_pinned_fields` for series); `SeriesEditDrawer` gains the fields with `applyServerErrors` | `api/series.rs`, `metadata/writers.rs`, `scanner/process.rs`, `web/components/library/SeriesEditDrawer.tsx` | Edited year survives force rescan and weekly refresh; identity resolver does not re-home the series on rename |
| 2.4 | "Files stay clean" gate audit | S | D4 rule 1 | Every archive writer (CBR auto-convert, page editor, bulk ops, `.bak`) checks `allow_archive_writeback`; UI hides the affordances when off | `scanner/cbr_convert.rs`, `api/archive_edit.rs`, web archive-edit dialogs | Test: each writer returns 409 `archive.writeback_disabled` when the flag is off |
| 2.5 | Rescan honours provider tier | M | D4 rule 2, DI-6, DI-7, DI-8 | Scanner writes file values only where provenance is file-tier or unset; junction rebuild diffs against provider-written rows and preserves `person_id`/ordinal; `scan_information`/`community_rating`/`review` join the gated set; edit endpoints write provenance inside the row transaction; retire the nine `user_edited` reads in the scanner (rest of `user_edited` in WP-3.7) | `scanner/process.rs`, `scanner/metadata_rollup.rs`, `api/issues.rs` | Test: provider-applied credits and summary survive a content-changed rescan on a non-writeback library; provenance stays `provider` |
| 2.6 | Writeback hardening | L | R9, DI-10..14, OP-8 | Rename order that never leaves the target missing (write tmp → hard-link target to `.bak` → rename tmp over target); carry unknown ZIP entries (CoMet.xml, `.json`, `.txt`) through `cbz_write`; MetronInfo raw passthrough; stop hard-coding `MainCharacterOrTeam`/`AlternateNumber`/`AlternateCount`; lock-busy returns `Err` so apalis retries; refuse apply-via-sidecar on CBR/CBT with a clear error (or auto-convert first when the library allows); write provenance/variants/`last_metadata_sync_at` only after a successful rewrite; drift ignores page-edit stamps; lock heartbeat | `archive_rewrite/mod.rs`, `archive/src/cbz_write.rs`, `metadata/sidecar_compose.rs`, `parsers/src/metroninfo.rs`, `jobs/rewrite_sidecars.rs`, `metadata/{apply,drift}.rs` | Crash-between-renames test leaves the original readable; sidecar-preservation test; CBR apply test; lock-busy retry test |
| 2.7 | Age-rating enforcement + drop `curator` | M | R10, SE-4, D5, D6 | `VisibleLibraries` carries per-library cap; series/issue lists, search UNION arms, OPDS feeds, page bytes, covers, and Up Next filter `age_rating IS NULL OR rank(age_rating) <= cap` using the ComicInfo rating ladder; admin UI already sets the cap; migration drops `library_user_access.role`, admin users API/UI lose the field, `just openapi` regenerates types | `library/access.rs`, `api/{series,issues,opds*,page_bytes,thumbnails,next_up,admin_users}.rs`, migration, `web/app/[locale]/(admin)/admin/users/**` | ACL test matrix: capped user sees unrated + at-or-below; direct page-bytes URL for an above-cap issue 404s; no `curator` reference remains in `crates/` or `web/` |
| 2.8 | Match-query override, search-by-URL, year-gate escape | M | R12, UX-13 | Optional `{name, year, publisher, issue_number}` overrides on series/issue search requests; "paste a ComicVine/Metron URL" path that calls `fetch_series`/`fetch_issue` directly and enters the normal preview; if the hard year gate empties the list, retry once with `PhashAware` and annotate the run | `api/metadata_search.rs`, `metadata/orchestrator.rs`, match dialog components | wiremock tests for override, URL fetch, and gate fallback |
| 2.9 | Provider resilience + Metron token + budget bar | L | R16, DI-16, DI-22, D7 | `metadata.metron.api_token` secret setting, preferred over user/pass; parse `Retry-After` and Metron rate-limit headers into the quota model; ETag conditional requests on Metron list endpoints; bounded retry with jittered backoff on 5xx/transport in a shared `provider::retrying_send`; `connect_timeout` + `Policy::limited(2)` on both builders; JSON body cap; search-time cover pHash cache keyed by provider image URL in `metadata_cache`; reuse one client in `fetch_and_hash_cover`; admin metadata page shows remaining budget per provider | `metadata/{comicvine,metron,provider,cache,phash}.rs`, `settings/registry.rs`, `api/admin_metadata.rs`, web admin metadata page | wiremock tests: `Retry-After` honoured, 503 then 200 succeeds, ETag 304 path, token header sent; budget bar renders from headers |
| 2.10 | Manual edit → sidecar writeback | M | R8 → 2.6 | Issue `PATCH` and bulk-metadata enqueue `RewriteIssueSidecarsJob` when both writeback flags are on; coalesced per issue; series identity edits (2.3) enqueue a series-scoped rewrite | `api/issues.rs`, `api/series.rs`, `jobs/rewrite_sidecars.rs` | Test: hand edit lands in ComicInfo.xml and survives the scoped rescan with `user` provenance |

### M3 — Library management at scale (target: 4 weeks of sessions)

Exit: a 50k-issue NAS library imports in a predictable time, new files appear without cron, mismatches and duplicates are findings you can act on, and the hot list queries have measured plans.

**Status (2026-09-30): all eight WPs merged.** PRs: 3.1 #902 · 3.2 #903 · 3.3 #900 · 3.4 #897 · 3.5 #898 (purge off by default) · 3.6 #904 · 3.7 #901 (`breaking-change`: `issue.user_edited` removed; `field_provenance` is the only pin store) · 3.8 #899, plus the cover-slot uniqueness fix #905 found along the way. #903's post-hash dedupe respects `dedupe_by_content`, made live by #900.

| WP | Title | Effort | Audit | Scope | Files | Done when |
|---|---|---|---|---|---|---|
| 3.1 | File watcher | L | R14, D1, §3.6 | `notify-debouncer-full` on local mounts; `statfs` mount-type detection; directory-mtime poll on network mounts; scoped scans; coalescing; admin dashboard mode + last trigger; per-library toggle already exists; docs Triggers table made true | new `library/watcher.rs`, `jobs/scheduler.rs`, `api/server_info.rs`, admin scan dashboard, `docs/dev/library-scanner.md` | Integration test with a temp dir (inotify) and a forced-poll mode; a 1,000-file copy triggers exactly one scan; NAS path stats directories only (assert via a counting fs shim) |
| 3.2 | First-import lazy-hash mode | M | §3.1 of audit | Library option "trust fingerprint on first import": ingest with size+mtime, enqueue hashing as a background job with progress; identity uses path until the hash lands; dedupe re-checks after hashing | `scanner/process.rs`, new `jobs/hash_backfill.rs`, library settings UI | Cold import of the stress fixture on a slow-disk simulation completes without hashing; hashes backfill; retag detection still works after backfill |
| 3.3 | Duplicates page | M | R15, UX-9, DI-21 | `GET /libraries/{slug}/duplicates` (cursor): groups by `(series, sort_number, special_type)` >1, exact-hash pairs, cover Hamming ≤8 within a series; actions keep / soft-remove / open editor; library-scope the hash lookup; honour or drop `dedupe_by_content` | new `api/duplicates.rs`, `scanner/process.rs:536`, new admin page | Tests for each grouping; cross-library same-file no longer flagged |
| 3.4 | Wire the stub health kinds | M | R23 | Emit `FolderNameMismatch` (folder vs ComicInfo `<Series>`) and `MixedSeriesInFolder`; delete `AmbiguousVolume` and `OrphanedSeriesJson` or implement if cheap; `AmbiguousFolder` row lists the skipped subtree | `library/health.rs`, `scanner/{enumerate,process}.rs` | Fixtures for each kind under `fixtures/`; findings page shows them |
| 3.5 | Hard-purge job | S | audit §3.1 | Daily sweep hard-deletes rows confirmed-removed for more than `soft_delete_days × 2` (configurable), cascading markers/progress with an export-to-log first | `jobs/scheduler.rs`, new sweep | Test: purge respects the window and writes a library event |
| 3.6 | Load baseline and projections | M | R25, OP-6, AR-1 | `just perf-explain` runs `EXPLAIN (ANALYZE)` for the top ten list/filter/sort queries against `fixtures/build.py --scale stress`; audit the 80 `issue::Entity::find()` sites on list paths and project with `select_only`; either write `docs/dev/load-testing.md` with a `oha` recipe or delete the spec promise | `justfile`, `api/*.rs`, `docs/dev/` | Plans recorded in `docs/dev/load-testing.md`; no seq scan on the top ten at stress scale |
| 3.7 | Retire `issue.user_edited` | M | AR-6, DI-8 | Replace the remaining reads (`issues.rs`, `series.rs:992`) with `field_provenance`; drop the column in a migration after a backfill check | `api/{issues,series}.rs`, migration, `docs/dev/schema-restructure.md` | Zero references; migration up/down tested |
| 3.8 | Migration down in CI + thumbs budget | S | AR-9, OP-5 | CI job runs `up` then `down` for the last 10 migrations against the template DB; `folio_thumbs_bytes` gauge and an optional byte budget with LRU like variants | `.github/workflows/ci.yml`, `library/thumbnails.rs`, `jobs/orphan_sweep.rs` | CI green with the new job; gauge visible at `/metrics` |

### M4 — Reader and PWA (target: 4–5 weeks of sessions)

Exit: the reader is measured on a real phone, reads well on desktop and tablet, keeps progress offline, downloads issues for offline reading, passes axe, and ships a code-split bundle with a truthful gate.

**Status (2026-09-30): 4.2–4.8 merged; 4.1 is owner-run and still open.** PRs: 4.2 #911 (`breaking-change`) · 4.3 #910 · 4.4 #913 (first-load 191 → 117 KB, gate 130 KB) · 4.5 #912 · 4.6 #914 · 4.7 #908 · 4.8 #909. 4.3, 4.5 and 4.6 were stacked and landed on main through #911. WP-4.4's gate should be re-measured after 4.2/4.3/4.5/4.6, which added reader code after the gate was set; WP-4.1 needs the owner's phone and tablet.

| WP | Title | Effort | Audit | Scope | Files | Done when |
|---|---|---|---|---|---|---|
| 4.1 | Real-device baseline | S | D8, OP-6 | Run the `docs/dev/pwa-performance.md` protocol on your phone and tablet (Lighthouse + WebPageTest + manual page-turn timing); record numbers; fix the 24 MP decode budget and webtoon mount window if they are wrong for the device | `docs/dev/pwa-performance.md` | Table filled; any tuning change has before/after numbers |
| 4.2 | Reader polish set | M | R17, UX-3..5, UX-10 | Ctrl+wheel / trackpad zoom; `contain` fit mode; zoom persists across pages behind a preference; zoom in double-page view; mirrored progress bar in RTL; series `ttb` → webtoon; left-edge inset for swipe in iOS standalone | `web/lib/reader/{zoom,detect,store}.ts`, `R/Reader.tsx`, `R/ReadingProgress.tsx`, `use-swipe.ts` | Unit tests per behaviour; keybinds sheet updated; docs `reader-shortcuts.md` |
| 4.3 | Manual spread controls | M | R18, UX-2 | Per-issue, per-user overrides: force spread / force pair / shift pairing by one; stored in a small `issue_page_overrides` table; strip affordance and settings toggle | migration, `api/issues.rs`, `web/lib/reader/spreads.ts`, `R/PageStrip.tsx` | Test: overrides win over `DoublePage` and aspect detection; persisted across devices |
| 4.4 | Reader code-split + bundle gate | M | D10, UX-11, §3.5 | Lazy chunks for overlays; direct icon imports; preload for first page; analyzer report as CI artifact; set gate to measured +10%; fix `features.md` number | `web/app/[locale]/read/**`, `web/next.config.ts`, CI bundle step | First-load ≤ ~120 KB gzip; gate updated; no functional regression in reader tests |
| 4.5 | Durable progress outbox | M | R19 | IndexedDB queue for progress and session writes; replay on `online`/launch; conflict-safe with the run model | `web/lib/reader/progress-writer.ts`, new `web/lib/pwa/outbox.ts`, sw hooks | DOM test: kill-and-relaunch replays; e2e offline flush |
| 4.6 | Per-issue offline download | XL | R27 → 4.5 | "Download for offline" on issue/series: pages (chosen variant tier) + metadata + strip thumbs into Cache Storage/IDB with a quota display; offline-capable reader shell; eviction UI; follows `docs/dev/pwa-offline-reading-plan.md` steps 1–4 | `web/app/sw.ts`, `web/lib/pwa/*`, reader shell, settings | e2e: download, go offline, read, progress replays on reconnect |
| 4.7 | PWA install completeness | S | inventory | Generate the manifest PNG icons from the brand masters (or interim glyphs) so install prompts stop showing placeholders; verify shortcuts and `display: standalone` on Android and iOS | `web/public/icons/`, `web/app/manifest.ts` | PWA e2e asset check passes with real icons |
| 4.8 | Reader accessibility pass | M | R24, AC-2..5 | Reader in the axe e2e; keyboard proxies for detected OCR regions; "page text" panel exposing OCR text to AT; bump 10–11 px labels; first-run hint that `t` shows chrome | `web/tests/e2e/a11y.spec.ts`, `R/MarkerOverlay.tsx`, `R/ReaderChrome.tsx` | axe clean on the reader; screen reader can reach page text |

### M5 — Discovery, annotation, and matching quality (target: 3–4 weeks of sessions)

Exit: notes are durable and browsable in context; issue-level queries exist; entities have pages; TPBs and annuals match correctly.

**Status (2026-09-30): all seven WPs merged.** PRs: 5.1 #917 · 5.2 #919 · 5.3 #925 (replaced #922, auto-closed when its stacked base merged) · 5.4 #921 · 5.5 #920 · 5.6 #918 · 5.7 #923, plus #924 (keyset cursors on saved-view results, `/me/markers` and admin metadata runs skipped the lookahead row at every page boundary). Owner decisions (2026-09-30): the notes export keeps every owned marker but marks ones on removed/hidden issues unavailable (no jump link); marker colour is a palette name or `#RRGGBB(AA)`; regions must fit the page; DI-20 visibility also applies to marker count/search/tags. Issue views are a separate `filter_issues` kind with keyset paging, `name` = series and `title` = issue title, `rating` = the viewer's own. Entity pages 404 when nothing is visible, sit under one "Browse" sidebar entry, and the scanner rollup now links character/team/arc/publisher rows by id (backfilled by migration; file-owned `issue_arcs` are reconciled from the `story_arc` text). Matching: ComicVine format inference feeds matching only and is never written; untagged plain-numbered issues count as singles; the format-mismatch penalty is a fixed 15 that also caps HIGH to MEDIUM.

| WP | Title | Effort | Audit | Scope | Files | Done when |
|---|---|---|---|---|---|---|
| 5.1 | Notes export + permalink | S | R11, UX-12 → 2.1 | `GET /me/markers/export` (`format=md` or `json`) grouped series → issue → page with body, captured text, tags, Jump URL; `GET /markers/{id}` → 303 to reader with `page` + `peek`; "Copy link" on cards | `api/markers.rs`, `api/issue_permalink.rs`, `MarkersList.tsx` | Markdown fixture snapshot test; permalink test |
| 5.2 | Notes in context | M | R11 | `series_id` filter on `ListQuery`; expose `issue_id`; "Notes" tab on series and issue pages reusing `MarkersList`; in-reader marker drawer listing this issue's markers; ACL re-check and `removed_at` join on `/me/markers` (DI-20) | `api/markers.rs`, series/issue pages, new `R/MarkerDrawer.tsx` | Tests for filters and ACL; drawer keyboard-navigable |
| 5.3 | Editable captured text + limits + validation | S | UX-12, SE-5 | Free-text edit of `selection.text`; cap raised to 8 KB; `Validated<CreateMarkerReq>` with garde; `color` capped; region `w/h ≥ 0.5%`; rate-limit bucket on marker writes | `api/markers.rs`, `R/MarkerEditor.tsx` | 422 envelope tests; editor round-trip test |
| 5.4 | Issue-level smart views | M | R20, UX-8 | `entity: issue` views with `special_type`, `format`, `story_arc`, `rating`, `read_status`, `is_empty`/`is_not_empty`; rails accept issue views; saved-view translator updated | `views/{dsl,compile}.rs`, `api/saved_views.rs`, filter registry, view builder UI | Compiler tests; a rail of "unread annuals 2019" renders |
| 5.5 | Entity landing pages | M | R21, UX-7 | `/characters/[slug]`, `/teams/[slug]`, `/arcs/[slug]`, `/publishers/[slug]` on the creators template with cursor pagination and OPDS feeds | new `api/{characters,arcs,publishers}.rs`, web routes | Pages render for the dev library; OPDS nav links |
| 5.6 | Format / series-type awareness | M | R13 | Populate `GenericMetadata.series_type/format` from CV volume type and Metron `series_type`; soft penalty for collected editions vs singles; normalise "Annual N", "½", "14AU"; language-aware article list only if a fixture proves the need | `metadata/{comicvine,metron,matcher,title_norm}.rs`, golden suite | New golden fixtures: TPB vs ongoing, annual, manga volume; existing 24 still pass |
| 5.7 | Smart-view "has notes / bookmarked" filters | S | R11 | `has_notes`, `has_bookmarks`, `has_highlights` as EXISTS filters at series and issue level | `views/compile.rs`, registry | Compiler test |

### M6 — Providers, portability, and residual security (target: 3 weeks of sessions, after M2)

| WP | Title | Effort | Audit | Scope | Files | Done when |
|---|---|---|---|---|---|---|
| 6.1 | GCD provider | L | R26 → 2.9 | Third `MetadataProvider` over GCD's JSON API with tolerant deserialisation (fields are declared unstable, so unknown/missing fields must never fail a search); Basic-auth settings keys + `metadata.gcd.enabled`; CSP `img-src` allowlist entry for GCD cover hosts; `list_series_issue_numbers` (GCD is a splitter); rate bucket (~100/h, 1/s); wiremock tests from recorded fixtures; operator-guide section; budget bar entry | new `metadata/gcd.rs`, `metadata/orchestrator.rs:78-112`, `settings/registry.rs`, `middleware/security_headers.rs`, `api/admin_metadata.rs`, docs | Search and apply round-trip against recorded fixtures; a fixture with a renamed field still parses; budget bar shows GCD; golden suite unchanged |
| 6.2 | Page-hash marker anchoring | L | R28 → 1.2 | Store a per-page content hash on markers and progress at capture (the scanner's page index already carries entry hashes); on rescan or archive replacement re-resolve `page_index` by hash before falling back to the ordinal map from WP-1.2; drift note when neither resolves; OCR cache key unaffected | migration (`markers.page_hash`, `progress_records.page_hash`), `api/markers.rs`, `api/progress.rs`, `scanner/process.rs`, `jobs/archive_edit.rs` | Test: a replaced archive with reordered pages keeps markers and the resume position on the right image; ordinal fallback test |
| 6.3 | Residual security items | M | SE-2, SE-3, SE-6 | Explicit `CorsLayer` deny-by-default; strip issuer from anonymous `/auth/config`; per-user CBL import quota; multipart content-type check; `LIMIT` binding tidy; pepper rotation via dual-pepper verify-and-rehash; magic-sniff provider cover bytes before persisting and require https; `override_user_edits` via `RequireAdmin` and `_force` audit on composite apply; add a status column to `docs/dev/security-audit.md` | `app.rs`, `auth/*`, `api/{auth_config,cbl_lists,metadata_search}.rs`, `metadata/writers.rs`, docs | Each item has a test; audit doc shows status per finding |
| 6.4 | ComicTagger parity fixture | S | D9, §3.4 | Real ComicTagger-tagged CBZ fixture + Folio's rewrite of it; test that both XMLs agree on every shared field; document known intentional differences | `fixtures/`, `crates/server/tests/sidecar_parity.rs`, `docs/dev/metadata-sidecar-writeback.md` | Test passes; diff list is empty or documented |
| 6.5 | arm64 images + CB7 | M | OP-3, inventory | `linux/arm64` in `release.yml` on arm runners (release only); CB7 via `sevenz-rust2` as convert-on-scan like CBR | `.github/workflows/release.yml`, `crates/archive/src/cb7.rs`, `scanner/cbr_convert.rs` | arm64 image boots in docker-smoke; CB7 fixture converts |
| 6.6 | Locale segment decision | S | AR-2 | Either wire a second locale end-to-end or collapse `[locale]` per the "keep i18n, wire later" memory; this WP only writes the decision and the plan | `docs/dev/i18n-completion-plan.md` | Decision recorded |

---

### M7 — Relationships and similar series (target: 4 weeks of sessions, after M5)

Exit: series carry typed, traversable relationships (manual and suggested), the scanner proposes them with confidence, and every series page offers "related" and "similar" rails that are explainable and never wrong-by-magic.

| WP | Title | Effort | Audit | Scope | Files | Done when |
|---|---|---|---|---|---|---|
| 7.1 | Relationship schema, API, and manual editing | M | spec §5.2 / Phase 7 | `series_relationship(from_series, to_series, kind, source, confidence, created_by, created_at)` with kinds `sequel_of / prequel_of / spin_off_of / crossover_with / collects / collected_in / same_universe / see_also`; inverse pair auto-created and kept in sync; unique on `(from, to, kind)`; `GET/POST/DELETE /series/{slug}/relationships`; recursive traversal query (CTE, depth ≤ 6) for "chain" views; admin audit via `record_admin_action!`; "Related" section on the series page with add/remove; OPDS related links | migration, new `crates/entity/src/series_relationship.rs`, new `api/series_relationships.rs`, series page component, `api/opds*.rs` | Inverse-pair and cycle tests; CTE depth cap test; audit-check passes; series page renders a chain |
| 7.2 | Suggestion engine | L | spec Phase 7 → 7.1 | apalis job after each library scan (and on demand) that proposes relationships from evidence already in the DB: ComicInfo `AlternateSeries`/`SeriesGroup`, shared `StoryArc` across series (crossover), name continuation with year gap ("X (2011)" → "X (2016)" = sequel), `<Format>`/`special_type` collected editions whose `Notes`/title cite issue ranges (collects), shared provider volume ids and `series_provider_range` (same run split), publisher + shared characters/teams density (same universe); each suggestion carries a confidence bucket and a human-readable reason; per-scan cap 1000; dedupes against accepted/rejected; rejected suggestions are remembered | new `jobs/relationship_suggest.rs`, `library/scanner` post-scan hook, migration (`series_relationship_suggestion`) | Fixture library yields the expected suggestions with reasons; rejected ones do not reappear; runtime bounded on the stress fixture |
| 7.3 | Review UI and bulk accept | M | spec Phase 7 → 7.2 | Admin page listing pending suggestions grouped by confidence with reason text and both covers; accept / reject / edit kind; bulk-accept high confidence; per-series inline "Suggested" chips on the series page; audit rows for every decision | new admin route + components, `api/series_relationships.rs` | Accept creates the pair via WP-7.1; bulk accept audited once per batch; e2e smoke of accept flow |
| 7.4 | Similar series | M | audit §3.6 | Content-based, explainable similarity with no ML: weighted overlap over creators (by role), characters, teams, genres, tags, publisher/imprint, story arcs, and accepted relationships, computed on demand with a small per-series cache invalidated by scan/apply; `GET /series/{slug}/similar` (cursor) returning the top matches with a "because" list; "Similar series" rail on the series page and an optional home rail; excludes series the user has hidden; respects library ACL | new `api/series_similar.rs`, `views/` or a dedicated similarity module, series page rail, `rails.rs` | Query-count guard in `perf_regressions.rs`; explanation list matches the overlap; ACL test; dev library produces sensible neighbours |

---

## 6. Dependency graph (hard edges only)

```
1.1 ──► 2.3 ──► 2.10
1.2 ──► 6.2
1.3 ──► 4.5 ──► 4.6
2.1 ──► 5.1
2.6 ──► 2.10
2.9 ──► 6.1
7.1 ──► 7.2 ──► 7.3
7.1 ──► 7.4
```

Everything else can be scheduled in any order inside its milestone. M1 has no external dependencies and should go first; M3 and M4 are independent of each other and of M2 except where the graph says otherwise, so they can interleave with M2 sessions when a change of pace helps.

---

## 7. Backlog and parking lot

Items noticed during the audit that are real but small, to be picked up opportunistically inside a related WP:

- `docs/features.md` is untracked; commit it once WP-1.5 corrects it.
- `types.ts` inline-debt guard: a vitest that fails if the inline block grows (AR-1 of audit §4.3).
- Thumbs `Cache-Control: private, max-age=300` + ETag instead of `no-cache` (OP-7).
- Variant-backfill drain stall (`backfill.rs:109`) and deactivated primary-cover file cleanup (DI-15): fold into WP-2.9 or WP-3.8.
- Stringly enums for `user.role`, `user.state`, `issue.state`, `marker.kind` (AR-7): convert opportunistically when a WP touches the entity.
- Rail-icon picker fix: PR #876 (open).
- Stale scanner env defaults noticed during WP-1.5: `docs/dev/library-scanner.md` and `.env.example` say `COMIC_SCAN_WORKER_COUNT` defaults to `min(cpu, 4)` and `COMIC_SCAN_HASH_BUFFER_KB` to 64; `config.rs` has `min(cpu, 8)` and 1024. Fix alongside WP-3.1.
- `docs/dev/comic-reader-spec.md` and `library-scanner-spec.md` still describe the intended watcher and pub/sub as design; left as specs, not claims.
- Found during M5 (2026-09-30):
  - `/creators/{slug}` returns 200 with empty lists when nothing is visible; align it with the entity pages' 404.
  - A single bulk-restore endpoint for marker Undo, after which the `marker_write` burst can drop from 600 to ~60.
  - `has_favorites` smart-view filter (only notes/bookmarks/highlights exist).
  - Issue saved views in OPDS feeds; multi-select on the issue-view detail page.
  - `useAdminMetadataRuns` uses `useQuery` on a `next_cursor` response, so the admin Runs tab shows only the first 25 runs; convert to `useInfiniteQuery`.
  - The arc/character/team entity pages have OPDS 1.x feeds only; no OPDS 2.0 equivalents yet.

---

## 8. Open decisions still needed from you

None. Every audit item is now either scheduled (§5) or confirmed excluded (§2). Work can start with WP-1.1.

## 9. Decision history

- 2026-09-29: D1–D10 answered; D3, D4, D5 settled per §3.1–§3.3 (reading-run model with silent follow; both clean-files rules; `curator` removed).
- 2026-09-29: M2 started; WP-2.2 (user-data import) removed at the owner's request. All nine M2 WPs opened as #885–#892 and #895 the same day.
- 2026-09-29: exclusion list ruled on. Pulled back in and planned: relationship suggestion engine (M7), similar series (WP-7.4), GCD provider (WP-6.1), page-hash marker anchoring (WP-6.2). All other proposed exclusions confirmed excluded. An earlier edit the same day had these four backwards; corrected.
- 2026-09-30: M3 and M5 fully merged; M4 merged except owner-run WP-4.1. M5 owner decisions recorded in the M5 status line.
