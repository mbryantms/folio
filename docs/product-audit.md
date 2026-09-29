# Folio product audit and roadmap

**Date:** 2026-09-29 · **Codebase:** v0.28.2 (`d1c2729`, branch `fix/thumbs-skip-soft-removed`) · **Scope:** Rust workspace, Next.js web app, Postgres schema, apalis jobs, provider integrations, deployment, tests.

This is an audit and roadmap, not a change set. Every material finding cites a repository path and, where possible, a line number or function. Findings are labelled **Observed** (read in code, tests, or docs), **Inference** (derived from observed code paths), or **Proposal**. Where a claim of absence is made, the search that came back empty is named. Line numbers are accurate for the commit above and will drift.

---

## 0. How this audit was done

**Repository map.** Seven crates (`archive`, `entity`, `migration`, `parsers`, `server`, `server-macros`, `shared`, plus `tools/audit-check`); the `server` crate is ~110k lines with 60 API handler modules, 72 entities, 100 migrations, and 138 integration test files. The Next.js 16 app has 49 page routes across library, reader, settings, admin, and auth groups, with 107 vitest files and 5 Playwright specs. Forty developer docs under `docs/dev/`, install docs under `docs/install/`, and an untracked feature showcase at `docs/features.md`.

**Method.** Six parallel read-only deep dives (metadata providers, reader, markers, organization/import, architecture/ops/security, and a web research pass on comparable tools), followed by direct verification of every headline defect against the source. Reports were cross-checked against `docs/features.md`, `docs/dev/phase-status.md`, `docs/dev/comic-reader-spec.md` §19–21, and `docs/dev/security-audit.md`.

**Checks run** (all on this workstation; the dev server was running on `:8080` throughout):

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean (one future-incompat warning from `apalis-redis 0.7.4`) |
| `just audit-check` | 64 files scanned, all `RequireAdmin` handlers audited |
| `just openapi-check` | no drift |
| `cargo nextest run --workspace --all-features` (external PG 18 + Redis 8, throwaway containers on alternate ports) | **1829 passed, 3 skipped**, 158 s |
| `pnpm --filter web test` | 808 passed across 107 files |
| `pnpm --filter web run lint` / `typecheck` | clean (2 pre-existing warnings) |

**Limitations.** No provider credentials, so ComicVine/Metron behaviour is from code and wiremock tests, not live calls. No large sample library was scanned; throughput numbers are the repo's own baseline (`docs/dev/scanner-perf.md`). The `just test-rust-fast` recipe could not run as-is because an unrelated container holds port 5433; the same suite was run on alternate ports. Playwright e2e and the docker-smoke gate were not re-run locally. Competitor version and date claims that could not be pinned to a primary source are marked UNVERIFIED in §9.

---

## 1. Executive summary

Folio is a mature, unusually well-tested self-hosted comic server. The reader, scanner, OPDS surface, metadata matcher, and admin area are all real and largely match what `docs/features.md` advertises. The test suite passes in full and the CI gates (OpenAPI drift, audit-log completeness, oasdiff, docker-smoke, SBOM) are stronger than any comparable open-source tool. The problems found are not missing pillars; they are a small number of silent data-integrity holes, a handful of features that the docs promise but the code does not deliver, and gaps in long-term library management that only bite at 10k+ issues or after a year of hand edits.

### The five highest-value opportunities

1. **Close the user-edit precedence holes in the metadata pipeline.** The headline promise ("your edits always win") has four concrete exceptions in code: a user-edited issue summary is overwritten by a direct-to-DB provider apply because the edit pins `summary` while the apply gates on `description` (`crates/server/src/api/issues.rs:2786` vs `crates/server/src/metadata/apply.rs:1234-1241`, `should_apply` at `:192`); a matching non-user write silently strips the user's claim on an external ID (`crates/server/src/metadata/writers.rs:263-281`, `:326-330`); series summary/status edits write no provenance at all (`crates/server/src/api/series.rs:370,392`); and the "Use theirs" external-ID override is plumbed but never read (`apply.rs:93`, only set in tests at `:2175`). Effort S–M each; confidence high; none has a test.

2. **Make markers and progress survive archive page edits.** The archive page editor (`crates/server/src/jobs/archive_edit.rs`, `PageOp::{Remove, Reorder, …}` at `:97-100`) rewrites page order and enqueues a rescan, but contains zero references to `markers`, `progress`, or `page_index` (grep confirmed). Every bookmark, note, highlight, and reading position after the edited ordinal silently points at different pixels. `simulate_ops` (`:200`) already computes the old→new map; applying it is a medium task and closes the only silent data-loss path in the annotation system.

3. **Fix multi-device progress conflicts.** `crates/server/src/api/progress.rs:4-5` documents "resolved by `max(last_page)`"; `upsert_for` sets `last_page: Set(page)` unconditionally (`:220`). A phone's stale debounced write after a desktop read-ahead regresses the position. One comparison and a two-device test (S) fixes the single most common cross-device complaint in reader apps.

4. **Ship a user-data export/import.** There is no `GET /me/export` and no markers export (grep `path = "/me/export` and `markers.*export` in `crates/server/src/api` → none). Progress, markers, collections, ratings, and saved views live only in Postgres. A JSON dump keyed by content hash plus series name/number, with a matching importer, turns "rebuild the DB" and "migrate from Komga/Kavita" (which the spec promised as `POST /admin/import/progress`) into routine operations and gives the rare comic-side note system a durable home outside the app.

5. **Let users fix what the scanner got wrong at the series level, and make hand edits reach the archive.** `UpdateSeriesReq` (`crates/server/src/api/series.rs:197`) exposes only `match_key, slug, status, comicvine_id, metron_id, summary, reading_direction, text_language`. Series name, year, volume, publisher, imprint, age rating, and issue count cannot be edited in the app; a mis-parsed folder can only be fixed on disk. Separately, a manual issue `PATCH` never enqueues a sidecar rewrite even when writeback is on (grep `rewrite_sidecars` in `api/issues.rs` → none), so "your archives stay canonical" holds only for provider applies. Both are M.

### The biggest current risks

| Risk | Evidence | Severity |
|---|---|---|
| **Docs promise behaviour the code does not have.** File watching (`docs/dev/library-scanner.md` Triggers table, `docs/features.md`) has no implementation: no `notify` dependency in any `Cargo.toml`, `file_watch_enabled` is stored but only read to count libraries (`crates/server/src/api/server_info.rs:74`). `docs/install/backup.md:11` says Redis loss re-enqueues scans on boot; `app.rs:347-353` does not. `docs/install/scaling.md:35` claims WebSocket fan-out via Redis pub/sub; `ws_scan_events.rs` uses a process-local `tokio::sync::broadcast`. `archive_backup_retain_days` is documented as a daily prune; no code reads it beyond the API/DB round-trip. | High for operators planning on those guarantees |
| **CBT files scan, thumbnail, and then fail to open in the reader.** `page_bytes.rs:87,394` → `zip_lru.get_or_open_indexed` → `Cbz::open` only (`library/zip_lru.rs:87`), no `archive::open` fallback. Result is a 500 `archive_unreadable` on a format the scanner accepted. | Medium; narrow but user-visible |
| **Sidecar writeback can lose a file, and lose sidecars.** Crash between the two renames leaves only `.bak` + `.tmp` and startup cleanup deletes the `.tmp` (`crates/server/src/archive_rewrite/mod.rs:104-137`, `:351`). The CBZ rebuild drops every `.xml`, `.json`, `.txt`, dotfile, and `__MACOSX` entry (`crates/archive/src/cbz.rs:24-34`, `cbz_write.rs:144-150`), so CoMet.xml or embedded notes vanish; MetronInfo unknown elements are never preserved (`sidecar_compose.rs:494`); three ComicInfo fields are hard-coded to `None` and therefore deleted (`:225`, `:326-327`). When the per-issue Redis lock is busy the job returns `Ok` and drops the write with a comment that "the caller will re-enqueue" (`jobs/rewrite_sidecars.rs:103-117`); no caller does. | Medium-high for the opt-in writeback minority |
| **ACL columns that do nothing.** `library_user_access.role` (reader/curator) and `age_rating_max` exist in schema and API (`crates/entity/src/library_user_access.rs:18`) but `library/access.rs:38` checks membership only; `age_rating_max` is only ever `Set(None)` (`admin_users.rs:881`). A household admin who sets a kids' rating cap gets no enforcement. | Medium; a safety expectation |
| **Provider budgets are tightening while Folio has no visible budget and re-downloads covers on every search.** Metron cut limits to 20/min and 5,000/day in March 2026 with ETag support; Folio has no conditional requests, no cover/pHash response cache (`metadata/cache.rs` covers detail fetches only), and `fetch_and_hash_cover` builds a new HTTP client per call. | Medium; grows with library size |
| **Load behaviour at 50k+ issues is unmeasured.** `perf_regressions.rs` asserts query counts at 10×10; the `issues` row carries `comic_info_raw`, `pages`, and ~20 legacy CSV columns, and 80 `issue::Entity::find()` sites vs 24 `select_only()` projections in `api/`. The spec promises a k6 soak and `docs/dev/load-testing.md`; neither exists. | Medium; the wide row is the most probable regression vector |

---

## 2. Feature inventory

Status: **Implemented** (working, tested), **Partial** (present with a material gap), **Absent** (searched, not found), **Unverified** (could not be confirmed without runtime access or credentials).

### Reading

| Feature | Status | Evidence / gap |
|---|---|---|
| Single / double / webtoon modes, auto-detected per series | Implemented | `web/lib/reader/detect.ts:63-98`, `store.ts:48` |
| Double-page spread awareness (ComicInfo `DoublePage` + aspect ≥1.2), cover-solo offset | Implemented | `web/lib/reader/spreads.ts:26-45,80-112`; `ReaderSettings.tsx:120-132` |
| Manual spread override / pairing shift | Absent | grep `double_page` outside the read route → none; no PATCH on `issues.rs` |
| Fit width / height / original | Implemented | `Reader.tsx:793-803` |
| Fit-to-screen (both axes), custom zoom % | Absent | `FIT_MODES` has three values (`store.ts:21`) |
| RTL/manga direction chain (issue → series → user → library → LTR) with scanner auto-pin at ≥80% | Implemented | `detect.ts:36-49`; `scanner/metadata_rollup.rs:561-615`; tests `reading_direction.rs` |
| RTL mirroring of progress bar | Partial | `ReadingProgress.tsx:323` is `origin-left`, takes no direction |
| Series `ttb` direction | Partial | server accepts it; web treats as no opinion (`detect.ts:30`) |
| Transform zoom (ladder, double-tap, drag-pan) | Partial | single view only (`Reader.tsx:566-651`); none in double (`:1400-1401`) or webtoon (`:1684`); resets every page (`:644-651`) |
| Mouse wheel / trackpad zoom | Absent | grep `wheel` in `web/lib/reader` + read route → only `PageStrip.tsx:258-279` |
| Guided / panel view | Absent | grep `guided\|panel zoom` across web + server + docs → one unrelated doc hit |
| Rebindable keyboard map + live shortcut sheet | Implemented | `web/lib/reader/keybinds.ts:149-193`; `ShortcutsSheet.tsx` |
| Touch: tap zones, swipe, native pinch preserved, coarse-pointer fallback | Implemented | `Reader.tsx:1719-1783`; `use-swipe.ts`; `coarse-pointer.ts` |
| iOS edge-swipe-back mitigation | Absent | no `popstate`/edge inset; `docs/dev/pwa-hardening.md` lists it untested |
| Progress: debounced, keepalive flush, sticky finished, incognito, peek | Implemented | `use-progress-write.ts`; `progress.rs:205-216` |
| Multi-device conflict rule | **Partial (bug)** | doc says `max(last_page)`; code is last-write-wins (`progress.rs:220`) |
| Durable offline outbox for progress | Absent | memory-only buffer (`progress-writer.ts:175-221`) |
| Up Next / end-of-issue card, CBL-aware resolver, Shift+N/P | Implemented | `next_up.rs`; `EndOfIssueCard.tsx` |
| Reading sessions, stats, streaks, log | Implemented | `reading_sessions.rs`, `reading_log.rs`, `admin_stats.rs` |
| Page prefetch + decode, width-negotiated WebP variants, Range/ETag | Implemented | `use-prefetch.ts`; `page_bytes.rs:331-460` |
| Offline reading (page bytes cached) | Absent (planned) | `web/app/sw.ts` never caches page bytes; `docs/dev/pwa-offline-reading-plan.md` six steps, none started |
| PWA install, safe areas, wake lock, update flow | Implemented | `InstallEvents.tsx`, `SafeAreaProbe.tsx`, `use-wake-lock.ts`; icons declared but PNGs missing |
| Tap-to-OCR speech bubbles (EN Tesseract, JA manga-ocr) | Implemented | `crates/server/src/ocr/`, `api/issue_ocr.rs`; `docs/dev/ocr.md` |

### Metadata and providers

| Feature | Status | Evidence / gap |
|---|---|---|
| ComicVine + Metron providers, priority fan-out, composite merge | Implemented | `metadata/comicvine.rs`, `metron.rs`, `orchestrator.rs:78-112`, `merge.rs` |
| GCD / Marvel / LOCG / AniList / MangaUpdates providers | Absent | `identifier.rs:19-33` reserves ID namespaces only; two `impl MetadataProvider` in tree; the GCD plan file referenced in memory does not exist on disk |
| Cover-pHash primary matching, ComicTagger ladder, gap guard | Implemented | `matcher.rs:151-167`; `orchestrator.rs:279-301`; golden suite (24 cases) |
| Pre-filter (year gate, publisher blacklist) | Implemented | `orchestrator.rs:435-467` |
| User-editable search query / search by provider URL | Absent | `metadata_search.rs:288-293` builds facts from the row; `CandidatesQuery` has only `run_id` |
| Format / series-type awareness (TPB, annual, one-shot) | Absent | `GenericMetadata.series_type/format` never populated; no golden fixtures |
| Manual match dialog with per-field preview diff and provenance | Implemented | `metadata_search.rs`; web dialogs under `web/components/library` |
| Batch matching (selection, saved view, review queue) | Implemented | `metadata_search.rs:1935-2765` |
| Scanner-triggered search | Absent | `trigger_kind::SCANNER` defined, never enqueued |
| Auto-apply on single strong match (weekly refresh / batch) | Implemented | `jobs/metadata_search.rs:554-727`, FillMissing + no override (`:595-610`) |
| Weekly refresh (off by default) | Implemented | `refresh.rs`, `scheduler.rs` |
| Rate-limit buckets (Redis token buckets, velocity floor), quota park + resume | Implemented | `comicvine.rs:58,165`; `orchestrator.rs:685-716`; `metadata_resume.rs` |
| `Retry-After` parsing, 5xx retry/backoff, connect timeout, JSON body cap | Absent | both clients hardcode `retry_after_secs: 60` on 429 (`comicvine.rs:211-214`, `metron.rs:187-195`); 30 s total timeout only; `resp.text()` unbounded; no `RetryLayer` anywhere |
| Conditional requests (ETag) to Metron | Absent | grep `If-None-Match\|ETag` in `metron.rs` → none |
| Visible per-provider budget in admin | Partial | `admin_metadata.rs` shows configured/enabled + test; no remaining-quota bar |
| Field provenance (user > provider > file) | **Partial (bugs B1–B7 in §5.2)** | `writers.rs:497-590`; exceptions listed in §1 |
| Sidecar writeback (ComicInfo + MetronInfo, atomic, `.bak`, scoped rescan) | Partial | works for CBZ; fails at open for CBR/CBT (`rewrite_sidecars.rs:288`); sidecar and unknown-element loss; lock-busy drops |
| Series-boundary divergence (`series_provider_range`, auto-split) | Implemented | `range_map.rs`, `auto_split.rs` |
| Cover download SSRF guard, size cap, decode limits | Implemented | `util/ssrf.rs:113-213` (24 MiB); `util/image_decode.rs:52-76` |
| Cover content-type / magic check | Absent | headers captured, never inspected; bytes written before decode (`writers.rs:1665`) |

### Library organization and management

| Feature | Status | Evidence / gap |
|---|---|---|
| CBZ read/write, CBT read/write (editor), CBR read + convert-to-CBZ | Implemented | `crates/archive/`, `scanner/cbr_convert.rs` |
| CBT pages in the web reader | **Absent (bug)** | `zip_lru.rs:87` opens `Cbz` only |
| CB7 | Absent (flagged) | `cb7.rs:26-31` stub |
| PDF / EPUB / loose folders | Absent (by decision) | spec §1.2 |
| Flat + publisher-nested layouts, specials subfolders, `series.json` | Implemented | `scanner/enumerate.rs`, `parsers/series_json.rs` |
| Incremental scan (folder mtime, size+mtime fingerprint, BLAKE3) | Implemented | `enumerate.rs:123`; `process.rs:164-175` |
| Content-hash move/retag survival | Implemented | `process.rs:536,880-892`; `scanner_retag.rs` |
| File watching | **Absent (documented as present)** | no `notify` dep; column unused |
| Cron scans, scan-on-startup, cancel, live WS progress, scan batches | Implemented | `scheduler.rs`; `scan_runs.rs:625-735`; `ws_scan_events.rs` |
| Library health issues (10 kinds live, 4 dead stubs) | Partial | `library/health.rs`; `FolderNameMismatch`, `MixedSeriesInFolder`, `AmbiguousVolume`, `OrphanedSeriesJson` never emitted |
| Soft-delete + restore; hard purge | Partial | sweep confirms but never purges |
| Duplicate detection | Partial | exact hash only, not library-scoped; `dedupe_by_content` flag never read; no same-series+number or near-duplicate cover surface |
| Per-issue edit (~37 fields), bulk edit, field pins | Implemented | `issues.rs:514-600`, `:1470` |
| Series identity edit (name/year/volume/publisher…) | Absent | `UpdateSeriesReq` at `series.rs:197` |
| Manual edit → archive writeback | Absent | no rewrite enqueue from `api/issues.rs` |
| Archive page editor (remove/reorder/rotate/replace, bulk ops, `.bak` restore) | Implemented | `api/archive_edit.rs`, `jobs/archive_edit.rs` |
| Marker/progress remap after page edit | **Absent (bug)** | no `marker`/`page_index` reference in the edit job |
| Collections (series + issues, reorder), Want to Read | Implemented | `collections.rs` |
| CBL lists: import file/URL/catalog, 3-tier match, refresh diff, export | Implemented | `cbl/`, `cbl_lists.rs` |
| Saved smart views (series-level, rich field/op set), rails, custom pages | Implemented | `views/dsl.rs`, `compile.rs:77`; issue-level views absent |
| Ratings (half-star), reviews | Partial | ratings yes; review text absent (grep `tiptap\|review_body` → none) |
| Search: PG FTS weights + trigram, snippets, palette | Implemented | `docs/dev/search.md`, `m20260301_000001_search_docs.rs` |
| Browse pages for characters / teams / arcs / publishers | Absent | only `/creators/[slug]` in route tree |
| Similar series / recommendations | Absent | grep `similar\|recommend` in `series.rs`, `rails.rs` → comment only |
| Series relationships (Phase 7) | Absent | no entity |
| Multi-user per-library ACL | Implemented (membership) / Absent (role, age cap) | `access.rs:38` |
| Collection / view sharing | Absent | no sharing columns |

### Annotation

| Feature | Status | Evidence / gap |
|---|---|---|
| Bookmarks, Markdown notes, region highlights, favourites, tags | Implemented | `crates/entity/src/marker.rs`; `api/markers.rs` |
| Region → OCR text capture, bubble snapping, re-detect | Implemented | `MarkerOverlay.tsx:447-490`; `marker-selection.ts:111-170` |
| Editable captured text | Absent | editor only refills from OCR; cap 1120 bytes (`markers.rs:645`) |
| Global Bookmarks page (kind/tag/text filters, bulk delete + Undo, crop copy/save) | Implemented | `web/components/markers/MarkersList.tsx` |
| In-reader marker list / drawer | Absent | only `]`/`[` (bookmark kind) and strip dots |
| Per-series / per-issue notes surface | Absent | `ListQuery` has `issue_id` (unexposed), no `series_id` |
| Notes in global search + reading log | Implemented | `use-search.ts:295-400`; `reading_log.rs:196-205` |
| Smart-view filter "has notes / bookmarked" | Absent | grep `has_note\|has_bookmark\|marker_count` → only a user pref |
| Notes export (Markdown/JSON) | Absent | reading-log CSV carries kind/tags only (`reading_log.rs:762-880`) |
| Marker permalink / share | Absent | `issue_permalink.rs` is issue-level |
| Anchoring robustness (page hash) | Absent | anchor is `(user, issue_id, page_index)` + % rect only |

### Interop, operations

| Feature | Status | Evidence / gap |
|---|---|---|
| OPDS 1.2 + 2.0, PSE signed URLs, Progression 1.0, personal feeds | Implemented | `opds.rs`, `opds_v2.rs`, `opds_pse.rs`, `opds_progression.rs`; 14 test files |
| Komga-compat shim (Panels), KOReader sync | Implemented | `komga_compat.rs`; `opds.rs:3697` |
| App passwords with scopes | Implemented | `app_passwords.rs` |
| Named devices / last-seen on API keys | Partial | last-used tracking exists; no device naming |
| `GET /me/export` / `POST /admin/import/progress` / Komga-Kavita importers | Absent | grep → none |
| Backup docs (PG nightly, `/data` weekly, secrets), upgrade + rollback docs | Implemented | `docs/install/backup.md`, `upgrades.md` |
| `just backup` / `just restore` recipes | Absent | `justfile` has none |
| Auto-migrate on boot, boot guards for regenerated secrets, `/readyz` | Implemented | `app.rs:172-233`; `health.rs:60-85` |
| PG ≥17 boot guard | Absent | grep `server_version_num` → none |
| Migration `down` exercised in CI | Absent | grep `migrate-down\|Migrator::down` in `ci.yml` → none |
| Prometheus metrics, ring-buffer logs, library events, admin dashboards | Implemented | `docs/dev/metrics.md`; `observability.rs` |
| Docker images: distroless non-root, cosign, SBOM, healthchecks | Implemented | `Dockerfile`, `release.yml`, `compose.prod.yml` |
| arm64 image | Absent | `release.yml:1-3` amd64 only |
| Multi-replica support | Absent (documented as possible) | in-process scheduler/mutexes/broadcast; `scaling.md` overstates |
| Load / soak tests | Absent (promised) | spec §16.5/§18.3; no `k6`/`oha` anywhere |

---

## 3. Workflow review

### 3.1 Import

**Observed.** A first import of a large collection is a strong experience up to the point where something needs fixing. The scanner is I/O-bound (~94–104 files/s on NVMe, `docs/dev/scanner-perf.md`), coalesces concurrent triggers, batches per series with `synchronous_commit = OFF` (`scanner/mod.rs:2057-2116`), and streams live progress over WebSocket. Health issues are typed, fingerprinted, auto-resolving, and surfaced in two admin views. Soft-deletion protects read state when a NAS unmounts.

**Inference.** At 100k issues on NVMe a cold scan is roughly 16–18 minutes; on spinning disks or a NAS the BLAKE3 pass dominates and could be hours, and there is no "trust size+mtime on first import, hash lazily" mode. The four dead health kinds (`FolderNameMismatch`, `MixedSeriesInFolder`, `AmbiguousVolume`, `OrphanedSeriesJson`) are exactly the problems a large mixed-provenance library has, so those errors surface as silently wrong series rather than as findings. `AmbiguousFolder` skips a subtree with one health row and no preview of what was skipped. The documented file watcher does not exist, so a user who copies files in and waits for them to appear will wait until the next cron tick.

**CBT** files ingest and thumbnail but 500 in the reader (`zip_lru.rs:87`). CBR files are either converted (opt-in) or flagged; there is no native RAR page streaming.

### 3.2 Metadata matching

**Observed.** The matcher is the strongest part of the product: cover-pHash primary, ComicTagger's exact ladder, a runner-up gap guard, per-provider pre-filters, composite merge across providers, per-field preview diff, provenance display, batch queues with review, and quota parking. The golden suite pins 24 known-good/known-bad cases.

**Inference on wrong matches.** The remaining wrong-match vectors are structural rather than tuning: (a) the query is built from the local row with no user override, so a folder year more than one year ahead of the true volume drops the correct candidate at the hard gate with no escape hatch (`orchestrator.rs:435-467`); (b) neither provider's series type / format is read, so TPBs, HCs, and omnibuses compete with the ongoing series on name alone, and manga "Vol. 3" folders parse as issue 3; (c) annuals are separate ComicVine volumes and `canonical_issue_number` leaves "Annual 1" verbatim, so the number component scores zero against "1"; (d) a local variant cover whose alternate is not in the first three `associated_images` gets a cover veto against a correct candidate; (e) no language awareness anywhere in the matcher.

**Observed on overwrite safety.** See §5.2. The pipeline honours user pins in the common path but has seven precise exceptions, three of which (B1, B3, B4) will bite an ordinary user without any admin override.

### 3.3 Organization

**Observed.** Collections, Want to Read, CBL lists with a catalog browser and refresh diffs, series-level smart views with a genuinely rich operator set, pinnable rails, custom pages, and multi-select bulk actions. This is ahead of Komga and roughly level with Kavita's 0.9 reading-list overhaul.

**Inference.** Three edges: smart views are series-only (`compile.rs:77`), so "unread annuals from 2019" or "issues I rated ≥4" cannot be expressed; there are no browse pages for characters, teams, arcs, or publishers despite the junctions and filter chips existing; and duplicate management is a single exact-hash health row with no page to act on. The `dedupe_by_content` library flag is stored and ignored.

### 3.4 Reading

**Observed.** Modes, spreads, RTL, zoom, prefetch, variants, sessions, and the CBL-aware Up Next card are all real and unit-tested. iOS handling is thorough.

**Inference.** For a phone or tablet user the two gaps that matter are last-write-wins progress and the memory-only failed-write buffer; for a desktop user it is the absence of pointer zoom and fit-to-screen; for a manga reader it is the absence of any manual spread control when `<Pages>` is missing or a scan is offset by one. Offline reading is planned but not started. The reader bundle is ~190 KB gzip against a 150 KB budget (`docs/dev/pwa-performance.md`), while `docs/features.md` still claims ~118 KB.

### 3.5 Annotation

**Observed.** Markers are the rare comic-side annotation system (Kavita's annotations are EPUB-only; Komga has none). Four kinds, region highlights with OCR capture, tags, a global page with text search, Undo on delete, reading-log integration, strict per-user isolation, 17 server tests + 8 schema tests + 37 web tests.

**Inference.** The system is anchored purely to `(issue_id, page_index)` plus a percentage rectangle. That survives retag and move (the scanner keeps `issues.id` stable) but not a page reorder, page removal, a replaced archive with different ordering, or a ComicInfo `<Pages>` reorder. Nothing detects the drift; out-of-range markers still list and their Jump link clamps to the last page. There is no export, no per-series view, no editable captured text, and no in-reader list. The `/me/markers` list also skips the library-ACL re-check that `reading_log.rs` performs, so a revoked grant leaves markers visible with dead links.

### 3.6 Discovery and retrieval

**Observed.** Postgres FTS with field weights and trigram fallback, snippets, people search, command palette, extensive grid filters and sorts, random, recently-added rails and OPDS feeds.

**Inference.** Adequate through 100k issues without Meilisearch. The gaps are navigational (entity landing pages), not engine-level. "Similar series" and recommendations are absent and, for a single-taste personal library, not worth building; a "more by this creator / in this arc" rail on the series page would capture most of the value at low cost.

### 3.7 Backup, export, migration

**Observed.** `docs/install/backup.md` is honest and complete about Postgres, `/data`, and secrets, including a restore drill. Boot refuses to start if the pepper or settings key was regenerated against existing rows (`app.rs:180-233`), which is an excellent operator guard.

**Inference.** The weak point is that everything a user creates (progress, markers, collections, ratings, views, sidebar layout) exists only in Postgres with no application-level export. A `pg_dump` is a backup, not a migration: it cannot survive a library re-import that changes issue ids, and it cannot import from Komga or Kavita. The reading-log CSV and CBL exports cover a fraction. Codex ships a user-data sidecar for exactly this; YACReader 10.2 added library backup/restore. Folio's content-hash issue ids make a portable format easy: key by `content_hash` with `series name + number` as fallback.

---

## 4. Findings by area

### 4.1 User experience

- **UX-1 (Observed).** Progress conflict is last-write-wins despite the module doc (`progress.rs:4-5` vs `:220`). No two-device test exists.
- **UX-2 (Observed).** No manual spread override; pairing depends entirely on ComicInfo `DoublePage` or aspect ratio (`spreads.ts:26-45`).
- **UX-3 (Observed).** Zoom is single-view only, resets on every page (`Reader.tsx:644-651`); no ctrl+wheel/trackpad zoom, so desktop pinch becomes browser zoom and breaks the fixed chrome.
- **UX-4 (Observed).** No fit-to-screen; a portrait page in landscape phone orientation body-scrolls in width-fit.
- **UX-5 (Observed).** Progress bar not mirrored in RTL (`ReadingProgress.tsx:323`); series `ttb` accepted by server but ignored by the web chain (`detect.ts:30`).
- **UX-6 (Observed).** Series identity fields are not editable in-app (`series.rs:197`).
- **UX-7 (Observed).** No character/team/arc/publisher landing pages; only `/creators/[slug]`.
- **UX-8 (Observed).** Smart views are series-only (`views/compile.rs:77`); no `is_empty`, `ends_with`, story-arc, format, special-type, or rating fields; one nesting level.
- **UX-9 (Observed).** No duplicates page; `DuplicateContent` is a health row only and not library-scoped (`process.rs:536`).
- **UX-10 (Observed).** iOS standalone edge-swipe-back is unmitigated (`docs/dev/pwa-hardening.md` device checklist).
- **UX-11 (Observed).** Reader JS is ~190 KB gzip vs the 150 KB budget; `features.md` claims ~118 KB.
- **UX-12 (Observed).** Markers: no export, no per-series/issue surface, no in-reader list, captured text not editable and capped at 1120 bytes (`markers.rs:645`), `MarkerView.kind` doc-comment stale (`:113`).
- **UX-13 (Observed).** `MetadataSearch` has no editable query and no paste-a-provider-URL path (`metadata_search.rs:288-293`).

### 4.2 Data and integrations

- **DI-1 (Observed, high).** Summary/description key mismatch overwrites user-edited issue summaries on direct-to-DB apply (`issues.rs:2786` pins `Summary`; `apply.rs:1234-1241` gates `Description`; `should_apply` exact-matches at `:192`). The sidecar path (`apply.rs:2093-2096`) and scanner (`process.rs:868-872`) alias correctly.
- **DI-2 (Observed, high).** `put_external_id` skips only when the incoming value differs (`writers.rs:269-271`); a matching non-user write rewrites `set_by` (`:326-330`), demoting the user's claim. The skip also returns `Set` (`:279`), which the apply reports as "added" (`apply.rs:1976-1980`).
- **DI-3 (Observed, high).** Series `summary`/`status` edits write no provenance (`series.rs:370,392`); a `replace_all` series apply overwrites them. Status is guarded only by `status_user_set_at` in `reconcile_status.rs`.
- **DI-4 (Observed, medium).** `override_external_id_sources` ("Use theirs") is never read in `apply.rs` (`:93`, set only in tests at `:2175`).
- **DI-5 (Observed, medium).** `number_raw` is gated by `MetadataField::Format` (`apply.rs:1166-1173`): a format pin blocks issue-number updates, a user edit to the number is unprotected, and a spurious Format provenance row is written.
- **DI-6 (Observed, medium).** Scanner always overwrites `scan_information`, `community_rating`, `review` from the file (`process.rs:988-990`), and rebuilds junctions from CSV read-cache columns with no provenance check (`metadata_rollup.rs:252-290` via `process.rs:1062`). Provider-written credits (`person_id` UUID, ordinal) are replaced by rollup rows (`person` name, `person_id NULL`, ordinal 0) on any content change or force scan, and the two sets never compare equal so they churn every time.
- **DI-7 (Observed, medium).** Edit endpoints write provenance after the row update, outside a transaction, ignoring failures (`issues.rs:~920-935`, `~1733-1751`). Inference: a failed provenance write means the next rescan undoes the edit.
- **DI-8 (Observed).** `issue.user_edited` is still read at `process.rs:862`, `issues.rs:305,754,1085,1719`, `series.rs:992` despite being marked retired; the scanner honours it only for nine fields (`process.rs:898-993`).
- **DI-9 (Observed).** Sidecar apply ignores `ApplyRequest.mode` (no `args.mode` in `apply_issue_via_sidecar` or `sidecar_compose`), so `fill_missing` behaves as provider-wins except for pins.
- **DI-10 (Observed).** Writeback fails at open for CBR/CBT (`rewrite_sidecars.rs:288` always `Cbz::open`; dispatch at `apply.rs:371,1098` does not check format) but provenance, variants, and `last_metadata_sync_at` are written at apply time regardless (`apply.rs:961-1010`), so the DB attributes provider values that never reached the XML. The XML-first path also never fetches the primary cover (`:961-993`).
- **DI-11 (Observed).** Sidecar loss on rewrite: CoMet.xml / `.json` / `.txt` dropped (`cbz.rs:24-34`); MetronInfo unknown elements dropped (`sidecar_compose.rs:494`); `MainCharacterOrTeam`, `AlternateNumber`, `AlternateCount` hard-coded `None` and deleted (`:225,:326-327`).
- **DI-12 (Observed).** `archive_backup_retain_days` has no prune implementation; only `retain_count` (0–5) rotates.
- **DI-13 (Observed).** Lock-busy or Redis-error sidecar jobs return `Ok` and drop the write (`rewrite_sidecars.rs:103-117`); the 120 s `SET NX EX` lock (`archive_rewrite/mutex.rs`) has no heartbeat and is not taken by the scanner, CBR convert, or thumbnail readers.
- **DI-14 (Observed).** Drift detection is timestamp-only, issue-level pins only, and `last_rewrite_at` is also stamped by page edits (`archive_edit.rs:545`), hiding metadata drift after a page edit.
- **DI-15 (Observed).** Covers: bytes written before decode, no content-type/magic check, extension guessed from URL (`writers.rs:1626-1686`); deactivated primary-cover files are never deleted while the issue is active; variant re-apply deletes local files before downloading replacements (`:1922-1928`); variant backfill drain stops after a pass that stores zero (`backfill.rs:109`).
- **DI-16 (Observed).** No provider-response or cover/pHash caching for searches; every search re-downloads up to 25 candidate covers per provider; `fetch_and_hash_cover` ignores its client argument. No Metron ETag conditional requests. Metron's March 2026 limits (20/min, 5,000/day) make this a real budget cost.
- **DI-17 (Observed).** `trigger_kind::SCANNER` is defined but never enqueued; scans never auto-search.
- **DI-18 (Observed).** `docs/dev/metadata-operator-guide.md` states the HIGH threshold default is 95; `config.rs:299` is 80.
- **DI-19 (Observed).** Markers and progress are not remapped by the archive page editor (`jobs/archive_edit.rs` has no `marker`/`page_index` reference; `simulate_ops` at `:200` already has the map).
- **DI-20 (Observed).** `GET /me/markers` does not join `issue.removed_at` nor re-check library grants; `reading_log.rs` does (test at `:676-694`).
- **DI-21 (Observed).** Duplicate content lookup is not library-scoped; `library.dedupe_by_content` is never read.
- **DI-22 (Observed).** Provider HTTP resilience is thinner than the docs suggest. Both clients are `reqwest::Client::builder().user_agent(..).timeout(30s)` with no connect timeout and the default redirect policy (`comicvine.rs:102-104`, `metron.rs:87-89`); base URLs are compile-time constants. A 429 maps to a hardcoded `retry_after_secs: 60` and the `Retry-After` header is never read (`comicvine.rs:211-214`, `metron.rs:187-195`); ComicVine's `status_code=107` maps to a hardcoded 3600 s. 5xx returns `Upstream` with zero retries. API JSON bodies are read unbounded via `resp.text()` (covers are capped at 24 MiB). apalis retry (`max_attempts=5` library default) and the dead-letter ZSET are effectively unreachable for provider failures because `handle_series`/`handle_issue` and the apply handler log and `return Ok(())` (`jobs/metadata_search.rs:163-172,239-248`; `jobs/metadata_apply.rs:172-179,412-419`); quota recovery goes through the run-level `awaiting_quota` park instead. Tests cover the 429/107 mapping and the all-providers-exhausted park, but nothing covers 5xx, timeouts, `Retry-After`, or retry exhaustion.

### 4.3 Architecture

- **AR-1 (Observed).** Wide `issues` row (`comic_info_raw`, `pages`, ~20 legacy CSV columns) with 80 `issue::Entity::find()` sites vs 24 `select_only()` in `api/`. Inference: the most probable 50k-issue regression vector.
- **AR-2 (Observed).** `[locale]` route segment and `next-intl` middleware run on every request for a single `en` locale (`web/i18n/request.ts`). The i18n decision is "keep, wire later" per memory; the segment is dead weight until then.
- **AR-3 (Observed).** apalis retry policy is implicit (comments say 5 attempts, `reenqueue_orphaned_after` 300 s at `jobs/mod.rs:257,423`; grep `RetryPolicy\|max_attempts` → none). Dead-letter tooling in `admin_queue.rs` is good.
- **AR-4 (Observed).** In-process cron scheduler with no leader lease (`jobs/scheduler.rs:23-42`); in-process mutexes/semaphores (`state.rs:57-116`); process-local WS broadcast. The design is single-instance; `docs/install/scaling.md` implies otherwise.
- **AR-5 (Observed).** No `TimeoutLayer`, no `CompressionLayer` (tower-http feature enabled, unused), no `DefaultBodyLimit` override, no global API limit.
- **AR-6 (Observed).** `issue.user_edited` retirement unfinished (40 references).
- **AR-7 (Observed).** 23 stringly `String` enum fields in entities (`user.role`, `user.state`, `issue.state`, `library_user_access.role`, `marker.kind`) vs 71 `DeriveActiveEnum` uses; `marker.kind` is a CHECK constraint on text.
- **AR-8 (Observed).** Only 2 bounded `string_len()` columns vs 188 unbounded; `marker.color` uncapped.
- **AR-9 (Observed).** Migration `down` functions exist (98) but are never exercised in CI.

### 4.4 Operations

- **OP-1 (Observed, high).** Documentation drift on resilience guarantees: file watcher (`library-scanner.md`, `features.md`), Redis re-enqueue on boot (`backup.md:11` vs `app.rs:347-353`), Redis pub/sub for WS (`scaling.md:35`), `retain_days` prune, `metadata_refresh` scan mode (doc vs `ScanMode` enum at `libraries.rs:392`), CBR row in the health table, `archive_edit.rs` "CBZ only" header, reader bundle size.
- **OP-2 (Observed).** No PG version boot guard although PG <17 breaks the search migration (`CLAUDE.md`; grep `server_version_num` → none).
- **OP-3 (Observed).** amd64-only images (`release.yml:1-3`); the Raspberry Pi / Apple-silicon homelab audience is excluded.
- **OP-4 (Observed).** No `just backup`/`just restore`; the documented commands are manual.
- **OP-5 (Observed).** `audit_log` has no retention (append-only by design, unbounded). Thumbs directory has no byte budget or gauge; variant cache does.
- **OP-6 (Observed).** Load/soak testing promised in spec §16.5/§18.3 and `docs/dev/load-testing.md`; neither exists. `fixtures/build.py --scale stress` exists and is unused for this.
- **OP-7 (Observed).** Thumbs served `private, no-cache` (revalidate every paint); provider covers `public, immutable` but ACL-gated so the header is browser-only.
- **OP-8 (Observed).** `.tmp` startup cleanup deletes the only remaining copy after a crash between the two rewrite renames (`archive_rewrite/mod.rs:104-137,351`).

### 4.5 Accessibility

- **AC-1 (Observed).** Solid baseline: live-region page announcements (`Reader.tsx:936-940`), `inert` hidden chrome, labelled icon buttons, `role=progressbar`, focus-managed end card, keyboard proxies for saved regions, 19 reduced-motion sites, `forced-colors` block.
- **AC-2 (Observed).** Chrome is hidden by default (`store.ts:207-209`); a screen-reader user must know `t`.
- **AC-3 (Observed).** Detected OCR bubbles are pointer-only (`MarkerOverlay.tsx:540,577-590`); pages expose `alt="Page N"` with no OCR-derived text alternative even though the pipeline exists.
- **AC-4 (Observed).** Seven `text-[10px]/[11px]` labels in reader chrome.
- **AC-5 (Observed).** axe runs only on `/sign-in` (`web/tests/e2e/a11y.spec.ts`); the reader axe pass is "deferred". Spec §Phase 3.5 calls WCAG 2.2 AA a hard gate.

### 4.6 Security

- **SE-1 (Observed).** Closed from `docs/dev/security-audit.md`: H-1 SSRF (`util/ssrf.rs`, `cbl_lists_ssrf.rs`), H-2 `Config` Debug (`config.rs:322`), M-3 key permissions (`secrets.rs:230`). The doc has no status column, so this is only knowable from code.
- **SE-2 (Observed).** Still open: M-1 no `CorsLayer` (grep → none; same-origin so low risk), H-3.2 OPDS shared bucket, M-2 `/auth/config` issuer exposure (not re-verified), M-5 per-user CBL import quota (grep `quota` → none), L-1 pepper rotation, L-3, L-4.
- **SE-3 (Observed).** `override_user_edits` admin check is inline `user.role != "admin"` → 403 at five sites (`metadata_search.rs:1086,1427,1755,1832,2788`) rather than the structural `RequireAdmin` extractor; composite apply audits override only in the payload, never as `_force`, and the dashboard counter excludes it (`admin_metadata.rs:384-391`).
- **SE-4 (Observed).** `library_user_access.age_rating_max` and `role` are unenforced (§1). For a family server this is the most consequential gap.
- **SE-5 (Observed).** Marker endpoints have no rate limit (`grep -rln rate_limit crates/server/src/api/*.rs` excludes `markers.rs`); `create` takes `Json<T>` not `Validated<T>`; region `w/h` may be 0 server-side.
- **SE-6 (Observed).** Cover fetch allows `http://` (`ssrf.rs:115-117`, `require_https=false`) and performs no content-type check; a non-image body is stored on disk and served with a MIME derived from the URL's extension (`thumbnails.rs:382-400`). Bounded by the 24 MiB cap and the ACL on the route; low severity, but it is the one path where untrusted bytes are persisted without sniffing.
- **SE-7 (Observed).** Strong: argon2id + pepper, Ed25519 JWT + rotating refresh with reuse detection, `__Host-` cookies, double-submit CSRF, per-request CSP nonce + `strict-dynamic`, HSTS, IP lockout, HMAC-signed OPDS page URLs, path-traversal guard in `admin_fs`, archive limits, secrets-regeneration boot refusal, audit log + CI enforcement, cargo-deny/audit/trivy/daily advisories.

---

## 5. Prioritized recommendations

Priority reflects expected user value × frequency × severity ÷ cost, with dependencies noted. Confidence is in the finding and the proposed fix, not in the effort estimate.

| # | Pri | Feature or fix | User problem | Evidence | Proposed behaviour | Implementation outline | Dependencies | Effort | Conf. |
|---|---|---|---|---|---|---|---|---|---|
| R1 | P0 | Close provenance holes B1–B6 | Hand edits get silently overwritten | DI-1..DI-5 | User pins always win unless admin `override_user_edits`; skip is reported as skipped | Alias `Summary`↔`Description` in `should_apply`; compare provenance not value in `put_external_id` and return `Skipped`; write `SetBy::User` rows in `update_series`; read `override_external_id_sources`; gate `number_raw` on its own field; add one failing test per bug in `metadata_apply.rs`/`metadata_writers.rs` | none | S–M | High |
| R2 | P0 | Remap markers + progress on archive page edit | Bookmarks/notes/positions point at wrong pages after reorder/remove | DI-19 | Ordinals remapped in the same transaction as the rewrite; markers on removed pages flagged, not orphaned; post-rescan sweep surfaces `page_index ≥ page_count` as a health note | Apply `simulate_ops` map to `markers.page_index` and `progress_records.last_page` in `jobs/archive_edit.rs`; add `markers_archive_edit.rs` test | none | M | High |
| R3 | P0 | Server-side progress conflict rule | Phone regresses desktop position | UX-1 | `max(last_page)` unless explicit `?from=start`/mark-unread; or client timestamp compare | One comparison in `upsert_for` (`progress.rs:220`); two-device test | none | S | High |
| R4 | P0 | Fix CBT reader path | CBT scans then 500s | Inventory | Reader streams CBT like CBZ, or scanner refuses CBT | Route `zip_lru` through `archive::open` with a non-indexed fallback, or drop `cbt` from `is_recognized_archive_ext`; add a CBT page-stream test | none | S | High |
| R5 | P0 | Docs/code truth pass | Operators plan on false guarantees | OP-1, DI-12, DI-18 | Each doc claim either backed by code or removed | Remove watcher claims and `file_watch_enabled` (or build R14); fix `backup.md` Redis sentence; declare single-instance in `scaling.md`; implement or drop `retain_days`; fix threshold default; refresh scanner/health/archive_edit comments; fix bundle-size claim | none | S | High |
| R6 | P1 | `GET /me/export` + `POST /me/import` (and admin progress import) | No way to move or rebuild user data | §3.7 | JSON dump of progress, markers, collections, ratings, views, sidebar keyed by `content_hash` + `(series, number)` fallback; import with dry-run report; documented shape doubles as the Komga/Kavita import format | New `api/account_export.rs`; reuse `hydrate_views`; streaming JSON; `just backup` wrapper | R1 (so imported provenance is honoured) | M | High |
| R7 | P1 | Series identity edits | Mis-parsed folder can only be fixed on disk | UX-6 | Edit name/year/volume/publisher/imprint/age rating/total issues with `SetBy::User` pins; scanner and apply respect them | Extend `UpdateSeriesReq` + `SeriesEditDrawer`; write provenance rows; extend `fetch_user_pinned_fields` for series; test | R1 | M | High |
| R8 | P1 | Manual edit → sidecar writeback | "Archives stay canonical" only for provider applies | §3.2 | When both writeback flags are on, an issue `PATCH`/bulk edit enqueues `RewriteIssueSidecarsJob` | Enqueue from `api/issues.rs` after commit; coalesce per issue; respect `skip_rescan` conventions | R9 (so the rewrite is safe) | M | High |
| R9 | P1 | Writeback hardening | Crash can lose the file; sidecars and unknown fields lost; dropped jobs | DI-10..DI-14, OP-8 | Rename order that never leaves the target missing; preserve non-ComicInfo sidecars and MetronInfo raw; stop hard-coding three ComicInfo fields; re-enqueue on lock-busy; refuse or convert CBR/CBT before apply; write provenance only after a successful rewrite | Reorder to `target→.bak` only after `tmp→target` via a hard-link or `renameat2`-style swap where available; carry unknown entries through `cbz_write`; add raw passthrough to MetronInfo serializer; make `rewrite_sidecars` return `Err` to apalis on lock-busy | none | M | High |
| R10 | P1 | Enforce `age_rating_max` (and decide `curator`) | Kids' library cap does nothing | SE-4 | Series/issue lists, search, OPDS, page bytes, and covers filter by the grant's cap; unrated content policy configurable | Extend `VisibleLibraries` to carry caps; add a WHERE in `series::list`, `issues`, search UNION arms, OPDS; tests; drop `curator` or gate metadata/archive edits on it | none | M | High |
| R11 | P1 | Notes export + per-series/issue notes surface + editable captured text | Notes are locked in; OCR mistakes uncorrectable | UX-12 | `GET /me/markers/export` with `format=md` or `json`; "Notes" tab on series/issue pages; editable `selection.text` with a few-KB cap; marker permalink | Add `series_id` to `ListQuery`; reuse `MarkersList` with preset filter; raise cap in `markers.rs:645`; `GET /markers/{id}` → 303 | R6 shares the export shape | S–M | High |
| R12 | P1 | Match-query override + search-by-provider-URL + year-gate escape | "No match" dead ends | UX-13, §3.2 | Editable name/year/publisher/number in the dialog; paste a CV/Metron URL to fetch directly; if the hard gate empties the list, retry `PhashAware` | Optional overrides on the search request DTOs; `fetch_series` path that bypasses scoring; orchestrator fallback | none | S–M | High |
| R13 | P1 | Format / series-type awareness | TPBs, annuals, manga volumes mismatch | §3.2 | Read CV volume type and Metron `series_type`; soft penalty for collected editions vs singles; normalise "Annual N"; golden fixtures | Populate `GenericMetadata.series_type/format`; extend `canonical_issue_number`; add fixtures to `matching_accuracy_golden.rs` | none | M | Medium |
| R14 | P2 | File watcher (or removal) | New files wait for cron | OP-1 | `notify` with 30 s debounce and network-mount fallback to polling | `notify-debouncer-full`, per-library toggle already exists, coalesce into the scan-key path | R5 decides | L (build) / S (remove) | Medium |
| R15 | P2 | Duplicates page | Repacks and cross-library copies pile up | UX-9, DI-21 | Groups by `(series, sort_number, special_type)` >1, exact-hash pairs, cover Hamming ≤8 within a series; keep / soft-remove / open editor | New `api/duplicates.rs` with cursor pagination; honour `dedupe_by_content` or drop it; library-scope the hash lookup | none | M | Medium |
| R16 | P2 | Provider budget, caching, and HTTP resilience | Quotas exhausted; every search re-downloads covers; transient 5xx fails the run | DI-16, DI-22 | Visible per-provider budget bar; parse `Retry-After` and Metron rate-limit headers; Metron ETag conditional requests; bounded retry with backoff on 5xx/transport; connect timeout + JSON body cap; search-time cover pHash cache keyed by provider image URL; shared HTTP client | Extend `metadata_cache` with an image-hash table; a small retry helper in `provider.rs`; `connect_timeout` + `Policy::limited(2)` on both builders; reuse client in `fetch_and_hash_cover`; wiremock tests for 5xx and `Retry-After` | none | M | High |
| R17 | P2 | Reader polish set | Desktop zoom, fit-screen, RTL bar, `ttb`, iOS edge-back | UX-2..UX-5, UX-10 | Ctrl+wheel/trackpad zoom; `contain` fit; zoom persists across pages by preference; mirrored progress bar; `ttb`→webtoon; left-edge inset in standalone | Small changes across `Reader.tsx`, `zoom.ts`, `ReadingProgress.tsx`, `detect.ts` | none | S each | High |
| R18 | P2 | Manual spread controls | Offset or unflagged spreads | UX-2 | Per-issue "force spread / pair / shift by one" persisted per user | New `issue_page_overrides` or extend `pages` JSON; strip affordance | none | M | Medium |
| R19 | P2 | Durable progress outbox | Progress lost if app killed offline | §3.4 | IndexedDB queue replayed on `online`/launch | Step 5 of the offline plan, independent of full offline reading | R3 | M | High |
| R20 | P2 | Issue-level smart views + fields | Cannot express issue queries | UX-8 | `entity: issue` views with `special_type`, `format`, `story_arc`, `rating`, `is_empty` | Second compiler root in `views/compile.rs`; registry entries; rail support | none | M | Medium |
| R21 | P2 | Entity landing pages | Characters/teams/arcs/publishers not browsable | UX-7 | `/characters/[slug]`, `/arcs/[slug]`, `/publishers/[slug]` reusing the creators template | New handlers over existing junctions; cursor pagination | none | M | High |
| R22 | P2 | Ops hygiene bundle | Silent PG16 failure; no arm64; implicit retries | OP-2, OP-3, AR-3, AR-5 | Boot guard `server_version_num ≥ 170000`; arm64 in `release.yml`; explicit `set_max_retries` per queue; `TimeoutLayer` + `CompressionLayer`; `just backup/restore`; cron lease helper | Straightforward | none | S each | High |
| R23 | P2 | Wire or delete the four stub health kinds | Folder/series mismatches invisible at scale | §3.1 | `FolderNameMismatch` and `MixedSeriesInFolder` emitted; others deleted | Scanner checks at series resolve time; fixtures | none | S–M | Medium |
| R24 | P3 | Reader accessibility pass | AT users cannot reach OCR text or hidden chrome | AC-2..AC-5 | Reader in the axe e2e; proxies for detected regions; "page text" panel from OCR; chrome-visible-by-default when a screen reader is detected is not reliable, so add a first-run hint | Extend `a11y.spec.ts`; `MarkerOverlay` proxies | none | M | Medium |
| R25 | P3 | Load baseline | 50k-issue behaviour unknown | OP-6, AR-1 | `just perf-explain` over the stress fixture; project the top list queries with `select_only` | Use `fixtures/build.py --scale stress`; `EXPLAIN (ANALYZE)` recipe; audit the 80 `find()` sites | none | M | Medium |
| R26 | P3 | GCD provider | Pre-1990 and non-US coverage | §9 | Third provider behind the existing trait, plugin-style tolerance for field churn | New `gcd.rs`; CSP `img-src` allowlist change; settings keys; wiremock tests | R16 (budget UI) | L | Medium |
| R27 | P3 | Per-issue offline download | Read on a plane | §3.4 | Selected issues cached (pages + metadata) with an offline-capable shell | Offline plan steps 1–4 | R19 | XL | Medium |
| R28 | P3 | Page-hash anchoring for markers | Markers survive replaced archives with reordered pages | §3.5 | Store per-page hash at capture; re-resolve on rescan | Scanner already has per-page entries; entity + OCR cache key change | R2 | L | Low |

---

## 6. Sequencing

### Quick wins (days each, no schema change)

R3 progress rule · R4 CBT · R5 docs truth pass · R1 (B1, B3, B5, B6 are each a few lines plus a test; B4 needs provenance rows) · R12 year-gate escape + query override · R17 reader polish · R22 ops hygiene · R11 export/permalink/editable text · R23 health kinds · the `MarkerView.kind` comment and `metadata-operator-guide.md` threshold drift.

### Foundational work (weeks; unlocks later items)

- **R6 user-data export/import** is the foundation for migration, rebuild, and notes durability; its shape should be designed with R11 so markers export once.
- **R9 writeback hardening** must land before **R8 manual-edit writeback**, otherwise hand edits become the most frequent trigger of the crash and sidecar-loss paths.
- **R7 series identity edits** depends on R1 so the new user pins are actually honoured by apply and rescan.
- **R10 age-rating enforcement** touches every list surface; do it once, with the cap carried on `VisibleLibraries`, rather than per endpoint.
- **R16 provider caching/budget** should precede any new provider (R26) because the third provider triples the search-time cover downloads.
- **R2 marker remap** is self-contained but should be designed so R28 (page-hash anchoring) can replace the ordinal map later without another migration.

### Larger product bets (choose deliberately)

- **R14 file watcher**: worth building only if the audience copies files in ad hoc; cron plus coalescing already covers scheduled ingest. Recommendation: remove the claim now (R5), build later if asked.
- **R26 GCD provider**: real value for Golden/Silver Age and non-US collections; the API's fields are declared unstable, so budget for churn.
- **R27 offline reading**: the highest-effort item on the list; only after R19 proves the outbox pattern.
- **R18 manual spreads** and **R20 issue-level views**: medium efforts with clear, bounded value.

### Not worth the complexity for a self-hosted household app

- Guided / panel view (needs a panel segmenter; the detector is bubble-tuned and reading-order inference is explicitly deferred in `ocr.md:423-425`).
- Automerge/CRDT sync and shared-collection collaboration (a single Postgres is the sync; dropped 2026-05-15).
- Series-relationship suggestion engine (spec Phase 7): high effort, low daily value next to CBL lists.
- Recommendations / "similar series": one household, one taste; rails and random suffice.
- Manga providers (AniList/MangaUpdates) before volume-mode matching exists; identifiers are already ingested from sidecars, which is the cheap win.
- PDF/EPUB readers, Meilisearch, Kavita-compat API, multi-replica support, Kobo/KEPUB, on-device translation with inpainting, paid tiers, extension-based scraping.

---

## 7. User journeys after the proposed changes

**Journey 1 — Importing 40k issues from a NAS.** Today: scan runs for an hour or more with no fast path, folder/series mismatches land as wrong series with no finding, CBT issues open to a 500, duplicates across the two libraries show up as one health row each. After R4, R15, R23, and a lazy-hash first-import mode: the scan finishes faster, the Findings page lists `FolderNameMismatch` and `MixedSeriesInFolder` rows the user can act on, the Duplicates page groups repacks by series and number with keep/remove, and every accepted format reads.

**Journey 2 — Fixing a series the scanner named "Batman (2016)" that is really "Batman (2011)".** Today: rename the folder on disk, rescan, hope identity resolution keeps progress, then re-search metadata. After R7 + R1 + R12: edit year in the series drawer (a user pin), open the match dialog, adjust the query or paste the ComicVine volume URL, apply. The pin survives the next rescan and the next weekly refresh, the summary the user wrote earlier is not overwritten, and if writeback is on, the corrected `<Series>`/`<Volume>` reach the archive (R8) safely (R9).

**Journey 3 — Reading a manga run across desktop and phone.** Today: desktop trackpad pinch triggers browser zoom; an offset scan pairs the wrong pages; a stale phone write moves the desktop position back; the progress bar fills the wrong way in RTL. After R3, R17, R18, R19: ctrl+wheel zooms the page and persists across turns, a one-tap "shift pairing" fixes the offset for that issue, the server keeps the furthest page, the phone's offline flush replays on reconnect, and the bar mirrors.

**Journey 4 — Keeping notes on a re-read of a 60-issue arc.** Today: notes and highlights work in the reader and on the Bookmarks page, but there is no per-series view, an OCR typo cannot be corrected, and a page cleanup in the archive editor moves every note after the removed ad page. After R2 + R11: notes remap with the edit, the series page gains a Notes tab, captured quotes are editable, and `GET /me/markers/export?format=md` produces a Markdown file grouped by issue and page with Jump links, ready for Obsidian.

**Journey 5 — Migrating hosts and rebuilding from a broken database.** Today: `pg_dump` restore is the only path; if the library is re-imported and any file was retagged in between, ids differ and progress is orphaned; nothing imports from Komga. After R6 + R22: `just backup` produces the dump plus a `me.json` export keyed by content hash, `POST /me/import --dry-run` reports what will match by hash and what falls back to series+number, and the same importer reads a Komga progress export. The PG version guard refuses to boot against PG16 with an actionable message instead of failing in a migration.

---

## 8. Open questions requiring a product decision or hands-on testing

1. **File watcher: build or remove?** The column and docs say yes; the code says no. Removing is a day; building is weeks with NAS edge cases.
2. **Single-instance by declaration?** `scaling.md` and the Kubernetes guide imply replicas. Either add leases and Redis pub/sub or state plainly that one instance is supported.
3. **Progress conflict semantics.** `max(last_page)` is simple but wrong for deliberate re-reads from page 1 on a second device. Timestamp-based last-writer with a monotonic floor unless `?from=start` is the recommended middle; needs a decision.
4. **What should a rescan do to provider-applied values on non-writeback libraries?** Today file values replace them silently while provenance still says "provider" (DI-6). Options: honour provider pins like user pins (file never overrides provider), or stamp provenance to `comicinfo` when the file wins. This is a policy choice with a migration cost either way.
5. **`curator` role: enforce or delete?** Enforcing means metadata and archive edits become non-admin capabilities with audit implications.
6. **Unrated content under an age cap: hide or show?** Needed before R10.
7. **Metron token auth and supporter tiers.** Metron moved to token auth in March 2026; the current client uses username/password settings keys (`registry.rs:204-231`). Whether the Basic-auth path still works needs a live check with credentials.
8. **Real-device reader baseline.** `docs/dev/pwa-performance.md` has never been run on a phone; the 24 MP decode budget and webtoon mount window are tuned blind.
9. **ComicTagger output parity for writeback.** ComicTagger 1.6 beta is the de-facto standard with GCD/Metron in plugins; whether Folio's composed `Notes` token and MetronInfo shape round-trip through it should be tested with real files.
10. **Bundle budget.** The reader is ~190 KB against a 150 KB gate; decide whether to raise the gate or split the marker/OCR overlay out of the first load.

---

## 9. Comparable tools (web research, 2026-09-29)

Versions and dates are from GitHub release APIs and changelogs where possible; items marked UNVERIFIED could not be pinned to a primary source.

| Tool | Relevant to Folio | Source |
|---|---|---|
| Komga 1.28.0 (2026-09-29) | Hash-based duplicate-page manager with Ignore/Manual/Auto-delete and "bytes saved" sort; landscape-page-is-single double-page rule; no spread detection; REST v2 search DSL; no annotations | komga.org/docs/guides/duplicate-pages, changelog |
| Kavita 0.9.1.4 (2026-09-02) | Multithreaded scanner (141k files 14 d → 3 h); double-page offset; per-device reading profiles; EPUB-only annotations with Obsidian export; CBL remap rules + scheduled URL re-fetch; named API keys with device tracking; critical CVE-2026-47202 fixed in 0.9.0.2 | GitHub releases, wiki |
| Codex 2.4.3 (2026-09-24) | Metron + ComicVine tagging with interactive match prompts, per-source rate-limit display, archive writeback, match review with covers and score comparisons; user-data backup/restore sidecar | GitHub NEWS.md |
| YACReader 10.3.2 (2026-09-28) | Library backup/restore, repair-library-db, rename-organize from metadata | CHANGELOG |
| Ubooquity 3.1.0 (Aug 2025, day UNVERIFIED) | Auto DB backup on each scan with rotation | vaemendis.net |
| ComicTagger 1.6.0-beta.10 (2025-12-07) | GCD and Metron talkers moved to plugins; CV 200 req/h throttle; source cover hashes for matching; no stable release in ~4 years | GitHub releases |
| Panels 3.12.6 (date UNVERIFIED) | Live on-device translation, ML panel view, reading presets; double-page handling still a feature request | guides.panels.app, community |
| Mihon 0.20.4 (2026-08-05) | Query-language library search with logical operators and field prefixes | GitHub releases |
| Komelia 0.20.0 (2026-09-27) / komf | Provider priority with gap-filling aggregation per field | GitHub releases |
| Chunky (dormant, 2020) | Auto-contrast / de-yellow scan filters, smart upscaling | App Store |

**Provider landscape.** ComicVine: 200 req/resource/hour. Metron: cut to 20/min and 5,000/day in March 2026, ETag conditional requests, token auth, supporter tiers. GCD: initial JSON API with explicitly unstable fields plus bulk JSON/YAML export (docs returned 403 during research; partially UNVERIFIED). MangaUpdates: official v1 REST. AniList: live 30/min. Marvel API: retired late 2025. MangaDex: 2026 anti-abuse enforcement. MangaBaka/Hardcover: newly adopted across Kavita, Mihon, komf.

**Ideas adopted into the recommendations above:** duplicate-page manager (R15), match review with score deltas and visible budgets (R12/R16), user-data sidecar (R6), notes export to Markdown (R11), landscape-single rule and double-page offset (R18), CBL remap rules (already partly covered by manual overrides surviving refresh), ETag conditional requests (R16), reading presets (a future generalisation of the direction chain; not ranked). **Ideas deliberately not adopted:** paid tiers, dual UIs, extension scraping, full translation pipelines, Kobo sync, acquisition, social annotation sharing, pluggable DB backends.

---

## 10. Proposed next three milestones

**M1 — Integrity (≈2 weeks).** R1 provenance fixes with tests · R2 marker/progress remap · R3 progress rule · R4 CBT · R5 docs truth pass · R22 PG guard, explicit retries, timeout/compression layers, `just backup/restore` · `MarkerView` and operator-guide drift. Exit criterion: every promise in `docs/features.md` and `docs/install/*.md` is either true or removed, and the four silent-overwrite paths have regression tests.

**M2 — Ownership (≈4 weeks).** R6 export/import (with R11's markers export designed in) · R7 series identity edits · R9 writeback hardening then R8 manual-edit writeback · R10 age-rating enforcement + `curator` decision · R12 query override and search-by-URL · R16 provider caching and budget bar. Exit criterion: a user can fix any scanner mistake in-app, hand edits reach the archive safely, a kids' cap is enforced everywhere, and a full library rebuild restores every piece of user data from `me.json`.

**M3 — Reading and discovery polish (≈4 weeks).** R17 reader polish set · R18 manual spreads · R19 durable outbox · R13 format awareness with golden fixtures · R15 duplicates page · R20 issue-level views · R21 entity landing pages · R23 stub health kinds · R24 reader accessibility pass · R25 load baseline. Exit criterion: a mixed manga/US library reads cleanly on desktop and phone, the reader passes axe, and the top ten list queries have measured plans on the stress fixture.

R14 (watcher), R26 (GCD), R27 (offline), and R28 (page-hash anchoring) are held for a decision after M2, in that order of likely value.
