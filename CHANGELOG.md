# Changelog

All notable changes to Folio are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (pre-1.0:
minor = features, patch = fixes/polish).

Versioning note: the crate/package manifests stay at `0.0.0` on purpose —
**the git tag is the version**. The running build reports it via
`COMIC_BUILD_TAG` (set from the tag at image-build time). See
[docs/dev/releasing.md](docs/dev/releasing.md) for the release ritual.

Releases before v0.7.2 are recorded only as Git tags + GitHub Releases;
this file starts at the first release that ships with a curated changelog.

## [0.10.3](https://github.com/mbryantms/folio/compare/v0.10.2...v0.10.3) (2026-06-11)


### Fixed

* **deps:** update dependency react-hook-form to v7.78.0 ([#123](https://github.com/mbryantms/folio/issues/123)) ([5851c3e](https://github.com/mbryantms/folio/commit/5851c3e79821b383608a12f84e4d1635f952957e))
* **jobs:** chunk scan-event writes, self-heal scan-coalescing keys, and surface dead-lettered jobs ([#133](https://github.com/mbryantms/folio/issues/133)) ([59e1ee4](https://github.com/mbryantms/folio/commit/59e1ee4b3582b17f2ceeaa0b4dd73c3eb5d8f917))
* **jobs:** stop scan_series retry churn on recorded failures (OPS-3 tail) ([#135](https://github.com/mbryantms/folio/issues/135)) ([6e49eaf](https://github.com/mbryantms/folio/commit/6e49eaf013c8b81b6273267845a1cc1e7e1439cd))
* **security:** trust X-Forwarded-For only from configured proxies, bound image decode, harden SSRF + archive-write paths ([#131](https://github.com/mbryantms/folio/issues/131)) ([1ab7b70](https://github.com/mbryantms/folio/commit/1ab7b709325c87837507a09dee36d8470adb9ff8))


### Changed

* cache app-password auth, stream page bytes, and honor conditional (304) requests ([#132](https://github.com/mbryantms/folio/issues/132)) ([0f4e772](https://github.com/mbryantms/folio/commit/0f4e772192ceea48c096f8fa72336dca72c02594))
* serve uncompressed pages lock-free and trim read-path overhead ([#136](https://github.com/mbryantms/folio/issues/136)) ([b2166e0](https://github.com/mbryantms/folio/commit/b2166e08bcc8f2d1baecb3021a1f1bba1f3784be))

## [0.10.2](https://github.com/mbryantms/folio/compare/v0.10.1...v0.10.2) (2026-06-09)


### Fixed

* dedup On Deck cards by issue id ([#129](https://github.com/mbryantms/folio/issues/129)) ([d9d8f23](https://github.com/mbryantms/folio/commit/d9d8f2329446f88589a2286938aae73ad9dbe826))

## [0.10.1](https://github.com/mbryantms/folio/compare/v0.10.0...v0.10.1) (2026-06-08)


### Fixed

* **security:** harden auth and unsafe IO ([#114](https://github.com/mbryantms/folio/issues/114)) ([f71ff93](https://github.com/mbryantms/folio/commit/f71ff93f540487f7d2599cbb1d84749009f32d2f))

## [0.10.0](https://github.com/mbryantms/folio/compare/v0.9.5...v0.10.0) (2026-06-08)


### Added

* **auth:** opt-in OIDC auto-link to local accounts by verified email ([#120](https://github.com/mbryantms/folio/issues/120)) ([db90d31](https://github.com/mbryantms/folio/commit/db90d31ad0b8795541cd63d24b8a484ea2f25fa4))


### Fixed

* **deps:** update radix-ui ([#118](https://github.com/mbryantms/folio/issues/118)) ([ea36555](https://github.com/mbryantms/folio/commit/ea36555d6b0a8dc2cd2c209d7fb7753ebd2145a6))
* **deps:** update tanstack to v5.101.0 ([#124](https://github.com/mbryantms/folio/issues/124)) ([a8a2d15](https://github.com/mbryantms/folio/commit/a8a2d1594bc39e2424e14a4145882010da178c49))

## [Unreleased]

### Fixed

- **Security hardening for auth, imports, and operational endpoints.** Password
  reset links are now single-use, first-admin bootstrap is serialized under
  concurrent signups, auth cookies use `__Host-` names with signed OIDC state,
  unsafe secret-file permissions are refused at startup, provider/CBL fetches
  reject internal-network URLs and oversized bodies, and production metrics
  require a bearer token unless explicitly opened.

### Internal

- Release workflow: a new `prepare release` dispatcher can stamp the
  changelog, open and auto-merge the changelog PR, create the release tag, and
  hand off to the image-publishing workflow without the local/manual release
  ritual.
- Docs dependency audit: Docusaurus transitive `uuid` callers are pinned to a
  patched `uuid` release.

## [0.9.5] - 2026-06-07

### Fixed

- **Duplicate provider IDs no longer wedge a scan.** When two files for one
  issue carried the same ComicVine/Metron/GTIN id, the second file&rsquo;s scan
  used to abort on a unique-constraint violation and then retry forever. The
  scanner now skips the already-claimed id and raises a **Duplicate external
  ID** finding under Admin&nbsp;→ Findings instead, so the scan completes. The
  surviving file automatically reclaims the id once a duplicate is removed (no
  more manual database cleanup), and manually adding an id that&rsquo;s already
  assigned now returns a clear conflict error.

## [0.9.4] - 2026-06-06

### Added

- **Bulk-fetch only missing or partial metadata.** A series&rsquo; bulk
  metadata fetch can now be scoped to just the issues whose metadata is
  incomplete (partial or missing) rather than every issue — saving provider
  budget and keeping the Review queue focused. The Series&nbsp;… menu&rsquo;s
  three metadata actions are grouped into one **Fetch metadata** submenu (Match
  this series · All issues · Only missing or partial).

## [0.9.3] - 2026-06-06

### Fixed

- Issue page: the "More in series" strip now stays on one horizontal rail on
  mobile instead of wrapping the previous and up-next cards.

## [0.9.2] - 2026-06-06

### Added

- **Bulk "Fill missing" / "Replace all" in the Review queue.** The metadata
  batch Review tab's _Needs review_ section gains one-click bulk actions that
  auto-apply the most-complete metadata merged across every provider that
  matched (covers prefer ComicVine), with an All / Selected scope — clearing
  the review queue without opening each item one at a time. Your pinned fields
  are preserved.

### Changed

- **Issue and series detail pages are easier to scan.** Details tabs now use
  card-based summary sections, avoid reserving empty space from large tabs, hide
  redundant provider web links, and keep empty series genre/tag fields out of
  the page header.

### Fixed

- Metadata matching: zero-padded issue numbers (e.g. `014`) now match a
  provider's unpadded number, and series search no longer hard-filters on a
  too-strict start year — fixing spurious "no matches" on issues that clearly
  exist at the provider.

### Internal

- CI/release: docs-only PRs skip redundant build work, and the release-tagging
  workflow trims steps that re-ran needlessly.

## [0.9.1] - 2026-06-06

### Added

- **Previous-issue cover on the issue page.** The in-series rail now shows the
  preceding issue's cover to the left of the next-issue strip (omitted on the
  first issue of a series); the section is retitled "More in series."
- **Clear detected (OCR) text on a marker.** The marker editor gains a "Clear"
  control to drop a highlighted region's OCR'd text.

### Changed

- **Issue & series tabs reorganized.** Both pages now lead with the same tab
  order (Credits · Cast & Setting · Details · …), and the standalone
  "Genres & Tags" tab folds into Details. Tab contents are regrouped into
  scannable categories that use the full width — full-width credit/cast rows,
  the Details fields split into Publication / Format / Library sections, and
  the issue Metadata tab's status moved into a card row.

### Fixed

- Activity tab: the ranking-dimension selector no longer overflows off the
  screen on mobile (it scrolls within the control instead).
- Home rails: an in-progress issue shown in **Continue Reading** no longer also
  appears in **On Deck**.

### Internal

- Clearing a marker's `selection` / `body` / `region` / `color` by sending
  `null` now works (a `double_option` deserialize fix; previously a silent
  no-op).
- New `GET /series/{slug}/issues/{slug}/prev` endpoint backing the
  previous-issue cover.
- Tooling/CI: a pre-commit `cargo fmt` guard (`.githooks` + `just bootstrap`);
  CI runs on `merge_group`; Renovate uses GitHub-native auto-merge
  (`platformAutomerge`); the release ritual is adapted to a protected `main`
  (changelog lands via PR, only the tag is pushed). Plus a developer-workflow
  cheat sheet under `docs/dev/`.

## [0.9.0] - 2026-06-05

### Added

- **Bulk-metadata Review queue.** A bulk fetch ("fetch all issues in a
  series", a saved view, a library refresh) now groups its per-issue/series
  runs into a single batch with live aggregate progress and one consolidated
  accept surface in `/admin/metadata` → **Review**: one-click "Accept all
  strong", per-item review that reuses the candidates already pulled by the
  batch (no re-search), and a fresh search only on no-match.
- **Metadata completeness.** Issues and series are scored against a
  provider-complete baseline (matched + cover date + summary + page count +
  a credit + cover; title/characters/arcs/genres surfaced as gaps but
  non-gating). The tier drives a card/list badge, a new series **Collection**
  tab (ownership gaps + per-issue completeness coloring), and a saved-view
  filter so you can build a "needs metadata" view.
- **Issue Metadata tab.** A per-issue overview of provenance (field → source
  → when), which sidecar files Folio has (ComicInfo / MetronInfo /
  series.json), and freshness (last synced / last rewritten).
- **Auto-resume for quota-parked fetches.** Runs parked at `awaiting_quota`
  when every provider is out of budget now resume on their own once the
  window passes, reusing the stored entity + batch so a large bulk fetch
  finishes without a re-trigger.

### Fixed

- Sign-in: auth tabs stay full-width on tablet/desktop.

### Internal

- New migrations: `metadata_batch` + `metadata_run.batch_id`; sidecar-presence
  columns (`issues.metroninfo_present`, `series.series_json_present`, both
  nullable so `NULL` reads as "unknown until next rescan", distinct from a
  definite absent).

## [0.8.1] - 2026-06-04

### Added

- **Observability: two non-overlapping admin streams.** The old unified
  `/admin/activity` feed is split into a **Server stream** (app-runtime logs +
  audit + user activity) and a **Library stream** (a durable, itemized record
  of scans, thumbnails, covers, metadata, and archive rewrites).
  - **Library activity** (`/admin/findings`): a durable `library_events`
    manifest of every change — issue/series add·update·remove·restore,
    thumbnail / metadata / archive ops — with expandable rows showing target,
    error, series, and on-disk path, alongside the health-issue and scan-run
    tabs.
  - **Scan dashboard** (`/admin/scan-dashboard`): live aggregate progress
    across a "Scan all" run — per-library status, overall completion, and a
    post-run summary of what changed — backed by a new `scan_batch` grouping.
  - **Server log** (`/admin/logs`): a Server/Library stream toggle and an
    error-code facet (every API error is captured with its `error_code`).
  - New read endpoints: `GET /admin/scan-batches[/{id}]` and
    `GET /admin/library-events`. See `docs/dev/observability.md`.

### Changed

- `/admin/activity` ("Server activity") is now audit + reading volume only;
  scan and health moved to the Library stream so the two never overlap. Nav:
  "Logs" → "Server log", "Activity" → "Server activity", plus new "Scan
  dashboard" and "Library activity" entries.

### Internal

- New migrations `library_events` + `scan_batch`; a daily retention prune
  bounds the event manifest (90 days / 50k rows per library).

## [0.8.0] - 2026-06-04

### Added

- **Phase 1 UX + architecture improvements** (#90). System theme option with
  SSR-safe hydration; backend bulk-selection operations ("all matching") plus
  an explicit "Select loaded" action; search category totals + cursor
  pagination with page-region thumbnails on marker/bookmark result cards;
  admin findings / health-issues / scan-runs tables moved to infinite-query
  pagination; `/me/account` now surfaces `email_editable` / `password_editable`
  so it only offers edits the active auth mode supports. See
  `docs/dev/ux-architecture-improvement-plan.md`.

### Changed

- **Dependency catch-up (round 3 + round 4).** Rust toolchain 1.91.1 → 1.96.0
  (+ constant_time_eq 0.5); postgres/redis completion, imageproc 0.27,
  axum-extra 0.12; web in-range bumps (next / react / react-dom 16.2.7 /
  19.2.7), openapi-typescript 7.4 → 7.13, blake3 1.8.5.
- **pnpm 10.33.2 → 11.5.1.** Security `overrides` moved to `pnpm-workspace.yaml`
  (pnpm 11 no longer reads the `pnpm` field in package.json); skipped native
  build scripts recorded via `allowBuilds`. pnpm 11 also enforces a default
  24h `minimumReleaseAge` supply-chain gate.
- **Lock-file maintenance** (#82): `@playwright/test` 1.59.1 → 1.60.0 plus
  transitive/dev-tooling refreshes (docs-site `@swc/core`,
  `@algolia/client-search`, webpack, react 19.2.7 propagation).

### Fixed

- **CI OpenAPI-drift job** had failed on every branch since the workflow
  regressed: it exec'd `openapi-typescript` from the repo root (where the dep
  doesn't exist) under suppressed stderr, and the downstream `oasdiff` step
  invoked `./oasdiff` instead of the on-PATH binary. Both fixed (#89).
- Offline toast no longer claims changes will be queued; transient failures
  keep an explicit retry path (#90).
- Series "Read from beginning" routes via the slug-based reader URL helper
  instead of the stale `/read/{issueId}` path (#90).

### Internal

- Branch-protection required checks updated to the Docker matrix job names so
  PRs are mergeable without an admin override.

## [0.7.23] - 2026-06-02

### Changed

- **Dependency catch-up (round 2).** Major/migration bumps across the stack:
  postgres 17 → 18 and redis 7 → 8 (dev + test containers), apalis 0.6 → 0.7
  (+ redis crate 0.27 → 0.32), fast_image_resize 5 → 6, and out-of-range rust
  0.x crates (imageproc 0.26, testcontainers 0.27, metrics-exporter-prometheus
  0.18, tokio-cron-scheduler 0.15). CI runner actions bumped to current majors
  (checkout v6, setup-node v6, docker/\* v7/v6/v4, cosign v4) and the
  `docker/dockerfile` syntax + dev `dex` image tags refreshed.

  **Operator note:** an existing dev `.dev-data/postgres` directory is
  PG17-initialized and will not start under PG18 — run `just dev-services-reset`
  (wipes the local dev DB) when adopting. Fresh installs and CI are unaffected.

### Removed

- Dropped the unused `notify` + `notify-debouncer-full` dependencies (declared
  but referenced nowhere).

### Internal

- Renovate tuned: `rangeStrategy` → `update-lockfile` (stops cosmetic
  manifest-floor churn), coordinated groups for cross-pinned crate sets, and
  `yaml` pinned to 1.x (override-only security pin for the docs toolchain).

## [0.7.22] - 2026-06-02

### Changed

- **Dependency refresh.** In-range lockfile updates across both stacks —
  Rust (`cargo update`, 36 crates incl. hyper 1.10, serde_json 1.0.150,
  opentelemetry_sdk 0.32.1) and web (`pnpm update`) — plus behind-by-minors
  bumps for `garde` (0.23), `lru` (0.18), and `infer` (0.19). No runtime
  behavior changes; all gates green.
- **Renovate coordinated groups.** `renovate.json` now groups the
  cross-pinned crate sets that previously surfaced as conflicting standalone
  bumps: `sea-orm + sqlx`, `apalis + redis`, and the RustCrypto
  digest/rand ecosystem (`sha2`/`hmac`/`rand`/`argon2`/`rsa`/…).

## [0.7.21] - 2026-06-02

### Fixed

- **Dead clicks after a dialog or menu closes.** Radix overlays set
  `pointer-events: none` on `<body>` while open; if the close raced a
  navigation, the lock could stick and silently kill every click on the
  page ("no actions taken"). The reset now runs on every route change
  (forward and back) instead of only when the shell first mounts, so any
  navigation un-sticks it.
- **Stalled page transitions now recover on their own.** A new loading
  watchdog mounts inside the library `loading.tsx`: if a route's content
  stays pending past ~15s (a proxy/upstream or RSC-stream stall the App
  Router can't otherwise escape), it hard-reloads the destination URL,
  with a per-URL guard so it never loops. No more spinning on the loading
  skeleton until a manual force-quit.

## [0.7.20] - 2026-06-01

### Fixed

- **iOS Safari / installed-PWA navigation hang.** The first client-side
  navigation after a fresh page load (e.g. tapping a creator pill) could
  hang on the loading skeleton, after which every link went dead until a
  reload. Root cause: the service worker's `clientsClaim` seized a page
  that had loaded _without_ the worker, and on WebKit the first RSC
  navigation through that mid-session-claimed worker never resolved. The
  worker no longer claims already-open pages, disables navigation preload,
  and hands **all** navigation/RSC requests straight to the browser — so it
  can never stall a route transition. (Supersedes the per-route allowlist
  from v0.7.19. As before, fully close/reopen the PWA — or reload the tab —
  once after upgrading to pick up the new worker.)
- **Pills now land at the top of the destination page.** Tapping a credit
  chip (→ creator page) or a cast/setting chip (→ filtered library grid)
  from a scrolled-down page could open the new page scrolled down with its
  header clipped off the top. Forward navigations within the library now
  reliably scroll to the top; back/forward still restore the previous
  scroll position.

## [0.7.19] - 2026-06-01

### Added

- **"Back to this issue" on the end-of-issue card.** The reader's up-next
  card now offers a direct link back to the current issue's detail page
  alongside the "Read" button, so you can leave to the issue you just
  finished without first advancing to the next one.

### Fixed

- **The installed PWA can now open creator pages (and other detail
  pages).** Tapping a writer/penciller credit links to `/creators/<slug>`,
  but that route — along with `/read/`, `/settings/`, `/bookmarks`, and
  `/pages/` — was missing from the service worker's native-loader bypass
  list. In the installed app the client-side navigation fell through to the
  worker's cache and hung; in a normal browser tab it worked. All
  client-navigable detail routes are now handed straight to the browser
  loader like `/series/` already was. (Takes effect once the updated
  service worker activates — fully close and reopen the PWA after upgrade.)
- **Full-width reader pages now start at the top after every page turn.**
  Tapping or swiping to a page whose image hadn't been prefetched could
  leave the viewport parked partway down the new page; scroll anchoring is
  now disabled on the reader and the top position is re-asserted once the
  page decodes.
- **Webtoon page jumps no longer flicker through intermediate pages.** A
  programmatic jump (page strip, keyboard, resume) is no longer dragged to a
  page it scrolled past mid-animation.

### Changed

- **Above-the-fold rail covers load eagerly (LCP).** The first row of cover
  images on the home rails is fetched with high priority instead of lazily,
  improving the largest-contentful-paint on the landing surface.

## [0.7.18] - 2026-06-01

### Added

- **Expanded Prometheus metrics at `/metrics`.** Added the service-level signals
  that were missing: HTTP request rate/latency/errors (`folio_http_requests_total`,
  `folio_http_request_duration_seconds`), process/runtime gauges
  (`folio_process_*` — CPU, RSS, file descriptors, threads), per-job outcomes +
  duration (`folio_jobs_processed_total`, `folio_job_duration_seconds`), and
  job-queue backlog (`folio_jobs_queue_depth`). The endpoint is unauthenticated
  by default; set the new **`COMIC_METRICS_TOKEN`** to require an
  `Authorization: Bearer` header on scrapes. Full catalogue + scrape config in
  [docs/dev/metrics.md](docs/dev/metrics.md).
- **Automated dependency monitoring.** Renovate (`renovate.json`) opens grouped
  update PRs and auto-merges safe patch/minor after CI; the weekly security
  workflow gains an OSV-Scanner sweep over both lockfiles.

### Changed

- **Node runtime upgraded 22 → 24 (Active LTS).** The web build + runtime images
  move to `node:24` / `distroless/nodejs24-debian12`; `@types/node` tracks 24.
- **The server now reports its real version and name.** The startup log
  (`Folio starting`), `/healthz`, `/readyz`, `/admin/server`, and every outbound
  HTTP `User-Agent` now show the build tag (e.g. `v0.7.18`) instead of the
  `0.0.0` / `comic-reader` placeholders.
- **Frontend dependency refresh.** All npm advisories resolved; TanStack Query
  5.100, react-hook-form 7.77 + resolvers 5.4, plus a sweep of safe Radix/UI
  bumps.
- **⚠️ Prometheus metric names renamed `comic_*` → `folio_*`** (every metric).
  **Update any Grafana dashboards or alert rules** that reference the old names.
- **JWT audience renamed `comic-reader` → `folio`.** Verification still accepts
  the legacy audience during the transition window, so existing sessions are
  **not** forced to re-authenticate on upgrade.

### Removed

- The dead, never-wired `openapi-fetch` client and the inert
  `COMIC_OTLP_ENDPOINT` env var (OTLP export was considered and dropped for v1;
  see incompleteness-audit §D-9).

### Fixed

- **The UI no longer locks up after saving an archive edit.** Removing a page
  (or any page-editor save) closed the confirm dialog and the editor in the same
  tick as the background `router.refresh()`. Radix sets `pointer-events: none` on
  `<body>` while a dialog is open and restores it on close; closing two nested
  dialogs while a soft RSC refresh ran raced that restore, and since the refresh
  doesn't remount the app shell (whose mount effect clears the lock), the whole
  page stayed unclickable until a hard refresh. The save now defers the refresh
  past the dialog close and clears any residual body lock itself.

### Security

- Resolved all outstanding npm advisories: a build-time PostCSS XSS in the web
  app, plus three High + several moderate transitive advisories in the docs-site
  build tooling (lodash, serialize-javascript, js-yaml, yaml) via root
  `pnpm.overrides`. One dev-server-only, non-exploitable advisory (sockjs → uuid)
  is documented as an accepted exception in `SECURITY-EXCEPTIONS.md`. None of
  these were reachable in the shipped server or web runtime.

## [0.7.17] - 2026-05-30

### Changed

- **"Generate page thumbnails" now queues only the issues that actually need
  them.** It previously enqueued one strip job per _active_ issue regardless of
  whether the page thumbnails already existed — so on a near-complete library it
  flooded the queue with tens of thousands of redundant jobs (the worker skipped
  each one after a disk check, but the queue depth was meaningless and took
  hours to drain). The enqueue path now does that same disk check up front and
  pushes jobs only for issues whose strips are missing or incomplete; issues
  with an unknown page count still enqueue so the worker can reconcile from the
  archive. The log line reports how many already-complete issues were skipped.

## [0.7.16] - 2026-05-30

### Added

- **The scanner now ingests CBR comics.** Previously a `.cbr` was recognized
  but skipped with an `UnsupportedArchiveFormat` health issue. A new per-library
  setting, **Convert CBR to CBZ on scan** (under Archive writeback, requires the
  master writeback toggle), makes the scanner convert each `.cbr` into a sibling
  `.cbz` in place and ingest it. Real RAR archives are decompressed and repacked
  (the original is kept as `.cbr.bak`); the conversion reuses the same audited,
  atomic archive-rewrite machinery as the page editor.
- The converter **sniffs the real container by magic bytes** rather than
  trusting the extension — a large share of `.cbr` files in the wild are
  actually ZIPs that were renamed. Those are moved into place byte-for-byte
  (an instant rename, no decompression); only genuine RAR archives take the
  decompress-and-repack path. A file that is neither is left skipped with the
  health issue.

## [0.7.15] - 2026-05-30

### Fixed

- **Navigations no longer spin forever.** The server-side API fetches that RSC
  pages depend on had no timeout, so a single hung or slow backend request
  stalled the whole render — leaving client navigations (notably exiting the
  reader and applying an archive edit, both of which land on the issue page)
  stuck on a loader until a force-quit. Server fetches now time out at 10s and
  fail into the route's error boundary, and a client-side watchdog hard-reloads
  a route whose loading state outlives ~15s — covering proxy/stream stalls the
  fetch timeout can't catch. This is the deeper layer beneath the v0.7.10
  service-worker fix.
- **The archive editor no longer shows a phantom trailing page.** It built its
  tiles from the database's `issue.page_count`, which can drift from the actual
  archive (a stale scan, or a ComicInfo `<PageCount>`), producing a blank extra
  page whose deletion errored with "ordinal out of range." The editor now reads
  the archive's real page count live (new `GET /issues/{id}/archive/page-count`)
  and builds from that, so it always matches the file.

## [0.7.14] - 2026-05-29

### Fixed

- **Home no longer inherits the previous view's scroll position.** Home, the
  library grid (`?library=…`), and search (`?q=…`) all share the `/` route, and
  the App Router only resets scroll on a pathname change — so opening Home from
  the grid or search (same path, scrolled down) left it scrolled mid-page. Home
  now resets to the top when it loads from those views. Filtering within the
  grid still preserves scroll, and other pages are unaffected.

## [0.7.13] - 2026-05-29

### Changed

- **Reverted the compact single-row mobile list headers** (Bookmarks, All
  Libraries, CBL list) introduced in v0.7.7. Those headers now stack their
  control rows again as they did before, on mobile and desktop. This also
  removes the `PageHeader` `descriptionClassName` prop and the Libraries
  toolbar's `⋯` overflow that folded Save-as-view / Clear-filters.

## [0.7.12] - 2026-05-29

### Added

- **Bulk archive editing.** The multi-select toolbar on the series, collection,
  and reading-list views gains an admin-only **Edit archives…** action that
  applies one operation across the whole selection — rotate cover, rotate every
  page, or remove the first/last N pages. Each op is _relative_, lowered per
  issue once its page count is known (so "remove the last page" does the right
  thing on every archive, and removal never empties a file). The server skips
  issues whose library has writeback disabled or whose format isn't editable and
  reports them back, so nothing is silently dropped.
- **Admin Queue page** (`/admin/queue`): a live pending-job depth overview
  across all queues (now including archive edits) plus an **Archive operations**
  tab listing recent edits from the audit trail with per-row drill-down.
- **Archive backups storage card** on the library health page — total size,
  file count, and oldest/newest of the `.bak` safety backups the editor keeps,
  so operators can gauge disk use.

### Fixed

- **Highlight thumbnails no longer squish on non-2:3 pages.** A saved highlight
  on a double-page spread (or any page that isn't ~2:3) rendered horizontally
  compressed in the markers grid, because the tile assumed every page is 2:3.
  New markers now stamp the page's natural dimensions at capture time and the
  grid renders them at their true aspect — with no layout reflow. (Markers saved
  before this update keep the old approximation until re-created.)

### Changed

- **Covers now open in an in-app lightbox instead of a new browser tab.** A
  cover tile in the issue's Covers tab was a `target="_blank"` link to the
  raw image bytes — fine in a browser, but an installed PWA has no new tab to
  open, so it navigated the app itself onto the chromeless image endpoint with
  no way back. Tiles now open a full-resolution viewer inside the app: page
  between covers (arrows or ←/→), tap the backdrop or press Esc to close back
  to the gallery. Controls are inset from the device safe areas so they clear
  the iOS status bar and home indicator.

## [0.7.10] - 2026-05-29

### Fixed

- **Exiting the reader no longer hangs on a spinner.** The exit arrow does a
  client-side navigation to the issue page; that request shares a path prefix
  (`/series/…`) with the API hard-guard in the service worker, which re-fetched
  it via `respondWith(fetch(request))` — forwarding the request's abort signal.
  When the App Router superseded the in-flight RSC fetch (intermittently, under
  the reader's decode load), the forwarded signal aborted the worker's fetch and
  the router stranded on the route's loading state until a hard reload. The
  worker now hands these requests to the browser's native loader (matching the
  cross-origin branch), signal intact — no re-fetch, no stranded navigation.
  Existing PWA clients pick up the fix once the new service worker activates
  (close all tabs, or accept the update prompt).
- **iOS PWA: the status bar no longer overlaps the comic art.** With the
  translucent status bar the reader painted full-screen, so the clock / battery /
  home indicator sat on top of the page. The reader now insets its image by the
  device safe areas, so the status bar and home indicator land on the black
  letterbox instead of the art. (iOS can't hide the status bar from a PWA; this
  keeps it clear of the page. No-op off-iOS, where the insets are zero.)

## [0.7.9] - 2026-05-29

### Fixed

- **Covers no longer flash white and paint in top-to-bottom as a page loads.**
  Library/series/issue pages render covers client-side, and the `Cover`
  component had no placeholder or fade — each cover painted onto the page as
  it loaded, cascading down the grid. Covers now sit on a stable dark tile
  and fade in once decoded (cached covers paint instantly, no fade), matching
  the reader's page-image behavior.
- **Library grid loading skeleton** is now a neutral cover-card grid instead
  of a rails shape, so it no longer mismatches the `?library=` grid view
  while loading.

## [0.7.8] - 2026-05-29

### Changed

- **Seamless reader page turning.** Page prefetch now decodes and retains the
  upcoming/previous pages (`img.decode()` + retained element) instead of only
  warming the byte cache, so the next/prev `<img>` mounts already-decoded and
  the flip is instant — no re-decode, no entrance fade. Prefetch now covers
  both directions (3 ahead / 2 behind), dedupes, caps concurrency, and works
  in webtoon mode; the visible page loads at `fetchPriority="high"`.
- **Smoother page map.** Strip thumbnails are pre-warmed around the current
  page when the reader opens (filling the cache and kicking server-side
  generation early) and load eagerly within the visible window, so the strip
  no longer flashes blank placeholders as it slides up.
- **Snappier page transitions.** Slide trimmed 280→210ms, fade 220→160ms.

## [0.7.7] - 2026-05-29

### Changed

- **More compact list headers on mobile.** The Bookmarks, All Libraries, and
  CBL-list headers stacked many full-width control rows, pushing content far
  down on phones. Now: search grows to fill one row with the density/view
  toggle (Bookmarks) or trailing controls (Libraries) beside it; the Libraries
  toolbar's secondary actions (Save as view, Clear filters) fold into a `⋯`
  overflow; the Bookmarks reference blurb is hidden on small screens; and the
  CBL search grows on mobile. (CBL's stats-pills/controls restructure is a
  follow-up.)

## [0.7.6] - 2026-05-29

### Fixed

- **Metadata apply now refreshes open tabs without a page reload.** Applying
  is async; the match dialog only re-hydrated on the writeback path (waiting
  for the rescan's `scan.completed`). A DB-direct (non-writeback) apply had no
  completion signal, so an already-open **Covers** or **Notes** tab stayed
  stale until a manual refresh. The apply job now broadcasts a
  `metadata.applied` event the dialog waits on, so both paths re-hydrate.
- **Action-menu "Thumbnails" item no longer highlights differently** from its
  siblings. The dropdown sub-trigger now flips text to `accent-foreground`
  (and animates) on hover/focus/open like a regular menu item, instead of
  showing the accent background with default-colour text.
- **Dropdown menus now scroll instead of overflowing the screen.** A long
  action menu opened mid-page on mobile ran items off-screen (up or down)
  with no way to reach them. Menu (and submenu) content is now capped to the
  available viewport height and scrolls.

## [0.7.5] - 2026-05-29

### Fixed

- **`GET /libraries/{id}` 404'd when called with a UUID.** The endpoint
  resolved only by slug, but the fetch-metadata dialog holds the issue's
  `library_id` UUID — so the lookup missed, the library never loaded, and
  `metadata_writeback_enabled` read as false. That silently broke the
  apply→wait-for-rescan flow (the dialog closed onto a stale issue page).
  The read endpoint now accepts a slug **or** a UUID.

## [0.7.4] - 2026-05-29

### Fixed

- **Candidate cover images failed to load in the fetch-metadata view.** The
  service worker's cross-origin guard was a no-op (serwist's `defaultCache`
  registered a second fetch listener that still intercepted provider covers);
  the resulting opaque cross-origin response is incompatible with the
  document's `COEP: credentialless`, so the browser blocked the images
  (`NS_ERROR_INTERCEPTION_FAILED`). The SW now hands cross-origin requests to
  the browser's native loader. Existing clients pick up the fix on the next
  service-worker update (hard refresh / close all tabs).

## [0.7.3] - 2026-05-29

### Added

- **"Re-download missing variant covers" button** in the admin Metadata
  dashboard. Triggers the variant-cover backfill (previously API-only) to
  recover provider covers whose local file is missing, looping in batches
  and reporting any that can't be refetched (stale provider URL).

### Changed

- **Error and 404 pages rebuilt** to be theme-aware and on-brand, replacing
  the legacy top-bar shell. A shared `StatusScreen`/`StatusCard` now backs the
  404, the per-area error boundaries, a new root-level not-found, and a new
  `global-error` boundary that catches root-layout crashes.

### Fixed

- **Page title wrapped despite available space.** The page header now extends
  on one line (ellipsizing only when genuinely out of room), matching the
  reading-list header instead of breaking onto two lines.
- **Renaming a page left a dead sidebar link.** The left nav is rendered in the
  server layout, which soft navigation preserved — so its link kept pointing at
  the old slug and 404'd. Renames now refresh the layout so the link updates.

## [0.7.2] - 2026-05-29

### Added

- **Page-editor image adjustments.** The archive page editor can now apply
  non-destructive image transforms per page — brightness/contrast, levels
  clip, sharpen (unsharp mask), despeckle (median filter), and crop — with a
  live canvas preview and a draggable crop box. Transforms are applied at
  archive-rewrite time across CBZ/CBT/CBR, after rotation and before
  re-encode; pages needing no encode still stream-copy verbatim. Frontend and
  backend share an identical transform chain for preview/output parity.
- **Loading-skeleton framework, rebuilt per surface.** Each area now renders a
  shape-matched skeleton inside its real shell instead of one generic cover
  grid in the legacy auth shell: home rails, series detail (hero + stats +
  tabs + issue grid), bookmarks, collections, admin (header + tabs/table),
  and settings (form cards). The top-level fallback is now shell-agnostic.

### Fixed

- **Reader loading flash on iPad.** The reader inherited the library's
  light/cover-grid loading fallback, flashing white before the dark reader
  painted. It now has its own dark, reader-shaped skeleton driven by a shared
  `--reader-bg` token, so the background can't drift between skeleton and
  reader. The reader's server-side prefetches (`/progress`, `/auth/me`) now
  run concurrently, shortening time-to-reader.
- **Variant covers wiped by the nightly orphan sweep.** Downloaded provider
  covers live under `thumbs/issues/…`; the thumbnail orphan sweep read
  `issues` as an issue id and `remove_dir_all`'d the whole tree every night,
  leaving "cover unavailable" 404s and gray gallery boxes. The sweep now skips
  the reserved tree and reclaims only covers of genuinely inactive issues; the
  variant-cover backfill re-downloads rows whose file went missing.
- **Page rename navigated to a 404.** Renaming a custom page reallocates its
  slug, but the post-rename refresh re-rendered the stale `/pages/<old-slug>`
  URL and hit `notFound()`. The rename now navigates to the new slug when on
  the page's detail route. Long page titles also wrap instead of truncating.

### Removed

- Dropped the vestigial `metadata_run_candidate.dismissed_at` column.

[Unreleased]: https://github.com/mbryantms/folio/compare/v0.9.5...HEAD
[0.9.5]: https://github.com/mbryantms/folio/compare/v0.9.4...v0.9.5
[0.9.4]: https://github.com/mbryantms/folio/compare/v0.9.3...v0.9.4
[0.9.3]: https://github.com/mbryantms/folio/compare/v0.9.2...v0.9.3
[0.9.2]: https://github.com/mbryantms/folio/compare/v0.9.1...v0.9.2
[0.9.1]: https://github.com/mbryantms/folio/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/mbryantms/folio/compare/v0.8.1...v0.9.0
[0.8.1]: https://github.com/mbryantms/folio/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/mbryantms/folio/compare/v0.7.23...v0.8.0
[0.7.21]: https://github.com/mbryantms/folio/compare/v0.7.20...v0.7.21
[0.7.20]: https://github.com/mbryantms/folio/compare/v0.7.19...v0.7.20
[0.7.19]: https://github.com/mbryantms/folio/compare/v0.7.18...v0.7.19
[0.7.18]: https://github.com/mbryantms/folio/compare/v0.7.17...v0.7.18
[0.7.15]: https://github.com/mbryantms/folio/compare/v0.7.14...v0.7.15
[0.7.14]: https://github.com/mbryantms/folio/compare/v0.7.13...v0.7.14
[0.7.13]: https://github.com/mbryantms/folio/compare/v0.7.12...v0.7.13
[0.7.12]: https://github.com/mbryantms/folio/compare/v0.7.11...v0.7.12
[0.7.11]: https://github.com/mbryantms/folio/compare/v0.7.10...v0.7.11
[0.7.10]: https://github.com/mbryantms/folio/compare/v0.7.9...v0.7.10
[0.7.9]: https://github.com/mbryantms/folio/compare/v0.7.8...v0.7.9
[0.7.8]: https://github.com/mbryantms/folio/compare/v0.7.7...v0.7.8
[0.7.7]: https://github.com/mbryantms/folio/compare/v0.7.6...v0.7.7
[0.7.6]: https://github.com/mbryantms/folio/compare/v0.7.5...v0.7.6
[0.7.5]: https://github.com/mbryantms/folio/compare/v0.7.4...v0.7.5
[0.7.4]: https://github.com/mbryantms/folio/compare/v0.7.3...v0.7.4
[0.7.3]: https://github.com/mbryantms/folio/compare/v0.7.2...v0.7.3
[0.7.2]: https://github.com/mbryantms/folio/compare/v0.7.1...v0.7.2
