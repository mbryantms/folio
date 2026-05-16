# Phase Status

Snapshot of what has actually shipped vs what's deferred. Source of truth for
what to expect when running locally; read alongside [comic-reader-spec.md §19](../../comic-reader-spec.md).

Updated 2026-05-16 (rust-public-origin v0.2 cutover).

---

## Phase 0 — Foundation ✅

Shipped:
- Cargo workspace (6 crates) + pnpm workspace (`web/`)
- Postgres (extensions: `pg_trgm`, `unaccent`, `fuzzystrmatch`, `pgcrypto`)
- SeaORM + migrations bootstrap (auto-apply on startup)
- Auth: cookie session + CSRF + Bearer; argon2id local users; OIDC code+PKCE; first-user admin bootstrap; refresh-token rotation with reuse detection; **15 tests green**
- Security middleware: full CSP, COOP/COEP/CORP, HSTS, Permissions-Policy, X-Content-Type-Options, X-Frame-Options
- `/healthz`, `/readyz`, `/metrics` (Prometheus skeleton), structured JSON logs
- Compose stacks: `compose.dev.yml` (postgres + redis + dex) and `compose.prod.yml`
- Distroless multi-stage Dockerfile (Rust + Next standalone)
- CI workflows: build, test, lint, audit, oasdiff, multi-arch container build
- Phase 0 deliverable docs: threat-model, auth-model, csp, rate-limits, caddy, scaling, authentik

Deferred to later in Phase 0+:
- TOTP enrollment + verification (handler scaffold present; not wired into login)
- Local self-serve recovery (returns 501; SMTP not wired)
- WebSocket ticket endpoint (file scaffold; needs Redis state)
- Rate-limiting middleware (`tower_governor` skeleton; buckets not yet enforced)
- In-process Next.js supervisor in the Dockerfile entrypoint

---

## Phase 1a — Library scan & basic browse ✅

Shipped:
- Recursive scanner with `walkdir` + `notify`-watch hooks (resumable via `scan_runs`)
- Two-phase scan model (Phase A walks/hashes/parses; Phase B = thumbnails, currently inline — see Phase 1b notes)
- CBZ reader with full §4.1.1 defenses (zip-slip, entry-count cap, single-entry cap, total-bytes cap, compression-ratio bomb defense, encrypted detection); **13 archive tests + 5 entry-name tests green**
- ComicInfo.xml parser (XXE-safe, 1 MiB cap, all canonical fields + `raw` map for forward-compat); **8 tests green**
- Filename inference (Mylar3-style); **7 tests green** (proptest-fuzzed)
- BLAKE3 content-hash dedupe (default `dedupe_by_content=true`)
- Encrypted CBZ detection (state = `encrypted`)
- Series + Issue schema with all ComicInfo fields, `library_user_access` ACL table, ComicVine/Metron ID columns reserved
- API endpoints: `GET /libraries`, `POST /libraries`, `POST /libraries/{id}/scan`, `GET /series`, `GET /series/{id}`, `GET /series/{id}/issues`, `GET /issues/{id}`
- Web pages: library grid, series detail, issue detail (full ComicInfo metadata + page list)

---

## Phase 1b — Format coverage, async thumbnails, search foundation ⚠️ partial

Shipped:
- **Parsers** — `series.json` (Mylar3 schema, 256 KiB cap) + MetronInfo.xml (XXE-safe, 1 MiB cap, role-tagged credits + ID maps); **8 new tests = 23 parser tests total**
- **Search** — `tsvector` STORED generated columns on `series` + `issues` with A/B/C/D weighting; GIN indexes; pg_trgm trigram index for autocomplete; `?q=` on `/series` and `/series/{id}/issues` using `websearch_to_tsquery` + `ts_rank_cd`; trigram fallback for short/typo'd queries; query length capped at 200
- **Thumbnail pipeline** — `image` crate (PNG/JPEG/WebP/GIF) → Lanczos3 resize to ≤ 600 px wide → WebP encode → atomic write to `/data/thumbs/<issue-id>.webp` (content-addressed)
- **Endpoint** — `GET /issues/{id}/pages/0/thumb` (covers only; lazy-generates if missing; ETag + immutable cache; ACL-checked)
- **Web** — `<Cover>` component, `<LibrarySearch>` + per-series issue search forms; `cover_url` surfaced on `SeriesView` and `IssueSummaryView`
- Scanner generates covers synchronously after each active issue
- `scan_runs.stats` adds `thumbs_generated` / `thumbs_failed` counters

Deferred to a follow-up session (Phase 1b stretch / bridge to Phase 2):
- **Format coverage**: CBR (`unrar` subprocess + `prlimit`), CB7 (`sevenz-rust`), CBT (stdlib `tar`), folder-of-images, EPUB comic-style predicate (§4.5)
- **MetronInfo ↔ scanner merge**: parser exists and tests pass, but the scanner's per-issue field merge currently only honors ComicInfo and series.json; MetronInfo isn't yet preferred over ComicInfo for fields it provides
- **apalis worker**: thumbnails run synchronously inline; moving to a Redis-backed background queue would unblock Phase A's >1000 issues/min target
- **JXL → AVIF transcode** (no JXL fixtures yet; defer until needed)
- **Adversarial-fixture suite for non-CBZ formats** — landed for CBZ only

---

## Phase 2 — Reader v1 + basic OPDS ✅

Shipped:
- **Page byte streaming** — `GET /issues/{id}/pages/{n}` with `Range`/`If-Range`/206/416/`Accept-Ranges` semantics, ETag, 16-byte magic-number sniff with allowlist (jpeg/png/webp/avif/gif/jxl), SVG rejection, `Cache-Control: private, max-age=3600` (§17.5). **6 integration tests** covering 200/206/416/415/If-Range round-trip plus **11 unit tests** for the parser and sniffer
- **ZIP central-directory LRU** — `COMIC_ZIP_LRU_CAPACITY` (default 64); thumbnails + page bytes acquire from cache; eviction drops the `Cbz` (Drop closes FD); Prometheus metrics `comic_zip_lru_open_fds` (gauge), `_hits_total`, `_misses_total`, `_evictions_total` exposed at `/metrics`
- **`/metrics` Prometheus endpoint** — `metrics-exporter-prometheus` recorder installed at startup; `PrometheusHandle` lives in `AppState`
- **`Cbz::read_entry_range`** — bounded `[start, len)` reads against any entry, decompress-from-start for both STORED and DEFLATED (with `debug` log on DEFLATED Range hits per spec §B7); 2 new archive tests
- **Reader UI** — `/read/[id]` RSC shell + client island. Single-page mode, LTR, keyboard (←/→/Space/Esc/m/f), tap zones (left third = prev, right third = next, middle = toggle chrome), fit modes (width/height/original), N+1/N+2 image-element prefetch, `aria-live="polite"` page-N-of-M announcements (§7.4)
- **Reader-local state** — Zustand store with per-series `fitMode` persistence in `localStorage`
- **Decode worker** — `web/workers/decode.ts` uses `createImageBitmap` (not WebCodecs `ImageDecoder`) per §7.2
- **Progress endpoint** — `POST /progress` upsert + `GET /progress?since=…` delta sync, ACL-checked, `X-Progress-Api: 1` forward-compat header per §9.7
- **Basic OPDS 1.2** — `/opds/v1` root, `/opds/v1/series` (paginated), `/opds/v1/series/{id}`, `/opds/v1/recent`, `/opds/v1/search?q=…`, `/opds/v1/issues/{id}/file` direct download. JWT only; ACL-filtered. Atom XML emission with explicit XML escaping
- **Reader bundle budget gate** — `web/scripts/check-bundle-size.mjs` parses `next build` output and asserts `/[locale]/read/[id]` First Load JS ≤ 150 KB gzip; also greps reader sources for forbidden imports (`framer-motion`, `@tiptap/*`, `@dnd-kit/*`). Wired into `web-check` CI job. **Current: 108 KB / 150 KB**
- **axe-core a11y baseline** — Playwright config + sign-in-page WCAG 2.2 AA test; CI runs it after `next start`
- **k6 soak script** — `tests/soak/k6-reader.js`: 10 VUs page through one issue for 1 hour, asserts no 5xx + p99 < 2 s
- **Postgres test image pinned** — testcontainers-modules defaults to `postgres:11-alpine` which predates `STORED` generated columns; `with_tag("17-alpine")` matches the prod compose stack

Tests: **35 server tests** (18 unit + 8 auth + 6 page_bytes + 3 security_headers) + **15 archive tests** + parsers green.

Deferred to follow-ups:
- **Per-page thumbnails (`/pages/{n}/thumb` for `n > 0`)** — only needed by Phase 3's mini-map / page-strip overlay; current 501 placeholder documents the Phase 3 TODO
- **JXL → AVIF transcode pipeline** — no JXL fixtures committed; spec carve-out
- **Authenticated axe-core walk** — library → series → issue → reader requires a seeded fixture harness; current baseline covers the public sign-in surface only
- **OPDS app passwords + PSE + signed URLs** — Phase 6
- ~~**Automerge sync for progress** — Phase 4; the API contract is stable so the backend swap is transparent~~ **Dropped 2026-05-15** — see spec §9 decision note. Progress stays on `progress_records` with server-side `max(last_page)` conflict resolution; no CRDT migration planned.
- **apalis worker for thumbnails** — synchronous in-scanner generation still fine at fixture scale

"Done when": ✅ open an issue, page through it, close, reopen, resume on the same page. ✅ Range request edge cases pass test. ✅ ZIP-LRU FD count visible in `/metrics`. ⏳ Soak script written but not yet run for the full hour against `compose.prod.yml` (manual gate before declaring Phase 2 closed in releases).

---

## Phase 3 — Reader v2 (modes + polish) ✅

Shipped:
- **Per-page thumbnails** — `GET /issues/{id}/pages/{n}/thumb` for any `n`.
  Cover stays at `/data/thumbs/<id>.webp`; per-page lives at `/data/thumbs/<id>/<n>.webp`.
  Lazy-generated through the existing ZIP LRU; same long-immutable cache headers + page-aware ETag
- **Typed page metadata** — `IssueDetailView::pages` is now `Vec<PageInfo>` (was `serde_json::Value`); web reader receives `double_page`, `image_width`, `image_height` per page directly
- **Reader view modes** — `single` / `double` / `webtoon`. Double-page renders pairs side-by-side (skipped when `pages[i].double_page === true`); webtoon stacks all pages vertically with native scroll
- **RTL direction** — auto from `Manga=YesAndRightToLeft`, then per-user `default_reading_direction`, then `ltr` fallback. Tap zones, arrow keys, and double-page pair ordering all flip in RTL
- **Auto-detection helpers** — `web/lib/reader/detect.ts` with `detectDirection` + `detectViewMode` (median page aspect rules); 10 vitest cases. Per-series localStorage choice always overrides
- **Mini-map / page-strip overlay** — `web/app/[locale]/read/[id]/PageStrip.tsx`; toggled with `m`. Lazy-loads thumbs, smooth-scrolls active into view (reduced-motion honored), direction-aware ordering
- **Gestures** — `@use-gesture/react`: horizontal swipe → next/prev (direction-aware, 30 px threshold), pinch → cycle fit mode. Disabled in webtoon mode (vertical scroll owns)
- **Per-user reading-direction preference** — `default_reading_direction` column on `users` (migration `m20260601_000001`); `PATCH /me/preferences` endpoint; surfaced on `/auth/me`; selectable from the user-menu dropdown
- **Zustand store extensions** — new persisted slices `viewMode`, `direction`, plus `pageStripVisible`. Storage keys follow `reader:<slice>:<series_id>`
- **Reader keyboard map** — added `d` (cycle view mode), reassigned `m` from chrome-toggle (Phase 2 stub) to page-strip-toggle per spec §7.4
- **Bundle budget** — 118 KB / 150 KB after `@use-gesture/react` addition (was 108 KB); CI gate still green
- **Documentation** — new `docs/dev/reader-shortcuts.md` consolidates keyboard/gesture/autodetect rules

Tests: **+6 server** (3 thumbnails + 3 preferences) + **+10 web vitest** (detect helpers).

Deferred to follow-ups:
- **Color modes (sepia / OLED-black)** — explicitly out of scope per user direction. The CSS-variable plumbing slot is documented in the phase plan if it returns
- **Phase 3.5 accessibility audit** — separate manual milestone (NVDA + VoiceOver + keyboard-only walks); needs the new modes in place and stable, which is now true
- **Authenticated axe-core walks** — still gated on the e2e fixture harness; current sign-in baseline still runs in CI
- **Custom color curves** — §7.3 mention; not in the Phase 3 line and not requested
- **Long-strip variant** — collapsed into webtoon (continuous scroll); revisit only if a real distinguishing case appears

"Done when": ✅ reader feels polished on desktop (manual pass: Berserk RTL flow, double-page on landscape spreads, webtoon scroll, page-strip jump, gestures all working). Mobile UX review is the natural follow-up alongside Phase 3.5.

---

## Library Scanner v1 — ✅

Spec: [`library-scanner-spec.md`](library-scanner-spec.md). Operator
notes: [`library-scanner.md`](library-scanner.md).

Shipped:

- **Schema** — new `library_health_issues` table; `series` + `issues` +
  `libraries` extended with the soft-delete / identity / config columns the
  spec needs (folder_path, last_scanned_at, match_key, removed_at,
  removal_confirmed_at, superseded_by, special_type, hash_algorithm,
  ignore_globs, report_missing_comicinfo, file_watch_enabled, soft_delete_days)
- **Apalis + Redis dispatch** — `POST /libraries/{id}/scan` enqueues + 202s.
  Library-scoped coalescing per §3.2 keeps "one in flight, one queued".
  `redis_url` is now required (was Optional)
- **Scanner refactor** — split into `scanner/{validate,enumerate,process,stats,mod}.rs`
  matching spec §4 phases. Per-folder mtime gate (§4.4)
- **Ignore rules** — built-in (dotfiles, __MACOSX, Thumbs.db, etc.) + user
  `globset` patterns. `PATCH /libraries/{id}` for per-library config
- **Library health** — 12 typed `IssueKind`s with fingerprint-based upsert,
  auto-resolve on next scan. `GET /libraries/{id}/health-issues`,
  `POST /libraries/{id}/health-issues/{issue_id}/dismiss`
- **Series identity** — `match_key` (sticky admin override) → `folder_path`
  (fast path) → `normalized_name + year` → create. `PATCH /series/{id}`
- **Reconciliation + soft-delete** — file missing on disk → `removed_at`;
  file returns → auto-restore. `GET /libraries/{id}/removed`,
  `POST /issues/{id}/restore`, `POST /issues/{id}/confirm-removal`. Daily
  04:00 UTC auto-confirm sweep
- **File-processing pipeline** — series.json + MetronInfo.xml wired in
  (MetronInfo wins on overlapping fields per §4.4); specials/annuals/one-shot
  detection populates `special_type`; ComicInfo `<PageCount>` is stored as
  metadata but no longer treated as a health signal
- **File-watch + scheduling** — `notify-debouncer-full` per library root
  (30 s debounce). `tokio-cron-scheduler` for `library.scan_schedule_cron`,
  daily reconcile sweep, daily scan_runs prune. `POST /series/{id}/scan` for
  per-folder rescan. Optional `COMIC_SCAN_ON_STARTUP=true`
- **WebSocket scan events** — `GET /ws/scan-events` (admin-only) emits
  `scan.started`, `scan.series_updated`, `scan.completed`, `scan.failed`
  per spec §8.1. `series_updated` throttled to ≥100 ms per library
- **Scan history** — `GET /libraries/{id}/scan-runs?limit=50`. Daily
  03:00 UTC prune keeps last 50 per library
- **Multi-format dispatch** — `archive::open(path, limits)` returns
  `Box<dyn ComicArchive>` based on extension. Full readers: `.cbz`, `.cbt`.
  Scaffolded: `.cbr`, `.cb7` (return clear "not implemented"; scanner emits
  `UnsupportedArchiveFormat` health issue)
- **Post-scan job pipeline** — apalis queues for `post_scan_thumbs`,
  `post_scan_search`, `post_scan_dictionary` registered + enqueued at scan
  end. Handlers are stubs today; the wiring is the deliverable
- **Observability** — `comic_scan_duration_seconds` (histogram),
  `comic_scan_files_total` (counter), `comic_scan_health_issues_open` (gauge)

Tests: ~75 tests passing — new `scan_dispatch` (2), `scanner_smoke` (4),
`ignore_globs` (4), `health_issues` (3), `identity` (3), `reconcile` (4),
`processing` (4) integration files; new `health` (3) + `ignore` (5) unit tests;
`archive::multi_format` (4). Existing 26 unit + auth (8), page_bytes (6),
preferences (3), security_headers (3), thumbnails (3) suites all green.

Cross-cutting tech debt entries (carry-over for follow-up plan):

- **Full CBR + CB7 readers** — extension recognized + dispatch wired; readers
  return "not implemented" today. Add real impls using `unrar` + `sevenz-rust`
  (deps already present)
- **Volume year-vs-sequence column split** (§6.4) — today the raw `volume`
  is stored as-is; needs mini-migration + parser branch
- **Hash-mismatch supersession** (§6.2) — modified-in-place files update the
  existing row rather than minting a new one with `superseded_by`
- **Dedupe-by-content `issue_paths` alias table** (§6, §10.1
  DuplicateContent) — needs new table + per-library dedupe-mode handler
- **LocalizedSeries + mixed-series merging** (§7.1.2, §7.2)
- **Mount-type sentinel detection** for the file-watcher (§3.1)
- **Live-reload of cron / library config** without server restart
- **WS per-user library-access filtering** — currently admin-only blanket
- **Web admin UI** — health-issues tab, scan history table, removed-items
  list, library settings form (ignore_globs, soft_delete_days, file_watch
  toggle, scan_schedule_cron); endpoints exist, UI doesn't
- **Page-byte streaming for non-CBZ formats** — reader still requires `.cbz`
- **Inline thumbnail generation** still happens in `process::ingest_one`;
  the `post_scan_thumbs` queue is wired but its handler is a stub. Move the
  generation off the scan loop in a follow-up

---

## Multi-page rails 1.0 ✅

Plan: `~/.claude/plans/multi-page-rails-1.0.md`. Shipped 2026-05-15.

Generalized the single hardcoded home page into a user-curated **page**
entity: each page carries its own pinned saved-view rails and an entry
in the sidebar. M1 → M7.

- **`user_page` table** — `(id, user_id, name, slug, is_system,
  position, description, timestamps)`. Per-user unique slug; partial
  unique index ensures exactly one `is_system = true` Home row per
  user. Custom pages capped at 20/user.
- **`user_view_pin` PK widened** to `(user_id, page_id, view_id)`.
  Migration back-fills existing pin rows onto each user's auto-created
  Home page so today's home rails keep rendering.
- **Server CRUD** at [crates/server/src/api/pages.rs](../../crates/server/src/api/pages.rs):
  list / create (`POST /me/pages`) / patch (rename + description) /
  delete / reorder + per-page sidebar-visibility toggle. The bare `/`
  route resolves to the system Home; custom pages render at
  `/pages/[slug]` (server-side slug→id lookup, `notFound()` on miss,
  redirect `/pages/home` → `/`).
- **Page-aware pin endpoints** in `saved_views.rs` — `pin` accepts
  `{page_ids: [Uuid]}` (multi-pin), `unpin`/`reorder` accept `{page_id}`,
  `GET /me/saved-views?pinned_on=<page>` filters per page.
  `SavedViewView.pinned_on_pages: Vec<String>` exposes ground truth
  for the multi-pin picker. Legacy no-body POSTs continue to target
  the system Home page as a transitional shim.
- **Sidebar layout** ([api::sidebar_layout](../../crates/server/src/api/sidebar_layout.rs))
  gained `kind='page'` for custom pages plus `kind='header'` /
  `kind='spacer'` for user-curated section dividers. The unified
  `compute_layout` emits explicit default headers (`default:browse`,
  `default:libraries`, `default:pages` when any custom pages exist) so
  the client renders the response as a flat ordered list — no more
  client-side group inference. `label` overrides on any row (e.g.
  rename "Bookmarks" → "Pins") are honored.
- **Web settings split** under `/settings/`:
  - **`views`** — Saved views CRUD; per-row "Pin to pages…" picker.
  - **`pages`** — tabbed page-list. Each tab shows the page's rails with
    drag-reorder + add/remove via picker, plus inline rename, edit
    description, sidebar-visibility toggle, and delete.
  - **`navigation`** — unified ordered sidebar list with drag-reorder,
    custom headers + spacers (with rename/remove), label overrides,
    and an Add picker (search input + sticky kind-grouped sections:
    Built-ins / Pages / Libraries / Filter views / CBL lists /
    Collections / Headers / Spacers).
- **Coverage** — 16 pages integration tests, 19 saved_views tests
  (multi-page pin, per-page cap, scoped reorder, pinned_on filter,
  legacy no-body pin), 15 sidebar_layout tests (default headers,
  custom header/spacer round-trip, label override, page in layout),
  plus 264 web vitest cases (page-rails prop wiring, multi-pin picker
  state, spacer grouping, mainNav header sections).

Deferred to follow-ups:

- Shared / public pages.
- Server-bundled default pages beyond Home.
- Page-level layout modes (no grid view; only rail mode).
- Cross-page pin sync ("always pin to all my pages").
- Page templates / cloning.

---

## v0.2 — Architectural normalization: Rust binary becomes the public origin ✅

Cross-cutting shipment 2026-05-16, threaded between the regular phase
work. Plan at `~/.claude/plans/rust-public-origin-1.0.md`. Triggered
by three patch releases (v0.1.15–v0.1.17) that each fixed a different
external-client path the Next.js front-end wasn't forwarding — a clear
sign the topology had drifted from the design intent stated in
[`web/next.config.ts`](../../web/next.config.ts).

The Rust binary now owns the public origin in prod. It handles its own
routes (`/api/*`, `/opds/*`, `/auth/*`, page bytes, `/ws/*`) directly
and reverse-proxies HTML / RSC / `/_next/*` to Next.js as an internal
upstream over the compose bridge. Single Ingress / single reverse-proxy
upstream — operators no longer need path-based routing rules. Concrete
pieces:

- New `crates/server/src/upstream/` module: streaming HTTP/1.1
  reverse proxy + raw byte-level WebSocket-upgrade passthrough;
  XFF chain preservation; standard error-envelope on upstream
  failures. Test coverage: 6 unit, 15 lightweight wiremock, and
  5 end-to-end (`TestApp` + `axum::serve`) tests, including a
  WS byte-bridge round-trip.
- `Router::fallback(upstream::proxy)` in [`crates/server/src/app.rs`](../../crates/server/src/app.rs)
  catches anything no explicit route claimed.
- `compose.prod.yml`: `app` is the only host-published service; `web`
  switched from `ports:` to `expose:` (internal-only). New env
  `COMIC_WEB_UPSTREAM_URL` wires the proxy.
- Install templates (`caddy.md`, `nginx.md`, `traefik.md`,
  `kubernetes.md`, `lan-https-mkcert.md`) collapsed from path-routed
  configs to single-upstream — see the `## Breaking changes by
  version` section in [upgrades.md](../install/upgrades.md) for the
  operator migration.
- The v0.1.15–v0.1.17 Next-side rewrite/matcher-exclusion patches
  for `/opds/*`, `/auth/oidc/*`, `/issues/*` removed.
- Web app drops the `/api/` fetch prefix entirely (one central change
  in `web/lib/api/auth-refresh.ts`; ~17 inline-fetch + form-action +
  download-href sites updated to match). Cuts the previous web → API
  ping-pong (browser → Rust → Next rewrite → Rust) down to one hop.
- Dev workflow: browser entry flipped from `localhost:3000` to
  `localhost:8080`. Next HMR rides the same proxy WebSocket path the
  byte-bridge tests already cover.

Sidelined: a Rust-side `/api/` strip middleware was prototyped and
deferred. `axum::Router::layer` applies layers per-matched-route +
fallback rather than before route matching, so the strip would have
needed bind-site `tower::ServiceBuilder` wiring plus a refactor of
every test that uses `app.router.clone().oneshot(...)`. Not load-
bearing once the web app drops the prefix; the option remains open if
a future client needs the alias.

Security-audit follow-up: M-1 (no explicit `CorsLayer`) downgrades
from Medium to Low — with HTML + API now on a single origin, the
attack surface a CORS policy would gate is gone. Annotation added
inline in [`docs/dev/security-audit.md`](./security-audit.md).

---

## Phases 3.5+ — outline only

| Phase | Focus | Status |
|-------|-------|--------|
| 3.5   | Accessibility audit milestone | not started |
| 4     | ~~Sync (Automerge)~~ — **considered, not chosen** (2026-05-15) | dropped |
| 5     | Lists, arcs, reviews + library access UI | not started |
| 6     | Search v2 + OPDS 2.0 + v1.0 GA | not started |
| 7+    | Relationships, iOS, Android, polish | not started |

---

## Cross-cutting tech debt to revisit before v1.0

- **Slug URLs** — backlogged in §21; series/issue URLs currently expose raw UUID/BLAKE3
- **Dex config recovery on `dev-services-reset`** — `.dev-data/dex-config.yaml` is the only file in `.dev-data` that's committed; `--exclude` from `rm -rf` or `cp` from a fixture each time would be nicer
- **OpenAPI codegen for web** — `web/lib/api/types.ts` is hand-written; `just openapi` should generate it from the Rust source via `openapi-typescript`. Phase 2 added `progress_records` types + reader paths; the gap widens
- **Sea-orm `unused import: self`** warning in `library/scanner.rs` — cosmetic
- **OTel** — currently only a startup-log indicator; actual `tracing-opentelemetry` wiring is deferred
- **Email Verification + Reset for local users** — handlers return 501; need SMTP integration via apalis when SMTP envs are set
- **Thumbnail addressing scheme** — Phase 3 settled the per-page path scheme (`<id>/<n>.webp` per-issue subdir). Cross-issue dedupe via source-bytes hashing (spec §17.5) remains a deliberate deviation: ACL is enforced at the endpoint, so URL guessability is moot. Revisit only if the page-byte deduplication becomes a measured concern
- **Root not-found.tsx removed** — Next 15 production builds reject a root `app/not-found.tsx` without a companion `app/layout.tsx`. The file was dropped because middleware redirects all unknown paths through `/[locale]`, so `[locale]/not-found.tsx` covers practical 404s. If a route ever needs to escape the locale segment this needs to come back as part of a root layout
- **`experimental.serverActions: false`** — `next.config.ts` uses `false as never` to satisfy the type but Next 15 emits a config warning. Migrate to the proper schema (`{ allowedOrigins: [] }` or unset) once Next exposes a typed-off path
