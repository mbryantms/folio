# Folio product audit — 2026-11 refresh

**Date:** 2026-10-31 · **Codebase:** v0.70.0 (`f558bf8a`, the `chore: release v0.70.0` commit on `main`) · **Previous audit:** [product-audit.md](product-audit.md) at v0.28.2 (`d1c2729`, 2026-09-29) · **Scope:** Rust workspace, Next.js web app, Postgres schema, apalis jobs, provider integrations, deployment, docs.

This is an audit, not a change set. It follows the method in §0 of the 2026-09 audit: every material finding cites a repository path and, where possible, a line or function; findings are labelled **Observed** (read in code, tests, or docs), **Inference** (derived from observed code paths), or **Proposal**; every claim of absence names the search that came back empty. Line numbers are accurate for `f558bf8a` and will drift. Finding IDs are prefixed `N-` (new) so they never collide with the 2026-09 `DI-`/`UX-`/`AR-`/`OP-`/`AC-`/`SE-` series, which this document re-verifies in §1.2.

---

## 0. How this audit was done

**Repository map at v0.70.0** (2026-09 figures in brackets). Eight crates (`archive`, `entity`, `migration`, `parsers`, `server`, `server-macros`, `shared`, `tools/audit-check`); the `server` crate is 177k lines [~110k] with 95 API handler modules [60], 83 entities [72], 140 migrations [100] and 210 integration test files [138]. The web app has 67 page routes [49], 209 vitest files [107] and 6 Playwright specs [5]. 53 developer docs under `docs/dev/` [40]. Between the two audits: 527 commits and 87 release tags (v0.28.2 → v0.70.0), 1,221 files changed, +258k / −39k lines.

**Method.** Six parallel read-only deep dives (reader + PWA; metadata providers + writeback; library management + scanner at scale; markers / progress / collections / saved views; architecture + ops + security; and a web-research pass on Komga, Kavita, Codex, Mylar3 and the provider landscape), each required to cite `path:line` and to name the empty search behind every claim of absence. Every headline finding below was then re-verified directly against the source before it was written down. The reports were cross-checked against `docs/features.md`, `docs/product-roadmap.md` (M1–M8 status lines and §7 backlog), `docs/dev/comic-reader-spec.md` §19–21, `docs/dev/security-audit.md`, `docs/dev/load-testing.md` and `docs/dev/pwa-performance.md`, plus the local post-audit plan (`~/.claude/plans/post-audit-roadmap-1.0.md`, Phases 1–4, PRs #1165–#1185).

**Checks run** (this workstation, worktree of `f558bf8a`, dev services on 5432/6380 untouched):

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean, 0 warnings |
| `just audit-check` | 95 files scanned, all `RequireAdmin` handlers audited |
| `just openapi-check` | spec + generated types in sync |
| `just test-rust-fast` (nextest, throwaway PG 18 + Redis 8 on 5433/6381) | **2716 passed, 4 skipped**, 257 s run / 564 s wall |
| `pnpm --filter web test` | 1450 passed across 209 files |
| `pnpm --filter web run lint` / `typecheck` | clean (2 pre-existing `react-hooks/incompatible-library` warnings) |

**Limitations.** No provider credentials, so ComicVine / Metron / GCD behaviour is from code and wiremock fixtures, not live calls. No scan of a large library was run; scale statements rest on `docs/dev/load-testing.md` (50k issues, 2026-09-30 → 2026-10-14) and `docs/dev/scanner-perf.md` (2026-05-09). Playwright e2e and docker-smoke were not re-run locally; CI is the gate. The real-device PWA baseline (WP-4.1) has still never been recorded, so reader performance on a phone is an unknown, not a finding. Competitor claims that could not be pinned to a primary source are marked UNVERIFIED in §8.

---

## 1. Executive summary

Folio at v0.70 is a different product from v0.28.2. Of the 2026-09 audit's five highest-value opportunities and six biggest risks, **ten are fixed or fixed-by-declaration** and the eleventh (load behaviour) is measured on query plans but not soak-tested; of its 64 numbered findings, 45 are fixed, 11 partial, 8 still open (§1.2). The roadmap's eight milestones shipped, then a second local plan (post-audit roadmap, PRs #1165–#1185) closed a further fourteen reader, scanner and bulk-edit items, and October added coverage analysis, GCD as a third provider, local-database plumbing, specials sections, entity profiles, the end-of-issue wrap-up, offline list downloads, and a shadcn/theming rebuild. The test suite grew from 1,829 to 2,716 Rust tests and from 808 to 1,450 web tests, and CI gained a migration-down round-trip, a reader bundle budget, axe on nine surfaces, and a four-way advisory-ignore drift check.

What is left is no longer missing pillars or silent data-loss paths. It is (a) a writeback layer whose *files* are less faithful than its database, (b) a handful of personal-list surfaces that skipped the "removed and revoked content is invisible everywhere" invariant, (c) one scanner outcome that disappears into a blank card, (d) a few per-request hot spots that are O(library) and will show at 50k, and (e) a second round of docs that promise slightly more than the code does. All five are S–M work.

### 1.1 The five highest-value opportunities (2026-11)

1. **Make the written sidecars as good as the database — the writeback fidelity bundle.** Every production library has writeback on, so the archive is the record. Six gaps make that record worse than the DB: the MetronInfo serializer is still pre-schema (`<Title>`, `<Year>/<Month>/<Day>`, flat `<Series>`/`<Publisher>`, `<ID>` without `<IDS>`, `<StoryArcs>`; `crates/parsers/src/metroninfo.rs:258-300`; only `<Credits>` was brought to the XSD in WP-8.1); the composer reads the CSV read-cache columns rather than the junctions and splits them with its own bare-comma `split_csv` (`crates/server/src/metadata/sidecar_compose.rs:339-384,909-918`), so a cached "Mike Deodato, Jr." becomes two `<Creator>`s in the file that Folio's own re-ingest then heals (`metadata_rollup::split_csv` reattaches suffixes), leaving other tools with the wrong file; `fill_missing` is still not consulted on the sidecar apply path (`docs/dev/metadata-sidecar-writeback.md:282` says so; grep `.mode|FillMissing` over `apply.rs:767-1330` → nothing), so "fill missing" behaves as provider-wins; the sidecar path never applies the **primary** cover (`args.apply_cover` is read only in the DB-direct `write_issue_fields`, `apply.rs:1855`; grep `apply_cover|cover_image_url|fetch_cover` in `jobs/rewrite_sidecars.rs` → nothing, while variants ride `PostRewriteWrites`); external-ID edits write `SetBy::User` rows but never enqueue a rewrite (grep `manual_writeback|enqueue_.*rewrite` in `api/external_ids.rs` → nothing) although issue PATCH, bulk edit and series PATCH all do; and a *failed* rewrite still `return Ok(())` (`jobs/rewrite_sidecars.rs:203-207`), as does the apply job when the series mutex is busy (`jobs/metadata_apply.rs:192-195`, "caller will retry" — none does), so apalis never retries transient I/O and a batch child whose series is locked by a page edit is dropped with its run never failed. Effort S each, M for the schema; confidence high; `sidecar_parity.rs` and `writeback_provider_ingest.rs` (32 tests) are the harness to extend.

2. **Apply the visibility invariant to the personal lists.** PR #1277 made soft-removed series invisible on every list surface, rail and OPDS feed, and `access::active_series` plus the age cap now guard series, issues, search, markers and the bookmarks feed. Three surfaces were missed: `GET /me/collections/{id}/entries` hydrates series and issues by id with no `removed_at`, `state` or `access::` filter (`crates/server/src/api/collections.rs:684-737` ownership check only, `hydrate_entries` `:1482-1570`; grep `access::|RemovedAt` over that range → nothing, while the `next-up` and `window` branches at `:774,836` do apply the ACL); `GET /me/cbl-lists/{id}/entries` applies `issue_ok` (library + age cap) but not `removed_at` on matched issues (`api/cbl_lists.rs:883-924`); and `GET /me/reading-sessions` filters only by user, issue and series (`api/reading_sessions.rs:594-660`). A soft-removed issue keeps a live card in a collection with a link that 404s. Effort S, one test each, high confidence.

3. **Stop losing encrypted archives into blank cards.** `ArchiveOutcome::Encrypted` bumps `stats.files_encrypted` and inserts a zero-page issue with `state = "encrypted"` (`crates/server/src/library/scanner/process.rs:416-419`); nothing else happens. There is no `IssueKind` for it (`library/health.rs:30-186` has 19 variants, none encrypted), `files_encrypted` is never serialized or read outside `scanner/stats.rs` (grep → only the increment), and the web app has no reference to the state (grep `"encrypted"` in `web/{components,app,lib}` → 0). The user sees a coverless card; `docs/features.md:175-176` promises "detected and flagged". Effort S (one variant, one emit, one stats field).

4. **Fix the O(library)-per-request surfaces before they show at 50k.** The duplicates page computes every group in the library, sorts in memory, then applies the cursor and `take(limit)` (`api/duplicates.rs:448-455,787-796`); the health page's `.bak` storage card `walkdir`s the whole library root per request (`api/health_issues.rs:62,101-113`); the scanner's person upsert races on `person_slug_key` because `ON CONFLICT (normalized_name)` does not cover the slug index, and the `?` at `metadata_rollup.rs:829` aborts the rest of that series' rollup (CSV cache, entity rows, counts) for the pass — observed 18 times at 50k (`docs/dev/load-testing.md:630-637`); the post-scan per-issue `stamp_done` UPDATE (F-4) and the full-row dedupe lookup (F-5) from `scanner-perf.md` are unchanged. None is measured on the perf harness (`scripts/perf/endpoints.txt` has no duplicates or health-page entry). Effort S each.

5. **A second docs-truth pass, plus the settings that exist but do nothing.** `docs/features.md` is dated "v0.27.x (July 2026)" and has ten claims the code does not back (§2): "Two providers" (three plus local files), "Twelve typed library health issues" (19), "about 117 KB gzip … 130 KB ceiling" (130.13 KB measured, gate 145 KB), "scan-on-startup" (no such feature), CBR/CB7 "Read" (never read in place), "password-protected archives are … flagged" (§1.1 item 3), "a 5-second accidental open doesn't count" (rule is <30 s and <3 pages), "annuals/specials only count toward completion if you want them to" (no such preference), "refresh-token … reuse detection" (replays are rejected, siblings are not revoked), and the Up Next resolver omits the collection branch. Beyond `features.md`: `docs/dev/metadata-providers.md:1514` still says the HIGH threshold default is 95 (code and operator guide say 80); CLAUDE.md names a `web/lib/metadata/csv.ts` twin that does not exist; `web/proxy.ts:9` says routes live outside `[locale]/` while all 67 pages are under it; `.env.example:47` documents `COMIC_OIDC_CLIENT_SECRET_FILE` with no reader in `config.rs`; `SECURITY-EXCEPTIONS.md:18` still lists the withdrawn braces advisory as accepted; the roadmap says hard purge is "off by default" while `config.rs:458-464` defaults the multiplier to 2 (purge at 60 days). Two registry settings have neither UI nor an operator-guide row (`metadata.alternate_cover_fetch_cap`, `metadata.merge.provider_preference`), and the four local-database settings are stored and shown but no provider reads them yet (`docs/dev/local-metadata-databases.md:33`, by design as M1 of 6). Effort S.

### 1.2 Scorecard against the 2026-09 audit

**Five highest-value opportunities (2026-09 §1)**

| 2026-09 item | Status at v0.70 | Evidence |
|---|---|---|
| Close provenance holes B1–B6 (DI-1..5) | **Fixed** | `apply.rs:184-221` `user_pinned()` aliases Summary↔Description; `writers.rs:326-385` compares tiers not values; `apply.rs:2387-2405` reads `override_external_id_sources`; `apply.rs:1542-1550` gates `number_raw` on `MetadataField::Number`; series PATCH pins at `series.rs:625-651` (but see N-M6) |
| Remap markers + progress on archive page edit (DI-19) | **Fixed** | `jobs/archive_edit.rs:550-573` → `reading::page_remap::reanchor_issue`, page-hash authority (WP-6.2) |
| Multi-device progress conflicts (UX-1) | **Fixed** | reading-run model: `api/progress.rs:6-29`, migration `m20270201_000001_progress_run.rs`; `max(prev, page)` within a run, `restart` opens run n+1 |
| User-data export | **Fixed (export only)** | `api/account_export.rs:362` `GET /me/export` with every WP-2.1 section incl. `run` and `page_hash`; import removed by owner decision (roadmap WP-2.2) |
| Series identity edits + manual edit → archive | **Fixed** | `UpdateSeriesReq` gains name/year/volume/publisher/imprint/age_rating/total_issues/language_code (`series.rs:237-257`); `manual_writeback::enqueue_*` from `issues.rs:1889,3110` and `series.rs:733-738` |

**Biggest current risks (2026-09 §1)**

| 2026-09 risk | Status at v0.70 | Evidence |
|---|---|---|
| Docs promise behaviour the code lacks (watcher, Redis re-enqueue, pub/sub, `retain_days`) | **Fixed; new drift listed in §2** | `library/watcher.rs` (inotify + statfs-chosen poll), `backup.md:11-27`, `scaling.md:1-21`, `jobs/backup_prune.rs` |
| CBT scans then 500s | **Fixed** | `library/zip_lru.rs:58-74` dispatches on sniffed container; `"cbt" => Cbt::open` |
| Writeback can lose the file / sidecars / jobs | **Fixed, residue in N-M1..N-M5** | `archive_rewrite/mod.rs:24-31` hard-link `.bak` then one rename ("target is never missing"); `archive/src/rewrite_policy.rs`; MetronInfo raw passthrough; lock-busy → `requeue_busy` with a 40 × 5 s budget (`rewrite_sidecars.rs:243-294`) |
| ACL columns that do nothing | **Fixed** | age cap enforced in `library/access.rs:276,314` and carried by `issue_allowed`; `library_user_access.role` dropped by `m20270123_000001_drop_library_access_role.rs`; grep `curator` in `crates/ web/` → 0 |
| Provider budgets, no cache, no `Retry-After` | **Fixed** | shared `metadata/http.rs` (connect timeout, 2-hop redirects, 8 MiB body cap, 3-retry jittered backoff, `Retry-After` incl. HTTP-date); Metron `If-None-Match`/`If-Modified-Since` (`metron.rs:319-320`); 30-day `cover_hash_cache`; budget bar; 9 wiremock tests in `tests/provider_http.rs` |
| Load behaviour at 50k unmeasured | **Fixed (plans), partial (coverage)** | `docs/dev/load-testing.md`: 50k fixture, 12 endpoints EXPLAIN'd with a seq-scan gate, `oha` per endpoint, M7 surfaces; `IssueCardRow` projections (85 refs). Still unmeasured: soak, NAS/HDD, >50k, duplicates/health pages (§7) |

**All 61 numbered 2026-09 findings** (verified individually by the deep dives, headline rows re-checked here):

| Series | Fixed | Partial | Still open |
|---|---|---|---|
| UX-1..13 | 11 | UX-8 (`ends_with` op and nested groups absent: `views/dsl.rs:222-226` "group_id always 0"), UX-11 (bundle re-based, see N-R1) | — |
| DI-1..22 | 16 | DI-3 (series pins written after commit, non-fatal: `series.rs:616-651`), DI-6 (`review` still file-owned unconditionally, `process.rs:1277`), DI-13 (failed rewrite / busy apply return `Ok`), DI-18 (`metadata-providers.md:1514` says 95) | DI-9 (`fill_missing` ignored on sidecar path), DI-17 (`trigger_kind::SCANNER` still never enqueued; grep `SearchSeriesJob|SearchIssueJob` in `library/`, `jobs/scan.rs`, `jobs/post_scan.rs` → nothing) |
| AR-1..9 | AR-3, AR-4 (by declaration), AR-5, AR-9 | AR-1 (`IssueCardRow` on card paths; 91 `issue::Entity::find()` in `api/` vs 80, 42 `select_only` vs 24 — the 15 remaining full-row `.all()` sites are id-set hydrates and write paths plus the 7 OPDS acquisition feeds), AR-8 (`marker.color` bounded by garde, schema still `text`) | AR-2 (`[locale]` segment: all 67 pages under it while `web/proxy.ts:9` says otherwise), AR-6 (30 `user_edited` references remain in comments/tests), AR-7 (19 stringly `role/state/kind: String` entity fields, 0 `DeriveActiveEnum`) |
| OP-1..8 | OP-1, OP-2 (`pg_version.rs`), OP-4, OP-6 (plans), OP-8 | OP-5 (thumbs gauge + budget yes; `audit_log` retention still none) | OP-3 (amd64 only, `release.yml:1-3,169,180`, owner-deferred), OP-7 (thumbs `private, no-cache`, documented design) |
| AC-1..5 | AC-1, AC-5 (nine `expectNoAxeViolations` sites incl. three reader states, `wcag22aa` tags, all in docker-smoke) | AC-3 (OCR regions are now keyboard buttons, `MarkerOverlay.tsx:651-665`; page images still `alt="Page N"`), AC-4 (3 `text-[10/11px]` labels, was 7) | AC-2 (`chromeVisible: false` default, `store.ts:268,414`) |
| SE-1..7 | all 7 | — | — |

### 1.3 The biggest current risks

| Risk | Evidence | Severity |
|---|---|---|
| **Writeback writes a MetronInfo other tools will not read correctly, and a ComicInfo whose credits can be split wrong.** | §1.1 item 1; `metroninfo.rs:258-300`; `sidecar_compose.rs:909-918`; roadmap §7 "Found during M8" acknowledges the schema gap with no WP | High for the owner (every prod library is writeback) |
| **Personal lists show content the invariant says is invisible.** | collections / CBL / sessions, §1.1 item 2 | Medium; dead links, and a revoked-library issue stays visible in a collection |
| **Encrypted archives vanish into blank issues.** | `process.rs:416-419`; no `IssueKind`; `features.md:175` promises flagging | Medium; narrow but silent |
| **Silent drops in the apply/rewrite jobs.** | failed rewrite → `Ok` (`rewrite_sidecars.rs:203-207`); busy series mutex → `Ok` (`metadata_apply.rs:192-195`); series PATCH pin failure is `warn!` only (`series.rs:649`) | Medium; the DI-7 failure mode fixed for issues still exists for series |
| **Per-request O(library) paths.** | duplicates, `.bak` walk, person-slug race (§1.1 item 4) | Medium at 50k; low at the owner's 2.5k-series library |
| **Offline reading is less faithful than online.** | spread overrides not stored or applied offline (N-R7); non-list downloads hit the network for Up Next (N-R8) | Medium for the owner's plane-and-commute case; both S |
| **Blocking I/O on the async runtime in one handler.** | `api/archive_edit.rs:722,728` `std::fs::create_dir_all` / `std::fs::write` of up to 32 MiB (`MAX_UPLOAD_BYTES`, `:102`) inside `pub async fn upload` (`:674`), `:748-760` directory sweep; no `spawn_blocking` between | Low; admin-only path, one worker thread stalls per upload |
| **Reader performance on a real phone is still unmeasured.** | `docs/dev/pwa-performance.md:10-18` "No physical-device baseline has been recorded yet"; the 24 MP decode budget and webtoon mount window remain tuned blind | Unknown; owner-run WP-4.1 |

---

## 2. Docs promise behaviour the code lacks

Every row was checked against source at `f558bf8a`. "Fix" is the smallest truthful change; where the code is the better side of the gap, build it instead.

| Doc | Claim | Code | Fix |
|---|---|---|---|
| `docs/features.md:230` | "Two providers that stack" | three (`metadata/identifier.rs:19-22` ComicVine/Metron/Gcd; `coverage.rs:89`) plus local GCD / ComicVine SQLite files (`metadata/local_db.rs`) | "Three providers, plus local database files" |
| `features.md:217` | "Twelve typed library health issues" | 19 `IssueKind` variants (`library/health.rs:30-186`), all with an emit site | "Nineteen" |
| `features.md:445-448` | "about 117 KB gzip … with a 130 KB ceiling" | measured 130.13 KB on 2026-10-15, gate `BUDGET_KB = 145` (`web/scripts/check-bundle-size.mjs:74`; `pwa-performance.md:50-52`) | quote the current row of the budget table |
| `features.md:211` | "scan-on-startup" | no implementation: grep `-i "on_startup|at_startup|startup.*scan|scan.*startup"` in `crates/server/src` → only coalescing-key sweep comments; nothing in `entity/library.rs` or the library settings form. `library-scanner.md:606` repeats it | delete the phrase (or build a per-library flag) |
| `features.md:171-172` | `.cbr` "Read; converts to CBZ when edited", `.cb7` "Read; converted…" | neither is ever read in place: `zip_lru.rs:71-73` rejects anything but cbz/cbt; unconverted CBR/CB7 → `UnsupportedArchiveFormat` (`process.rs:850-856`); conversion is at scan (`process.rs:518-526`), not edit | "Converted to CBZ on scan when the library opts in; otherwise flagged" |
| `features.md:175-176` | "Password-protected archives are detected and flagged" | detected, counted, never flagged (§1.1 item 3) | build N-L1 |
| `features.md:416` | "a 5-second accidental open doesn't count" | thresholds are per-user `reading_min_active_ms` = 30 000 and `reading_min_pages` = 3 (`m20261001_000001_reading_sessions.rs:254,260`; `web/lib/reader/session.ts:97`) | "under 30 s and fewer than 3 pages" |
| `features.md:422-425` | annuals/specials count toward completion "if you want them to (per-user format preference, per-series override)" | completeness always excludes `special_type IS NOT NULL` (`views/compile.rs:850-856`); grep `special|annual` in `auth/preferences.rs`, `api/series_reader_prefs.rs`, `entity/user.rs` → nothing | delete the clause (or build the preference) |
| `features.md:433` | "refresh-token rotation and reuse detection" | rotation yes; a replayed hash is rejected with "we don't know the user, so we can't revoke siblings" (`auth/local.rs:743-749`) | "replayed refresh tokens are rejected" (or store `previous_hash` per session and revoke on match) |
| `features.md:147-161` | Up Next resolver: list → series → caught up | collection context comes first (`api/next_up.rs:5-24`, PR #1180) | add the collection branch |
| `features.md:564` | "current as of v0.27.x (July 2026)" | — | re-date with this pass |
| `docs/dev/metadata-providers.md:1514` | HIGH auto-apply threshold "default 95" | 80 (`config.rs:491-492`); the same file says 80 at `:286`, operator guide says 80 | 95 → 80 |
| `CLAUDE.md` ("Junctions are the truth") | `split_csv` (server) and `lib/metadata/csv.ts` (web) "must stay twins" | `web/lib/metadata/` holds `coverage-hint.ts`, `provider-status.ts`, `quota.ts`; no `csv.ts` | name the real splitter or drop the sentence; also the composer's private `split_csv` is a third, non-twin copy (N-M2) |
| `web/proxy.ts:9` | "Routes live directly under `web/app/...`, no `[locale]/`" | `find web/app -name page.tsx \| grep -vc '\[locale\]'` → 0 | delete the sentence (or finish Human-URLs M3) |
| `.env.example:47` | `COMIC_OIDC_CLIENT_SECRET_FILE=/run/secrets/oidc` | grep `CLIENT_SECRET_FILE|secret_file` in `crates/` → nothing; no `_FILE` indirection in `config.rs` | remove, or add `_FILE` handling |
| `.env.example` | (omissions) | 37 `Config` fields have no example line, incl. `COMIC_THUMBS_BUDGET_MB`, `COMIC_PAGE_VARIANT_CACHE_BYTES`, `COMIC_OTLP_ENDPOINT`, `COMIC_OPDS_PANELS_MODE`, `COMIC_CHECK_UPSTREAM_RELEASES`, `COMIC_LIBRARY_HARD_PURGE_MULTIPLIER` | add commented lines |
| `SECURITY-EXCEPTIONS.md:18`, `pnpm-workspace.yaml:52`, `osv-scanner.toml:95` | GHSA-vfj7-8cjw-p6xm (braces) accepted, "no patched release exists" | advisory withdrawn by GitHub 2026-10-23 (sibling session note, 2026-10-30); grep `-i withdrawn` in the three files → none | move to Resolved; update `scripts/ci/check-advisory-ignores.py` in the same PR |
| `docs/product-roadmap.md:167` | WP-3.5 "purge off by default" | `default_hard_purge_multiplier` = 2 (`config.rs:458-464`), i.e. purge **on**, 60 days after confirmation for the default `soft_delete_days = 30` (`libraries.rs:610`) | state which; either is defensible |
| `docs/dev/library-scanner.md` health table | 15 kinds | 19; missing `MalformedArchive`, `RecoveredArchive`, `UnreadablePage`, `NoPages` | add the four rows |
| `docs/dev/archive-writes.md:21` | bulk-edit route `/issues/archive/bulk-edit` | `/archive/bulk-edit` (`api/archive_edit.rs:313`); the writer table also lacks `POST …/flush-metadata-drift` (`api/health_issues.rs:853-869`, gated 409) | fix path, add row |
| `docs/dev/metadata-operator-guide.md` | (omissions) | no rows for `metadata.match_medium_threshold` (UI yes), `metadata.alternate_cover_fetch_cap` (no UI), `metadata.merge.provider_preference` (no UI), `metadata.{gcd,comicvine}.local_{path,mode}` | add rows; decide whether the two UI-less keys get a field or go env-only |
| `docs/dev/metadata-sidecar-writeback.md` | (omission) | `issues.review` is always overwritten from the file (`process.rs:1275-1277`), by design; the sidecar path never applies the primary cover (N-M3) | document both |

Everything else checked in `docs/features.md` (reader, scanner, metadata, organizing, search, OCR, OPDS, stats, multi-user, speed sections) and in `docs/install/*.md` resolves to code; the install docs' Redis sentence, single-instance declaration, `.env.example` numeric defaults, operator-guide threshold and `library-scanner.md` Triggers table are all true at this commit.

---

## 3. Findings by area

Each finding is **Observed** unless marked otherwise.

### 3.1 Reader and PWA (N-R)

- **N-R1.** The reader bundle budget was re-based twice: 150 KB (pre-WP-4.4) → 130 KB (117.45 KB measured, 2026-09-30) → 145 KB (130.13 KB measured, 2026-10-15, after the per-series account prefs T4.4) (`docs/dev/pwa-performance.md:50-52`; `check-bundle-size.mjs:72-74`). The gate is honest; `features.md` is not (§2). Inference: the +13 KB came from reader features landing after the split; the roadmap's "measured + 10%" rule is being applied, so this is a tracking note, not a defect.
- **N-R2.** No physical-device baseline exists (`pwa-performance.md:10-18`, "Pending physical-device run" on every column). WP-4.1 is the only roadmap item still open that only the owner can do.
- **N-R3.** Reader chrome is hidden by default (`web/lib/reader/store.ts:268,414` `chromeVisible: false`, AC-2); the WP-4.8 first-run hint mitigates but does not change the default.
- **N-R4.** Page images carry `alt="Page N"` only (`Reader.tsx:1623,1861`) although the OCR pipeline and the page-text panel exist (AC-3 residue).
- **N-R5.** Three `text-[10px]`/`[11px]` labels remain in reader chrome (AC-4 residue).
- **N-R6.** Present and verified (do not re-propose): manual spread / pair / shift-by-one saved per account per issue (`api/page_overrides.rs`), ctrl+wheel and trackpad zoom, contain fit, RTL-mirrored progress bar, series `ttb` → webtoon, iOS left-edge inset, durable IndexedDB progress outbox, per-issue and per-series offline downloads with size tiers and quota display, list/collection "next N" downloads (#1296), `/downloads` offline landing, PWA icons generated (WP-4.7), per-series reader prefs following the account (#1185), end-of-issue rate / note / review (#1258), collection-driven Up Next (#1180), one-bar phone chrome (#1376–#1384), reader axe in docker-smoke.

- **N-R7 (medium).** Manual spread / pair / shift overrides are not applied when reading offline: `OfflineReader.tsx:155-195` passes no page overrides, nothing persists them to the offline store (grep `-i override` in `web/lib/pwa/offline-store.ts`, `web/lib/pwa/downloads.ts` → nothing), and `useSpreadOverrides` returns `null` on a failed fetch so automatic pairing takes over (`web/lib/reader/use-spread-overrides.ts:9-12`). An offset manga scan the user fixed online re-pairs wrongly on the plane.
- **N-R8 (medium).** An offline download opened outside a list leaves Up Next on the live network query: `OfflineReader.tsx:97-100` yields `undefined` when there is no list, `Reader.tsx:310` treats only a defined `nextUpOverride` as overriding, so the live `useNextUp` query runs, fails offline, and the end-of-issue card shows its error state. `offline-lists.ts:101 summaryFromRecord` already builds the summary a stored-series fallback needs.
- **N-R9 (medium, touch).** Pinch is still handed to the browser by design (`Reader.tsx:1158-1163`, `web/lib/viewport.ts:27-28` `userScalable: true`), so on touch the fixed chrome scales with the page; the UX-3 fix (transform zoom with ctrl+wheel / trackpad) is desktop-only, and touch reaches transform zoom only via double-tap (`Reader.tsx:876-890`). `features.md:90-93` frames this as intentional; it is the one remaining reader complaint from 2026-09 that a phone user still meets.
- **N-R10 (low).** `contain` silently falls back to fit-width in webtoon (`Reader.tsx:1123-1126`) while `ReaderSettings.tsx:92` still offers it there.
- **N-R11 (low).** The page-strip spread toggle is hover-revealed (`opacity-0 group-hover/thumb:opacity-100`) and only the active thumb is tabbable (`PageStrip.tsx:539-556`), so touch and keyboard users can change only the current page's pairing from the strip. `useIssuePageOverrides` (`web/lib/api/queries.ts:~2282`) uses default TanStack retry and retries three times offline before falling back.
- **N-R12 (hygiene).** `Reader.tsx:858` re-creates the `RECENTER` literal every render and carries four `exhaustive-deps` suppressions (`:861,867,872,888`) to tolerate it. No `TODO|FIXME|HACK` in reader or PWA code (grep → only "placeholder" image markers). Error handling is sound: one auto-retry then tap-to-retry on page images (`PageImage.tsx:81-135`), health toast, `error.tsx` with digest, the #1221 previous-issue-write regression guarded by `ready` (`use-progress-write.ts:189-193`). Test inventory: 25 files under `web/tests/reader/`, 10 under `web/tests/pwa/`, 9 reader DOM tests, 43 server progress tests.

### 3.2 Metadata, matching, writeback (N-M)

- **N-M1 (high).** MetronInfo output is pre-schema except `<Credits>` (`crates/parsers/src/metroninfo.rs:258-300`; grep `CoverDate|<IDS|"Arcs"` → nothing). Acknowledged at `docs/product-roadmap.md:357` and `metadata-sidecar-writeback.md:136`; no WP schedules it. Every writeback library's archives carry a non-conforming file.
- **N-M2 (high).** The sidecar composer reads the CSV read-cache columns, not the junctions, and splits them with a private bare-comma `split_csv` (`metadata/sidecar_compose.rs:339-384,448-452,909-918`), violating the CLAUDE.md "junctions are the truth" rule for every writeback and producing wrong `<Creator>` splits for suffixed names. `sidecar_parity.rs` has three cases, none with a suffixed name.
- **N-M3 (medium).** The sidecar apply never applies the primary cover (`apply.rs:1855` is DB-direct only; `jobs/rewrite_sidecars.rs` `PostRewriteWrites` `:125-150` carries variants, reprints, provenance and `bump_sync` only). On writeback libraries "Apply cover" silently does nothing for the primary.
- **N-M4 (medium).** `fill_missing` is not consulted on the sidecar path (DI-9 still open; `mask_unselected_issue_fields` at `apply.rs:2558` honours `selected_fields` only).
- **N-M5 (medium).** External-ID edits (`api/external_ids.rs:340-345`) do not enqueue manual writeback; `drift.rs` is `field_provenance`-only, so the archive's `<ID>` drift is never surfaced.
- **N-M6 (medium).** Series PATCH provenance is written after commit, outside the row transaction, failures `warn!`-ed as "non-fatal" (`api/series.rs:616-651`); the issue path got one transaction in WP-2.5 (`issues.rs:3085-3098 update_issue_with_user_pins`).
- **N-M7 (medium).** Failed rewrite returns `Ok` (`rewrite_sidecars.rs:203-207`) so apalis never retries transient I/O; the apply job's busy series-mutex path also returns `Ok` with no requeue (`metadata_apply.rs:192-195`) and nothing marks the run failed (grep `"applying"` in `orchestrator.rs`, `metadata_apply.rs` → nothing). The lock is claimed only by apply, rewrite and page edit (`try_claim(` at `apply.rs:938`, `rewrite_sidecars.rs:158`, `jobs/archive_edit.rs:294`, `api/archive_edit.rs:498`), never by scanner, convert or thumbnails.
- **N-M8 (low).** `SIDECAR_ISSUE_PROVENANCE_FIELDS` (`apply.rs:2529-2548`) omits `Number`, `Notes`, `WebUrl`, so after a writeback apply those read as file-tier and a later non-writeback scan or `fill_missing` treats them as unowned.
- **N-M9 (low).** DI-17: scans still never auto-search (`trigger_kind::SCANNER` mapped at `metadata_search.rs:928`, `orchestrator.rs:66`; never enqueued).
- **N-M10 (low).** Operator surface: 25 `metadata.*` registry keys; `alternate_cover_fetch_cap` and `merge.provider_preference` have no UI and no guide row; `match_medium_threshold` has UI but no guide row; the four `local_*` keys are stored and shown but no provider reads them (`local_db.rs` only exposes `LocalDbProbe`; grep `local_db::|LocalBacked` in `gcd.rs`, `comicvine.rs`, `orchestrator.rs` → nothing). The preview pane shows "Metron · 2026-04-15" (absolute) where `features.md` promises "2 days ago" (`MetadataPreviewPane.tsx:482-488`); the issue tab has a pinned-fields list, no per-field hover.
- **N-M11.** Verified clean: no writer-surface bypass outside `metadata_rollup.rs` (the sanctioned ingest); `MetadataField::iter()` only in `field.rs` tests; no unguarded `unwrap` on provider JSON; `provider_http.rs` (9), `metadata_drift.rs` (6), `manual_writeback.rs` (12), `local_metadata_db.rs` (15), `writeback_provider_ingest.rs` (32), `backup_prune.rs` tests exist. Missing tests: sidecar `fill_missing` (cannot exist until built), external-ID PATCH → rewrite, suffixed-name compose, failed-rewrite retry.

### 3.3 Library management and scanner (N-L)

- **N-L1 (high).** Encrypted archives become blank issues with no finding (§1.1 item 3).
- **N-L2 (medium).** `ensure_persons_for_series` races on `person_slug_key` (`metadata_rollup.rs:825-835`; `ON CONFLICT (normalized_name)` is not an arbiter for the slug index); the error aborts the whole best-effort rollup for that series in that pass (`:783`, `:618`, `rollup_series_metadata_best_effort` `:940-943` only warns). 18 occurrences at 50k (`load-testing.md:630-637`). Credits still land; CSV cache, entity rows and counts for that series lag until the next pass.
- **N-L3 (medium at scale).** Duplicates page is O(library) per page request (`api/duplicates.rs:448-455,787-796`); `total` recomputed every page though serialized once.
- **N-L4 (medium on NAS).** Health page `.bak` card walks the library root per request (`api/health_issues.rs:62,101-113`).
- **N-L5 (low).** Scanner F-4 (per-issue `stamp_done` UPDATE, `post_scan.rs:449-456`) and F-5 (full `issue::Model` per new file in `dedupe_by_content_hash`, `process.rs:632-637`) from `scanner-perf.md:380,392` are unchanged.
- **N-L6 (low).** `GET /opds/v1/recent` is a hard `limit(50)` with full rows (`opds.rs:923-925`); the 7 OPDS acquisition-feed sites are the acknowledged projection backlog (`load-testing.md:578-586`). `GET /series/{slug}/collection` returns every issue of a series (domain-bounded, `series.rs:2965,3384`).
- **N-L7.** Verified: every archive writer in `docs/dev/archive-writes.md` checks `allow_archive_writeback` (`api/archive_edit.rs:158,400`, `jobs/archive_edit.rs:391`, `apply.rs:447,1320`, `rewrite_sidecars.rs:534`, `manual_writeback.rs:96`, `process.rs:524`, `backup_prune.rs:9`, `scheduler.rs:449`); a `fs::rename|remove_file|fs::write(|File::create|hard_link` sweep outside thumbs/cache/tests finds no ungated writer. Hard purge is on by default (multiplier 2, floor 1 day, daily 04:15 UTC, export-to-log + `library_events` row). The watcher's 1,000-file test and forced-poll test exist (`tests/file_watcher.rs:161,270,387,481`). Present and worth crediting since 2026-09: container sniffing over extension (#1068), decorated bucket folders and more `<Format>` spellings (#1087), series-named-specials repair keyed on folder (#1078), "Repair creator and character names" admin action, admin cover picks (#1181), unreadable folder never authorises removal (#1167), rolled-back batch leaves the folder dirty (#1168), series-level and bulk removal confirm (#1278), removed series hidden everywhere (#1277), per-series missing-issues report (`GET /series/{slug}/collection`), folder browser before adding a library (`admin_fs.rs:65`).

### 3.4 Organization, annotation, discovery (N-O)

- **N-O1 (high).** Collection entries ignore ACL and `removed_at` (§1.1 item 2).
- **N-O2 (medium).** CBL entries show soft-removed matched issues (`cbl_lists.rs:883-924`).
- **N-O3 (low).** `/me/reading-sessions` lists sessions on removed or revoked issues (`reading_sessions.rs:594-660`).
- **N-O4 (low).** Roadmap §7 items still open: multi-select on the issue-view detail page (`rg -c useSelection web/components/saved-views/IssueFilterViewDetail.tsx` → 0; the series twin has it at `FilterViewDetail.tsx:43,125`); "Sequel of / Prequel of" groups still repeat the Reading-order strip (`SeriesRelatedSection.tsx:240-244` vs `:287`); `useRestoreRailItem` (`web/lib/api/mutations/index.ts:1384-1390`) has zero consumers, so the `GET`/`DELETE /me/rail-dismissals` endpoints (`rails.rs:1560,1646`) have no UI; CBL import quota stays constant (`cbl/quota.rs:37-41`, by decision).
- **N-O5 (low).** UX-8 residue: no `ends_with` op (grep `EndsWith|ends_with` in `views/`, `web/components/filters` → nothing) and no nesting (`dsl.rs:222-226`).
- **N-O6.** The command palette has exactly 27 actions (`web/lib/search/actions-registry.ts`, 15 user + 12 admin) — `features.md` is right — but none for Browse, the Views index, Stats/Log, `/admin/relationships` or `/admin/background-work`.
- **N-O7.** Verified clean: no TODO/FIXME in the scoped modules; no client-side `.filter` over a capped list (the `MarkersList`/`CollectionViewDetail`/`FilterViewDetail` filters run over fully-walked data); no `useQuery` on a `next_cursor` response in this area (the three remaining are admin scanner hooks `useScanRuns`, `useScanBatches`, `useAdminHealthIssues` at `queries.ts:625,695,815` — all bounded admin tables, flagged for the next admin pass); account export covers every WP-2.1 section plus `rail_dismissals`, `issue_notes`, `want_to_read`, `keybinds`; `/me/markers/export` includes markers on revoked issues as `available: false` by owner decision.

### 3.5 Architecture, operations, security, accessibility (N-A)

- **N-A1 (low).** Blocking file I/O in `api/archive_edit.rs` `upload` (§1.3). `grep -rn block_on crates/server/src/api/` → 0 elsewhere.
- **N-A2 (low).** No `DefaultBodyLimit` anywhere (grep `DefaultBodyLimit|RequestBodyLimit` → 0): axum's 2 MiB default applies to buffered extractors; multipart uploads cap themselves (`cbl_lists.rs:48` 4 MiB, `archive_edit.rs:102` 32 MiB); the upstream proxy streams request bodies unbounded with a 60 s `PROXY_TIMEOUT` (`upstream/mod.rs:13,58`); `/ws/*` has no tungstenite `max_message_size` (defaults 64 MiB / 16 MiB).
- **N-A3 (low).** CLAUDE.md's "every `pub async fn` in `api/` carries `#[handler]`" has eight exceptions: route handlers `api/csp.rs:25` `csp_report` and `api/ws_scan_events.rs:48` `handler`, plus six pub helpers (`reading_sessions.rs:791`, `saved_views.rs:2280`, `scan_runs.rs:599`, `server_releases.rs:152`, `sidebar_layout.rs:301`, `work_targets.rs:149`).
- **N-A4 (low).** 58 `Json<T>` bodies vs 29 `Validated<T>`; no `Json<T>` struct derives `garde::Validate` (so nothing bypasses garde), but `CreateCollectionReq` (`collections.rs:165,503-516`), `UpdateIssueReq` (`issues.rs:695`), `AddExternalIdReq` (`external_ids.rs:63`) and `CreateCblListReq` validate by hand outside the `respond_with_field_errors` field-details path.
- **N-A5 (low).** Rate-limit coverage: only 11 `api/` files reference `rate_limit` (account_export, admin_metadata_coverage, auth_config, cbl_lists, csp, issue_ocr, markers, markers_export, opds, opds_v2, provider_ranges). `/auth/refresh` does a DB lookup + write per call with no bucket (`auth/local.rs:63-66`, deliberate); search, series/issue PATCH, collection writes and reorders have none.
- **N-A6 (low).** AR-7: 19 stringly discriminators in `crates/entity/src` (`user.rs:33,36`, `issue.rs:28`, `marker.rs:26`, `metadata_run.rs:31`, `scan_run.rs:11,21`, …) and 0 `DeriveActiveEnum`; `UserRole`/`UserState` already exist as typed query enums (audit-remediation M9) and are the natural template.
- **N-A7 (low).** OP-5: `audit_log` still has no retention or export-and-trim (grep `-i audit` over `jobs/*.rs`, `settings/registry.rs` filtered by `retain|prune|retention` → none; `jobs/hard_purge.rs:33` only nullifies references). Append-only by design; unbounded.
- **N-A8.** Verified clean: AppState collections are bounded (`thumb_path_cache` LRU 4096, `thumb_throughput` capped at 20 000 samples / 120 s, `background_tasks` guard, `library_scan_job_ids` drained); no secrets in tracing calls; `Config` Debug redacts; 9 non-test `unwrap()` sites in handlers all invariant-guarded; TODO census across `crates/` is 19 of which 18 are `'nonce-XXX'` CSP doc strings (one real: `reconcile_status.rs:92` `TODO(future)`), web 0. Security: `deny_all_cors()` (`app.rs:621,688`), `AdminGatedOverride<T>` (9 refs), `MARKER_WRITE` bucket, cover sniff + https, `pepper.previous` rotation, OPDS bucket split, `security-audit.md` status table with a named regression test per row.
- **N-A9.** CI: 15 jobs (`ci.yml`) incl. 4 nextest shards, migration round-trip, openapi snapshot + oasdiff, audit-check (81-entry allowlist), cargo deny/audit + `check-advisory-ignores.py`, pnpm audit, bundle budget + analyzer artifact, SBOM, docker-build, docker-smoke (trivy, compose boot, all Playwright projects, PWA asset check). Release: SPDX SBOM + cosign keyless, amd64 only; `deploy.yml` records a GitHub Deployment. Not gated: arm64, soak/k6, axe on browse / series / issue / admin / settings / downloads / bookmarks routes, hard-purge behaviour on real data; no `jsx-a11y` lint (`grep -c jsx-a11y web/eslint.config.mjs web/package.json` → 0).

---

## 4. Low-hanging fruit (S-sized, half a session or less each)

Each item: file:line and the one-line change. Grouped by the finding it closes.

**Writeback fidelity (N-M2, N-M5, N-M6, N-M7, N-M8)**
1. `crates/server/src/metadata/sidecar_compose.rs:909-918` — delete the private `split_csv`; call `crate::library::scanner::metadata_rollup::split_csv` (already used by its own test at `:2074`). Drop the `_unused: &str` parameter at `:940-942` and the misleading `"writer"` literal at `:399-406`.
2. `crates/server/src/api/external_ids.rs:340` — after the write, call `manual_writeback::enqueue_issue_rewrite` / `enqueue_series_rewrite` (`manual_writeback.rs:102,138`); one test in `tests/manual_writeback.rs`.
3. `crates/server/src/api/series.rs:616-651` — move `am.update` and the `write_field_provenance` loop into one `db.begin()` transaction, mirroring `issues.rs:3085-3098`.
4. `crates/server/src/jobs/metadata_apply.rs:192-195` — requeue with backoff like `rewrite_sidecars::requeue_busy` instead of `return Ok(())`; mark the run failed when the budget is exhausted.
5. `crates/server/src/jobs/rewrite_sidecars.rs:203-207` — return `Err` for transient I/O failures so the explicit `JOB_MAX_ATTEMPTS` budget applies; keep `Ok` for `sidecar_refusal`.
6. `crates/server/src/metadata/apply.rs:2529-2548` — add `MetadataField::Number`, `Notes`, `WebUrl` to `SIDECAR_ISSUE_PROVENANCE_FIELDS`.
7. `crates/server/tests/sidecar_parity.rs` — add a suffixed-name credit case ("Deodato, Jr.") so item 1 has a guard.

**Visibility invariant (N-O1..3)**
8. `crates/server/src/api/collections.rs:1501,1518` — add `access::active_series()` and `issue::Column::RemovedAt.is_null()` plus an `access::` pass in `hydrate_entries`; one test in `tests/collections.rs` with a soft-removed issue and a revoked library.
9. `crates/server/src/api/cbl_lists.rs:884` — add `.filter(issue::Column::RemovedAt.is_null())`.
10. `crates/server/src/api/reading_sessions.rs:~615` — join `issue.removed_at IS NULL` like `visible_marker_condition` (`markers.rs:918-929`).

**Scanner (N-L1, N-L2, N-L5)**
11. `crates/server/src/library/scanner/process.rs:416` — emit a new `IssueKind::EncryptedArchive { path }` (variant at `library/health.rs:30-186`, name at `:223-241`); expose `files_encrypted` in the scan-run stats view; fixture: an AES CBZ under `fixtures/`.
12. `crates/server/src/library/scanner/metadata_rollup.rs:831-833` — `ON CONFLICT DO NOTHING` without an arbiter (as `writers.rs:1056` does) or retry on `23505`; regression test with 8 concurrent rollups.
13. `process.rs:632-637` — `.select_only().columns([Id, FilePath]).into_tuple()` for the dedupe lookup (F-5).
14. `post_scan.rs:449-456` — collect ids and issue one `update_many` per batch (F-4).

**Per-request hot spots (N-L3, N-L4, N-L6)**
15. `crates/server/src/api/duplicates.rs:787-796` — memoise `compute_groups` per `(library_id, last_scan_at, decisions_version)`, or push grouping + keyset into SQL; add `duplicates` to `scripts/perf/endpoints.txt`.
16. `crates/server/src/api/health_issues.rs:62,113` — TTL-cache `scan_backup_files` per library, or compute it in `backup_prune` and store it on the library row; add the health page to `endpoints.txt`.
17. `crates/server/src/api/opds.rs:923-925` — project `recent` through `IssueCardRow` and add `?page=` like the other v1 feeds (`:415-435`).
18. `crates/server/src/api/thumbnails.rs:305` — `private, max-age=300` + ETag (OP-7, roadmap backlog) if the stable-URL rationale allows a 5-minute staleness.

**Ops and architecture (N-A1..3, N-A6)**
19. `crates/server/src/api/archive_edit.rs:722-728` — `tokio::fs::create_dir_all` / `tokio::fs::write`; `:748-760` sweep → `spawn_blocking`.
20. `crates/server/src/api/csp.rs:25`, `api/ws_scan_events.rs:48` — add `#[handler]`; decide whether the six pub helpers should be `pub(crate)`.
21. `crates/server/src/app.rs` — declare `DefaultBodyLimit::max(…)` on the `api` group (1–2 MiB) so the limit is explicit, not inherited.
22. `crates/entity/src/user.rs:33,36` — promote `role`/`state` to `DeriveActiveEnum` using the existing `UserRole`/`UserState` as the AR-7 template.

**Reader and offline (N-R7..N-R12)**
23a. `web/lib/pwa/downloads.ts` + `offline-store.ts` — store the `PageOverridesView` in the `DownloadRecord` at download time; `web/app/[locale]/downloads/OfflineReader.tsx:155-195` passes it as the initial overrides (N-R7).
23b. `OfflineReader.tsx:97-100` — pass `nextUpOverride={nextUp ?? null}` with a stored-series fallback built from `offline-lists.ts:101 summaryFromRecord` (N-R8).
23c. `web/lib/api/queries.ts:~2282` `useIssuePageOverrides` — `retry: false` / `networkMode: "offlineFirst"` when offline (N-R11).
23d. `ReaderSettings.tsx:92` — hide or annotate `contain` when `viewMode === "webtoon"` (N-R10).
23e. `PageStrip.tsx:539-556` — show the spread toggle on coarse pointers (`primaryPointerIsCoarse()` is already imported elsewhere) and give overridden pages `tabIndex=0`; lift `PageStrip.tsx:551`, `WrapUp.tsx:71`, `ReaderSettings.tsx:313` to `text-xs` (closes AC-4).
23f. `Reader.tsx:858` — hoist `RECENTER` to module scope and delete the four `eslint-disable` lines.
23g. `docs/dev/pwa-hardening.md:139-143` — record the iOS edge-back device check when the owner runs WP-4.1.

**Web polish (N-O4, N-O6, N-R3..5)**
23. `web/components/saved-views/IssueFilterViewDetail.tsx` — wire `useSelection` + `SelectionToolbar` as `FilterViewDetail.tsx:43,125` does.
24. `web/components/library/SeriesRelatedSection.tsx:244` — hide chain kinds from the grouped list when `chain.length > 1`.
25. `web/lib/search/actions-registry.ts` — add `go-browse`, `go-views`, `go-log`, `admin-relationships`, `admin-background-work`; update the "27 actions" sentence.
26. Settings — a "Hidden from rails" list using `GET`/`DELETE /me/rail-dismissals` and the orphaned `useRestoreRailItem`.
27. `web/lib/reader/store.ts:268,414` — consider `chromeVisible` default true on first run for coarse pointers; or leave and close AC-2 as "by design".
28. `Reader.tsx:1623,1861` — when the page-text panel has OCR text for the page, use its first line as the image `alt`.

**Docs truth (§2)**
29. The §2 table, one PR: `features.md` (10 rows + re-date), `metadata-providers.md:1514`, CLAUDE.md csv twin, `web/proxy.ts:9`, `.env.example` (remove `_FILE`, add 6 lines), braces → Resolved in three files + `check-advisory-ignores.py`, roadmap purge default, `library-scanner.md` 4 kinds, `archive-writes.md` path + drift-flush row, operator guide 4 rows, `metadata-sidecar-writeback.md` review + primary-cover notes.

---

## 5. Missing features, ranked by owner value

Ranked for a single-household collector with every library on writeback, phone/iPad PWA use, and long runs. Items confirmed excluded by `docs/product-roadmap.md` §2 or `§8` are not listed (guided view, CRDT/shared collaboration, AniList/MangaUpdates before volume-mode matching, PDF/EPUB/loose folders, Meilisearch/pgvector, Kavita-compat API, multi-replica, Kobo/KEPUB, on-device translation, gamepad, AVIF variants, page-curl, configurable tap zones, BZip2 7z, user-data import). Absence verified by the named search.

| # | Feature | Why it matters here | Absence check | Size |
|---|---|---|---|---|
| 1 | **MetronInfo v1.x schema output** (composer + tolerant parser + XSD-shaped parity test) | Every archive on prod gets this file; Codex 2.5.7 and Metron-Tagger read the schema shape | `rg "CoverDate\|<IDS\|\"Arcs\"" crates/parsers/src/metroninfo.rs` → nothing | M |
| 2 | **Local-database backends** (plan M2 GCD, M3 ComicVine over the M1 plumbing) | Settings and probe exist; 4,280 CV-linked series could be enriched offline at zero quota | `rg "local_db::\|LocalBacked" crates/server/src/metadata/{gcd,comicvine,orchestrator}.rs` → nothing | L (planned) |
| 3 | **Sidecar `fill_missing` + primary cover on the writeback path** | Today the mode selector lies on writeback libraries and "Apply cover" is a no-op for the primary | §3.2 N-M3/N-M4 | S–M |
| 4 | **Reviews with a text body** at issue and series level | `user_rating` is a float; the issue closing note (`api/issue_notes.rs`) is the nearest thing; nothing at series level | `rg -l "user_series_note\|series_note\|SeriesNote" crates/entity/src crates/server/src web/components` → nothing | M |
| 5 | **Per-field "lock" control** | Pins arise only as a side effect of editing; the `DELETE …/field-provenance/{field}` endpoint exists (`issues.rs:1195-1202`) but there is no affirmative lock | no "lock"/"pin this field" control in `IssueMetadataTab.tsx` | S |
| 6 | **Barcode / GTIN / ISBN match** | Identifiers are ingested from Metron/GCD (`metron.rs:920-923`, `gcd.rs:1521-1538`) but never used as a lookup key; direct lookup is URL-only (`lookup.rs:173`) | `rg -i "gtin\|barcode\|isbn\|\bupc\b" api/metadata_search.rs metadata/direct_lookup.rs` → exit 1 | S–M |
| 7 | **Reading goals / year-in-review** | The stats pipeline (`reading_sessions.rs:164-207`) makes an annual summary cheap | `rg -l "reading_goal\|year_in_review" crates/server/src web/{components,app,lib}` → one placeholder string in `ConfigureWidgetDialog.tsx:402` | S–M |
| 8 | **Encrypted-archive finding** (N-L1) | A real file class with no surface | §3.3 | S |
| 9 | **Hidden-series management** (durable "hide from my library" + restore UI for rail dismissals) | Hiding exists only as auto-restoring `rail_dismissals`; no restore UI | `rg -ln useRestoreRailItem web -g '!*.test.*'` → only the definition | S–M |
| 10 | **User tags on series/issues** (personal, not metadata) | `series_tag`/`issue_tag` are provider-derived with no `user_id`; markers have per-user tags, items do not | `rg -l "user_series_tag\|personal_tag\|user_tag" crates/entity/src crates/server/src` → nothing | M |
| 11 | **Provider-variant as primary cover** | `covers.rs` is GET-only; `cover_pick.rs:70-74` takes a page index only | `rg -i "set as cover\|make primary\|setPrimary" web/components/library/CoverGallery.tsx` → nothing | S |
| 12 | **Apply history / undo** | Audit rows carry no before-image; a bad apply is fixed by hand | `ls crates/entity/src \| rg -i "history\|snapshot\|revision"` → exit 1 | M–L |
| 13 | **Scanner-triggered auto-search** for new series (DI-17) | New folders wait for a manual match or the weekly refresh | §3.2 N-M9 | S–M |
| 14 | **Holdings export** (CSV of series/issues) and reading-list export beyond `.cbl` | Only the reading log exports CSV (`reading_log.rs:679,749`) | `rg -i "text/csv" crates/server/src/api/*.rs` → reading_log only | S |
| 15 | **Read-only share links** for a collection or list (§2 excludes collaboration, not sharing) | Spec §10 `/s/{token}` was promised for v1.1 | `rg -l "is_public\|public_token\|share_token\|ShareLink" crates/server/src crates/entity/src` → nothing | M |
| 16 | **Library-level stats / disk usage** | `LibraryView` has no counts or bytes (`libraries.rs:78-145`); `/admin/stats` is system-wide | `rg -i "disk_usage\|library_size\|sum\(file_size" crates/server/src/api/*.rs` → 0 | S |
| 17 | **Scan dry-run** (filesystem diff, not DB counts) | `scan-preview` returns `dirty_folders`/`known_issue_count` only (`libraries.rs:460-469,1210`) | `rg -i "dry_run\|dry-run" api/{libraries,scan_runs}.rs` → 0 | M |
| 18 | **Admin self-diagnosis page** (Codex "Doctor": mount writable, unrar present, watcher limits, config problems each with a fix) | `/admin/server` has pieces (`root_path_writable`, watcher tiles, update check) but no one "what is wrong" list | no `doctor`/`diagnos` route under `web/app/[locale]/(admin)` | S–M |
| 19 | **Notifications** (ntfy/Gotify/webhook on scan failure, writeback drop, new release) | Failures land in library events and toasts only | `rg -i "webhook\|discord\|ntfy\|gotify\|apprise" crates/server/src web/app web/components web/lib` → none | M |
| 20 | **arm64 images** | Owner-deferred (roadmap 2026-10-01); Raspberry Pi / Apple-silicon homelabs excluded | `release.yml:169,180` `platforms: linux/amd64` | S (CI time) |
| 21 | `DELETE /me` self-service account deletion (spec §17.10 promised with the export) | No handler | `rg 'path = "/me"' crates/server/src/api/*.rs` → nothing | S |
| 22 | **Reader colour filters beyond brightness/sepia** (grayscale / invert "night", warmth, contrast) | `store.ts:177-181` holds only `brightness` and `sepia` | `rg -i 'invert\|grayscale\|contrast\(' 'web/app/[locale]/read' web/lib/reader` → two comments about arrow-key inversion | S |
| 23 | **Touch pinch as transform zoom** (N-R9) | the one 2026-09 reader complaint a phone user still meets; `zoom.ts` already has `zoomAboutPoint` | `Reader.tsx:1158-1163` hands pinch to the browser by design | S–M; decision first (features.md calls the current behaviour deliberate) |
| 24 | **Reader prefs that follow the account** for brightness / sepia / zoom-persist | four per-series slices and the account defaults sync (`series-prefs-sync.ts`); these three are localStorage-only (`store.ts:143-153`) | — | S |
| 25 | **Auto-crop margins**, **on-screen reading timer**, **per-series zoom memory** | common in native readers; `session.ts:212-219` already accumulates `active_ms` | `rg -i 'auto.?crop\|trim.?margin'` → 0; `rg -i 'reading.?timer\|stopwatch'` → 0; zoom persistence is one global boolean (`store.ts:133`) | S each |
| 26 | **Metadata-driven file organiser** (YACReader 10.3) | Deliberately out: library deletion and the scanner never rename or move (`archive-writes.md:33-40`); moving files is a separate decision from "never write archives" | `rg -i "rename_file\|organize\|naming_template\|relocate" crates/server/src/{api,jobs,library}` → none | L; decision first |

Present at v0.70 and therefore **not** proposed: view folders (`view_groups`), issue ordering across series in collections (`collection_sequence.rs`), bulk mark-series-finished (`progress.rs:1103`), per-series missing-issues report, bulk re-match with `scope=unmatched|stale|all|recent`, provider budget bar, 30-day cover-hash cache, manual provenance reset, folder browser, coverage analysis across three providers, offline list downloads, end-of-issue rate/note/review.

---

## 6. Performance: measured and unmeasured

**Measured.**
- **Query plans at 50k issues** (`docs/dev/load-testing.md`, 2026-09-30 → 2026-10-14): `just perf-explain` builds 2,500 series × 20 issues with `--rich` metadata (550k `field_provenance`, 150k `issue_characters`), ingests through the real scanner, seeds per-user activity, then captures `auto_explain` plans for 12 endpoints with a gate that fails on a seq scan of any table ≥ 5,000 rows. Recorded before/after Σ SQL ms per endpoint (e.g. `issues-browse-recent` 57.9 → 4.3 ms), an `oha` pass per endpoint (215–2,065 req/s), the M7 relationship/similarity surfaces (WP-8.3), rails ACL-before-LIMIT (#1170), and the reverse-FK index set (#1174). "No committed pass/fail thresholds" beyond the seq-scan gate (`:618-622`).
- **Query-count guards** (`crates/server/tests/perf_regressions.rs`): 10 async tests incl. collections bulk-add (observed 3, was 104) and a ~500-link relationships guard.
- **Scanner throughput**: 67 GB / 1,395 files cold in ~15 s on NVMe (`scanner-perf.md`, **2026-05-09**, never re-run); "about 100 files/s for 50,000 rich-metadata CBZs, debug build, thumbnails off" (`load-testing.md:629-631`, 2026-09-30).
- **Reader bundle**: 130.13 KB gzip first-load across 14 chunks against a 145 KB gate, measured in CI on every web PR with the analyzer report retained (`pwa-performance.md:50-52`, `check-bundle-size.mjs`).
- **Local SW smoke** (2026-09-18, headless Chromium, no Rust backend): worker ready 54–66 ms; offline document navigation 34–44 ms.

**Not measured.**
- A physical phone or tablet: cold/warm usable library, first readable page, page-turn p95, transfer bytes, retained pixels — every cell of the `pwa-performance.md` table is "Not measured". WP-4.1 is owner-run and still open.
- Soak / concurrency: `oha` is one endpoint at a time; spec §16.5 k6 soak is explicitly "not covered" (`load-testing.md:624`); `rg -l -i soak crates/ justfile .github` → none.
- NAS or spinning-disk scan throughput (`rg -i "NAS|spinning|HDD" docs/dev/scanner-perf.md` → 0); the watcher beyond the 1,000-file test; release-build scan at 50k; >50k issues or >2,500 series.
- The duplicates page, the health page's `.bak` walk, hard purge, and the OPDS acquisition feeds at 50k — none is in `scripts/perf/endpoints.txt`.
- Memory: 24 MP prefetch budget and webtoon mount window are tuned blind; archive-open memory for CB7 conversion is capped (256 MiB) but not profiled.

Inference: at the owner's ~2.5k-series library none of the O(library) paths will be felt; the first to surface at 50k is the duplicates page (full in-memory grouping per page), then the health page on a NAS.

---

## 7. Open questions for the owner

1. **MetronInfo schema**: ship the v1.x composer as one WP (composer + tolerant parser + XSD-shaped parity), or stay pre-schema and document it? Every prod archive is affected either way; the rescan re-ingests whatever is written.
2. **Hard purge default**: the roadmap says off, the code says on at 60 days. Which is intended?
3. **`fill_missing` on writeback libraries**: implement the mask in the composer (fill only fields the archive lacks), or remove the mode from the dialog when the library is writeback?
4. **Local-database backends (plan M2/M3)**: still wanted? The M1 plumbing is live and idle.
5. **`chromeVisible` default**: keep hidden-by-default (AC-2) with the first-run hint, or show on first run for coarse pointers?
6. **Real-device baseline (WP-4.1)**: a 30-minute protocol on the owner's phone and iPad would turn the biggest unknown in this audit into numbers.
7. **arm64**: still deferred?
8. **Specials in completeness**: `features.md` promised a per-user preference and per-series override that were never built; delete the promise or build it?

---

## 8. Comparable tools (web research, 2026-10-31)

Versions and dates are from GitHub release APIs and official changelogs; UNVERIFIED marks anything not pinned to a primary source.

| Tool | Version (date) | Relevant to Folio since 2026-09 | Source |
|---|---|---|---|
| Komga | 1.29.2 (2026-10-28) | 1.29.0: "notable entities" (characters/teams/locations from ComicInfo, per-book editable, searchable, `/api/v3/notable-entities`); OAuth2 in NextUI; DB-backed sessions; RFC 9457 errors; KOReader hash fixes. 1.28.0: contextual prev/next within series/read list, "part of these read lists / collections". 1.27.0: paste cover from clipboard, edit dialogs, RTL locales. Longstanding: Kobo sync, komf hook | github.com/gotson/komga/releases |
| Kavita | stable 0.9.1.4 (2026-09-02); nightly 0.9.1.16 (2026-10-28, `develop`, not a release) | nightly: scanner change-detection 3.5 min → 7 s on 1,496 folders + self-managing dismissable media-error list (#4983); folder-watcher rework with polling where inotify can't see network mounts, on by default (#5009); "Ambiguous Series" media issue (#5000); event widget with upcoming tasks, replay, re-trigger (#4969). Stable: Hardcover OAuth + recommendations (Kavita+), MangaBaka URL change | github.com/Kareadita/Kavita/releases, PRs #4983 #5009 #4969 |
| Codex (ajslater) | 2.5.7 (2026-10-28) | series end-year/status/summary round-tripped through **MetronInfo v1.2**; Metron API-key only; "Ask" match mode; **Admin Doctor** tab (archive tools, codecs, DB, watcher limits, config problems with fixes); disappeared comics kept 1 day + admin list; overlapping-library guard; **Reprints** unified from MetronInfo/ComicInfo/CoMet as reading orders; online-tagging Effort setting | github.com/ajslater/codex/releases |
| Mylar3 | 0.8.3 (2025-08-17); dev last commit 2026-03-25 | no release since 2026-09; dev: paged API. Longstanding gap vs Folio: acquisition (Wanted lists, weekly pull list, NZB/DDL search, post-processing). Low activity | github.com/mylar3/mylar3 |
| YACReader | 10.4.0 (2026-10-09) | server API to set read/unread/completed/type from clients; web UI context actions; 10.3.x: in-app folder rename preserving state, experimental metadata-driven **organizer**, drop image to set cover | github.com/YACReader/yacreader/releases |
| ComicTagger | 1.6.0-beta.10 (2025-12-07); dev 2026-09-06 | unchanged; GCD/Metron talker plugins, quick tag by image hash | github.com/comictagger/comictagger |
| Komelia / komf | 0.20.0 / 2.1.0 (2026-09-27) | komf: MangaBaka database linking API; BookWalker moved to offline DB; dropped Nautiljon/Hentag/Kodansha | github.com/Snd-R/{Komelia,komf}/releases |
| Panels (iOS/macOS) | 3.13.10 (blog 2026-10-27); App Store shows 3.13.9, year not displayed — UNVERIFIED | clearer OPDS/Komga/Kavita connection errors, Komga progress write-back, retry of failed uploads; **Panels 4 "by end of year"**: panel detection core, Dual View, rebuilt reader; OPDS 2.0 still "planned" | panels.app/blog |
| Mihon | 0.21.3 (2026-10-25) | local source reads chapter dates from ComicInfo.xml; duplicate-entry merge; Comick tracker | github.com/mihonapp/mihon/releases |
| Ubooquity | 3.1.0 (Aug 2025; vendor pages give 10 vs 18 Aug) | dormant | vaemendis.net |
| Suwayomi-Server | 2.4.2366 (2026-09-30) | SyncYomi v2 sync protocol; peripheral | github.com/Suwayomi/Suwayomi-Server |

**Provider landscape.** ComicVine unchanged (200 req/resource/hour, non-commercial, link-back). Metron: 20/min + 5,000/day since March 2026 with `X-RateLimit-Burst-*`/`Sustained-*` headers, 429 + `Retry-After`, ETag and `If-Modified-Since` (304s still count); token auth added server-side as "part of the Basic Auth deprecation migration" (commit 2026-07-27); donor tiers to 25,000/day UNVERIFIED against Metron's own page (bot protection blocks fetchers); Codex dropped Metron passwords for API-key only in 2.5.7 — Folio's `metadata.metron.api_token` (WP-2.9) is the right side of this. GCD: the live REST root exposes only `series` and `publisher` (233,974 series, no auth, no rate-limit statement); no issue endpoint, so the SQL dump / local file route (plan M2) remains the practical path. MangaBaka: official docs unreachable; base `api.mangabaka.org/v1` plus a full SQLite dump per komf's client source; limits UNVERIFIED. Hardcover: GraphQL, scoped PATs, OAuth incl. device grant, Free 5,000/day · 60/min, Supporter 50,000/day; book-centric. Marvel API retirement not re-checked (UNVERIFIED this pass).

**What the competitors have that Folio lacks** (deduplicated against §5): Kobo sync (Komga; excluded), Admin Doctor (Codex → §5 #18), MetronInfo v1.2 round-trip (Codex → §5 #1), reprints as reading orders (Codex; Folio persists `issue_reprints` since WP-7.8 but has no reading order over them), "Ask" match mode and Effort setting (Codex; Folio's auto-apply threshold + review queue cover most of it), acquisition pipeline (Mylar3; out of scope), metadata-driven organiser (YACReader → §5 #22), Hardcover/MangaBaka book-side providers (Kavita+/komf; excluded until volume-mode matching), native clients with offline (Komelia/Panels via Komga shim — Folio's PWA offline covers the owner's case), clipboard-paste cover (Komga; Folio picks from pages and provider variants), notable entities (Komga 1.29 — parity with Folio's entity pages, not a gap), Kavita's event widget replay/upcoming (Folio's `/admin/background-work` shows in-flight and queued but not upcoming scheduled runs — `rg -i "upcoming|next_run" api/background_work.rs` → only a `scheduled` count).

**What Folio has that none of them have** (verified in code this pass): page-anchored markers with OCR capture and page-hash drift anchoring; the `series_provider_range` divergence model with set-cover coverage analysis across three providers; archive-canonical sidecar writeback with drift detection and a "files stay clean" gate on every writer; cover-pHash-primary matching with provider-complete park/resume on the same run; GCD as a full provider plus local-database plumbing; typed series relationships with detectors, review queue, and IDF-weighted explainable similar series; smart views at series and issue level feeding rails, OPDS and custom pages; one server speaking Komga-compat + KOReader sync + OPDS 1.2/2.0 + PSE + Progression; a server-side archive page editor with `.bak` rotation; a duplicates page keyed on hash, number and external ID; reading-run progress semantics with a durable offline outbox.

---

## 9. Suggested sequencing

**Slice A — writeback fidelity (one to two sessions).** Low-hanging items 1–7, then the MetronInfo v1.x composer (§5 #1) behind the existing `sidecar_parity.rs` + `writeback_provider_ingest.rs` harness, then `fill_missing` + primary cover on the sidecar path (§5 #3). Exit: a Folio-written archive round-trips through ComicTagger 1.6 and Metron-Tagger with no diff on shared fields, and a hand edit to an external ID reaches the file.

**Slice B — visibility, findings, offline fidelity (one session).** Items 8–12 (visibility + encrypted finding) and 23a–23c (offline overrides, offline Up Next, offline retry). Exit: `removed_series_hidden_from_lists_rails_and_opds` gains collection, CBL and session cases; an AES CBZ produces a health row; an offline open of a fixed manga issue pairs as it did online and the end card resolves without the network.

**Slice C — scale and ops hygiene (one session).** Items 13–22 plus `duplicates` and `health` in `endpoints.txt`, re-run `just perf-explain`. Exit: no O(library) work per page request on the two pages; no blocking I/O in handlers; body limit declared.

**Slice D — docs truth 2 (half a session).** Item 29 as one PR.

**Owner-run.** WP-4.1 device baseline; decisions in §7.

Held, in order of likely value: local-database backends (§5 #2), reviews with text (§5 #4), per-field lock (§5 #5), barcode match (§5 #6), goals / year-in-review (§5 #7), Admin Doctor (§5 #18), notifications (§5 #19), arm64 (§5 #20).
