# Folio (comic reader) — Claude Code project notes

Self-hostable comic reader. Rust workspace (axum + sea-orm + apalis) backing a
Next.js 16 web app. Spec: [`docs/dev/comic-reader-spec.md`](docs/dev/comic-reader-spec.md).

## Stack pins

- **Rust**: edition 2024, axum, sea-orm, apalis (Redis-backed jobs), utoipa for
  OpenAPI emission. Postgres 17 in dev (older versions break the search migration).
- **Web**: Next.js 16, React 19.2, Tailwind v4, TanStack Query v5, shadcn/ui
  (copy-in style under `web/components/ui/`).
- **Dev services**: Postgres 5432, Redis 6380, Dex 5556 — `just dev-services-up`.

## Dev workflow

```sh
just dev-services-up   # start postgres + redis + dex
just dev               # cargo run server + pnpm dev web in parallel
just openapi           # regenerate web/lib/api/openapi.json from utoipa
just test              # cargo test --workspace + pnpm test
```

**Browser entry point: `http://localhost:8080`** (the Rust binary).
The Rust server reverse-proxies HTML / `/_next/*` / static-asset
requests to the Next dev server at `http://localhost:3000` via the
upstream-fallback wired in M2 of the rust-public-origin plan. Hitting
`:3000` directly works for raw Next dev (useful when debugging the
client bundle) but bypasses the Rust middleware stack — auth, CSRF,
security headers — so it's not what `just dev` is set up for.

Web dev defaults to **webpack** (`next dev --webpack`), not turbopack. Turbopack
has an open dev-server leak — `RangeError: Map maximum size exceeded` from
`hot-reloader-turbopack.js` after extended sessions — that kills the dev process.
`pnpm dev:turbo` is available for the faster HMR if you don't mind restarting.

Default admin (first registered user becomes admin):
`first@example.com` / `correctly-horse-battery-staple`.

## Tooling pipeline

- **Rust**: `cargo fmt` (config in [rustfmt.toml](rustfmt.toml)) and
  `cargo clippy --workspace --all-targets -- -D warnings` (config in
  [clippy.toml](clippy.toml) + `[workspace.lints]` in [Cargo.toml](Cargo.toml)).
  All crates inherit lints via `[lints] workspace = true`.
- **Web**: `pnpm --filter web run lint` (ESLint flat config at
  [web/eslint.config.mjs](web/eslint.config.mjs)),
  `pnpm --filter web run format` (Prettier with
  [web/.prettierrc.json](web/.prettierrc.json) + `prettier-plugin-tailwindcss`),
  `pnpm --filter web run typecheck` (`tsc --noEmit`, strict mode).
- **Editor**: [.editorconfig](.editorconfig) enforces LF / no-trailing-WS / 2-space
  default (4 for Rust). VSCode workspace config in [.vscode/](.vscode/) wires
  format-on-save, ESLint code-actions, debug profiles for the server + integration
  tests, and a `check all` build task that fans out to all the above in parallel.

## MCP servers (registered in user-level Claude config)

- **context7** — pull current library/framework docs. Use whenever the task
  involves an external API surface (Next.js, React, sea-orm, axum, Tailwind,
  TanStack Query, etc.) before relying on training data.
- **playwright** — headless Chromium via `@playwright/mcp`. Use to drive the web
  app for UI verification, capture console/network during a flow, and produce
  screenshots. Snapshots dump to `.playwright-mcp/` (gitignored).
- **postgres** — read/write SQL against the dev database. Use for schema
  inspection, ad-hoc queries to verify migration outcomes, and EXPLAIN/index
  analysis. Connection string is the COMIC_DATABASE_URL env var.
- **Outline** — self-hosted Outline at `outline.example.com`. Use only when the
  user references docs/wiki content there.
- **Gmail / Google Calendar / Google Drive** — registered but unauthenticated;
  ignore unless the user explicitly asks.

## Tests

- Server integration tests share **one Postgres AND one Redis per process**
  (a migrated `comic_template` cloned per test via `CREATE DATABASE … TEMPLATE`,
  dropped in `TestApp::Drop`; Redis isolates each test onto its own **logical
  DB** — `redis://…/<n>` — `FLUSHDB`-ed on acquire). Run a single file with
  `cargo test -p server --test <name>` to iterate — plain `cargo test` boots a
  per-process Postgres + Redis testcontainer (no external services needed).
  - **Fast path: `just test-rust-fast`** runs the whole suite via
    `cargo-nextest` against throwaway external Postgres + Redis
    (`COMIC_TEST_PG_URL` / `COMIC_REDIS_URL`), scheduling every binary's tests
    in one global pool. CI uses the same (nextest + the Postgres & Redis service
    containers); see
    [`crates/server/tests/common/mod.rs`](crates/server/tests/common/mod.rs)
    and [`.config/nextest.toml`](.config/nextest.toml).
  - **Redis logical-DB isolation (CI-speed Phase 3):** under nextest
    (process-per-test on a shared external Redis) each test's DB index is
    `NEXTEST_TEST_GLOBAL_SLOT`, so `[profile.ci] test-threads` must stay `≤` the
    Redis `databases` count (16 on the service container → 12). Plain
    `cargo test` (one process per binary) uses an in-process index pool instead.
  - The harness drops each clone DB on teardown so databases/connections stay
    flat on the shared server — do not remove `TestApp::Drop` or a big test file
    will exhaust `max_connections`.
- Web tests: vitest under `web/tests/`. Tests that touch routing/auth mock
  `next/navigation` and `@/lib/api/fetch`; see
  [web/tests/admin/layout.test.tsx](web/tests/admin/layout.test.tsx).
- Playwright is configured but the harness is incomplete; treat it as opt-in.

## Conventions to preserve

- **Routing precedence (rust-public-origin v0.2)**: every request hits
  the Rust router first. The router is split into two groups in
  [`app.rs::router()`](crates/server/src/app.rs):
  - `bare` routes — what external clients hit directly without a
    prefix: `/health*`, `/auth/*`, OIDC callbacks, `/opds/*`, page
    bytes (`/issues/{id}/pages/{n}` and `…/thumb`), `/ws/*` upgrade
    endpoints, plus the `auth::local` form-action POSTs the Next
    sign-in form submits to.
  - `api` routes — every JSON endpoint the web app reaches via
    `apiFetch` (which prepends `/api/`). Mounted as
    `Router::nest("/api", api)`. The `/api/` prefix is back as of
    v0.2.1 because many JSON endpoints share path shapes with Next
    pages (`/admin/users`, `/series/{slug}`, `/audit`, …) and would
    otherwise collide with HTML routes.
  - Anything no explicit route claimed falls through
    `Router::fallback(crate::upstream::proxy)`, which streams the
    request to the Next.js SSR upstream at `cfg.web_upstream_url`
    (env: `COMIC_WEB_UPSTREAM_URL`, default `http://localhost:3000`).
    The fallback is wrapped by the same middleware stack as every
    other route — `set_context` / `security_headers` / CSRF /
    TraceLayer all run on proxied requests. WebSocket upgrades for
    fallback paths get raw byte-level passthrough; `/ws/*` is still
    owned by explicit Rust handlers.

  **Adding a new route, decide first which group it lives in.**
  External-client/browser-form/streaming-bytes surface → `bare`.
  Web-app JSON the browser fetches via `apiFetch` → `api`. If both,
  register the same `routes()` in both routers (`auth::local::routes()`
  already does this for the dual form-action + cookie-API split). Do
  not re-introduce per-prefix rewrites or matcher exclusions in
  `web/next.config.ts` / `web/proxy.ts` — those v0.1.15-17
  workarounds were retired by the migration. See
  [crates/server/src/upstream/mod.rs](crates/server/src/upstream/mod.rs)
  and the plan at `~/.claude/plans/rust-public-origin-1.0.md`.
- **Error envelope** (audit-remediation M3): every server error is
  `{"error": {"code": "...", "message": "...", "details"?: ...}}`. The
  **only** construction site is
  [`api::error(status, code, message)`](crates/server/src/api/mod.rs)
  in `crates/server/src/api/mod.rs`. Every handler returns errors
  through that helper (directly or via a per-module thin alias that
  delegates). The shape is also reachable as `shared::error::ApiError`
  for handlers that need to attach additional response headers (e.g.
  `Content-Range` on 416 responses). Do not hand-build
  `serde_json::json!({"error": {...}})` at call sites — the lint
  surface in `M10` greps for that anti-pattern. The 9-variant
  `shared::error::ApiErrorCode` enum (~40 variants as of M0) is the
  long-term replacement for the stringly-typed `code` arg; M3's
  `api::respond(status, ApiErrorCode, msg)` is the typed entrypoint
  for new code.
  - **Field-level details (1.3)**: validation (422) responses carry
    `error.details` as a `[{field, message}]` list
    (`shared::error::FieldError`) so a client form can bind each
    message to its input. The single construction site is
    [`api::respond_with_field_errors`](crates/server/src/api/mod.rs),
    which `Validated<T>` and any handler running garde manually
    (`extractors::from_garde`) route through — don't populate
    `details` by hand for validation. The human `message` stays a
    complete summary so non-form callers lose nothing; empty field
    lists leave `details` unset (wire shape identical to a plain
    error). Web side: `apiMutate` parses it onto
    `ApiMutationError.fields`, and
    [`applyServerErrors(setError, err)`](web/lib/api/form-errors.ts)
    binds them onto a react-hook-form instance (adopted by the long
    admin forms in chunk 2.8).
- **Admin guard** (audit-remediation M1 + M2): admin-only handlers take
  `RequireAdmin` (or `RequireAdmin(actor)` when the handler also needs
  the actor id for audit-logging). The extractor lives at
  [`crates/server/src/auth/extractor.rs`](crates/server/src/auth/extractor.rs)
  and returns 403 + canonical envelope before the handler body runs —
  the gate is structural, not body-level.
  - **Inline `if user.role == "admin"` is reserved for ACL widening**
    (admin gets broader visibility on a handler users can also call):
    e.g. `series::list` shows admins removed/hidden rows, `libraries::list`
    shows admins all libraries instead of just their ACL-grant set.
    These handlers take `CurrentUser` and branch on role. Confirmed
    legitimate sites (do not migrate to `RequireAdmin`): libraries.rs,
    ratings.rs, reading_sessions.rs, reading_log.rs, progress.rs,
    series.rs, issues.rs, page_bytes.rs, thumbnails.rs, issue_ocr.rs,
    opds.rs, library/access.rs.
  - **Mutating admin handlers** also call
    [`record_admin_action!`](crates/server/src/audit.rs) (or
    `crate::audit::record`) to write an `audit_log` row. The M10 CI tool
    will statically verify every `RequireAdmin` handler that returns
    success emits one; M2 added the regression guard in
    `crates/server/tests/audit_log_completeness.rs`.
- **Runtime config split**: infrastructure stays in `.env`; policy
  (SMTP, OIDC, auth mode, JWT TTLs, log level, workers) lives in the
  `app_setting` table and is editable via `/admin/{auth,email,server}`.
  DB wins over env (D1 in the plan). Secret rows are sealed with
  XChaCha20-Poly1305 under `secrets/settings-encryption.key`. See
  [docs/dev/runtime-configuration.md](docs/dev/runtime-configuration.md)
  for the slice-by-slice matrix and the "add a new setting" recipe.
- **Audit log**: every mutating admin handler emits via `crate::audit::record`.
  Action names are dotted (`admin.user.update`, `admin.user.library_access.set`).
  Append-only at the role level — never UPDATE/DELETE.
- **CSRF**: cookie-bound. POST/PATCH/DELETE require `X-CSRF-Token` header
  matching `__Host-comic_csrf` cookie. Bearer-auth requests bypass.
- **Cursor pagination**: `next_cursor` is opaque (base64). Don't expose ordering
  details to callers.
- **API types are codegen-driven** (audit-remediation M1+M1a+M1b+M1c
  complete, 2026-05-23). `web/lib/api/openapi.json` and
  `web/lib/api/types.generated.ts` are regenerated by `just openapi`
  from `#[utoipa::path]` annotations + `#[derive(ToSchema)]`. Both are
  `.gitattributes`-marked `linguist-generated`. `just openapi-check` is
  the CI drift gate; it regenerates both files into temp paths and
  fails when either disagrees with the checked-in copy.
  `web/lib/api/types.ts` is a **hybrid alias shim** (~370 lines):
  177 named types alias one-line over `components["schemas"]["X"]`;
  ~40 stay inline (frontend-only computed types, WS payloads, or
  typed enums where the Rust source still uses bare `String`). Each
  inline entry is a tracked debt; promote to alias once the Rust
  source derives `ToSchema` on the underlying enum (template:
  [`crates/server/src/auth/preferences.rs`](crates/server/src/auth/preferences.rs)).
  `web/scripts/build-types-shim.py` regenerates the partition.
  **Adding a new API type:** write the Rust `#[derive(ToSchema)]`
  struct + reference it in a `#[utoipa::path]` response/body, run
  `just openapi`, then add an alias to `types.ts` or rerun the shim
  script. Never edit the codegen files by hand; the drift gate will
  revert the change.
- **Notifications**: every mutation flows through `useApiMutation` in
  [web/lib/api/mutations.ts](web/lib/api/mutations.ts), which always
  toasts on error (unwrapping the server's `error.message`) and toasts
  on success when the hook author opts in via `successMessage`. Don't
  layer `toast.success(...)` at the call site — put the message on the
  hook. Variants: `toast.success` = completion with after-state,
  `toast.error` = failure, `toast.message` = symmetric toggle,
  `toast.info` = app-state notice (plan limits, coming soon),
  `toast.loading` = work without other progress UI. Shared strings
  (e.g. `"Want to Read isn't ready yet…"`, `"Name is required"`) live
  in [web/lib/api/toast-strings.ts](web/lib/api/toast-strings.ts).
  Form-submit no-changes paths use `disabled={!isDirty}`, not a toast.
  Auth forms (sign-in / register / forgot / reset) intentionally use
  inline `<Banner>` + `<FormMessage>` instead of toasts — see
  [docs/dev/notifications-audit.md](docs/dev/notifications-audit.md)
  §F-6. Destructive mutations get an `AlertDialog` confirm at the call
  site; marker deletes are the exception and use Undo toast actions
  via [web/lib/markers/recreate.ts](web/lib/markers/recreate.ts)
  instead.
- **List pagination**: list views must never silently truncate.
  Use `useInfiniteQuery` with an IntersectionObserver sentinel
  (template:
  [`web/app/[locale]/(library)/series/[slug]/IssuesPanel.tsx`](web/app/[locale]/(library)/series/[slug]/IssuesPanel.tsx)).
  New collection endpoints accept `cursor + limit` and return
  `next_cursor`; the response includes `total` only on the first
  page so subsequent fetches stay cheap. Filter chips drive
  **server-side** query params (e.g. `?status=ambiguous,missing`),
  not client-side `.filter()` over a finite array — the moment a
  cap exists, an in-memory filter starts lying. Surfaces with
  drag-reorder (collections) auto-walk all pages before enabling
  the DnD sensors so the reorder mutation sees the full list.

  **Reviewer heuristic — reject PRs that:**
  - Add `useQuery<...>` on a response with a `next_cursor` field
    (that's an infinite query in disguise; use `useInfiniteQuery`).
  - Pass a hardcoded high `limit:` (200+) to a list-fetching hook —
    "high enough for now" is exactly how the CBL >500 bug shipped.
    If the endpoint is genuinely unbounded, use cursor pagination;
    if it's bounded by domain (`/me/sessions`), leave it `limit:`-less.
  - Add a `.filter(...)` over the data returned by a list-fetch hook
    where the filter could be a server query param. Filtering an
    already-truncated parent set silently drops data once the parent
    hits its cap. The CBL Resolution tab was the canonical case.

  See [`docs/dev plans`](../../../.claude/plans/list-pagination-completeness-1.0.md) for the full rationale; regression guards
  in `crates/server/tests/cbl_lists.rs::entries_endpoint_walks_past_old_500_cap`
  and `web/tests/api/cbl-entries-next-page.test.ts` anchor the
  invariant — keep those passing.

- **Metadata-provider writes** (metadata-providers-1.0): every
  write to provider-touched data (`external_ids`, junctions like
  `issue_credits` / `issue_characters` / etc., scalar fields with
  provenance tracking, covers) goes through the audited writer
  surface in [`crates/server/src/metadata/writers.rs`](crates/server/src/metadata/writers.rs)
  — never raw `INSERT INTO issue_credits` or
  `am.title = Set(...)` from a new metadata handler. The writers
  enforce the **user-precedence rule**: a row with
  `field_provenance.set_by='user'` (or `external_ids.set_by='user'`)
  is never silently overwritten by a non-user write. Bypassing the
  rule requires the explicit `override_user_edits` flag, which is
  admin-only and audited as `metadata_apply_force`.

  **Reviewer heuristic — reject PRs that:**
  - Add direct ActiveModel writes to `issue_credits`, `issue_characters`,
    `issue_teams`, `issue_locations`, `issue_arcs`, `issue_concepts`,
    `issue_objects`, `issue_universes`, `issue_genres`, `issue_tags`,
    `issue_reprints`, or any `series_*` junction. The
    `writers::set_issue_*` / `set_series_*` helpers are the only
    audited write surface; they also rebuild the CSV read-cache
    on the parent issue/series (see `docs/dev/schema-restructure.md`
    "denormalized read-cache" section).
  - Add `INSERT INTO external_ids` from a non-writers caller.
    Always `writers::set_external_id` so the precedence rule fires.
  - Iterate `MetadataField::iter()` without an `is_junction()` /
    `is_cover()` guard. The flat-column update path and the
    junction-reconcile path can't share a switch.
  - Read provider data from `issue.user_edited` JSON. That column
    is being retired; consult `field_provenance` via
    `fetch_field_provenance_map` instead.

  See [`docs/dev/metadata-providers.md`](docs/dev/metadata-providers.md)
  for the architecture, [`metadata-operator-guide.md`](docs/dev/metadata-operator-guide.md)
  for tunables, and [`schema-restructure.md`](docs/dev/schema-restructure.md)
  for the M0 column-to-table migration.

- **Metadata writeback** (metadata-sidecar-writeback-1.0): when a
  library has both `allow_archive_writeback = true` AND
  `metadata_writeback_enabled = true`, the apply path inverts: instead
  of writing DB rows directly, it composes ComicInfo + MetronInfo
  XML, rewrites both into the archive (atomic temp → fsync → .bak
  rotate → rename → fsync-parent), and enqueues a scoped rescan so
  the scanner re-ingests the freshly-written XML. The archive becomes
  the canonical source; the DB is downstream cache. Legacy DB-direct
  path stays for libraries with either flag off (dispatch lives in
  `apply_issue` / `apply_series`).

  **When adding a new metadata field**, the changeset must touch:
  1. The Rust struct in `crates/parsers/src/comicinfo.rs` (and/or
     `metroninfo.rs`).
  2. The serializer's `write_*` block in the same file.
  3. The composer in
     [`crates/server/src/metadata/sidecar_compose.rs`](crates/server/src/metadata/sidecar_compose.rs)
     (`compose_comicinfo` + `compose_metroninfo`).
  4. The scanner ingest path —
     [`process.rs`](crates/server/src/library/scanner/process.rs)
     (read the parsed value, stamp the issue/series row) and, when the
     field has a junction, the rollup in
     [`metadata_rollup.rs`](crates/server/src/library/scanner/metadata_rollup.rs).

  **Not the apply job.** The apply path runs the composer + enqueues
  the rewrite; the scanner ingest is the single place where parsed XML
  values become DB rows. Adding a direct `writers::set_*` call inside
  `apply_issue_via_sidecar` / `apply_series_via_sidecar` for entity-row
  writes bypasses the rescan-as-source-of-truth invariant and breaks
  drift detection.

  **Reviewer heuristic — reject PRs that:**
  - Add a new `writers::set_*` call inside `apply_*_via_sidecar` for
    a scalar / junction the composer can already emit. The XML +
    scanner ingest is the canonical path; direct writers from the
    sidecar apply path are reserved for **metadata-only** rows the
    XML schemas don't carry (today: variant covers via
    `set_issue_variants`).
  - Add a synth row to `library_health_issue` via the scanner. Drift
    surfacing is **synthesized per-request** in
    [`api/health_issues.rs::list`](crates/server/src/api/health_issues.rs) —
    not persisted, doesn't go through the `HealthCollector` lifecycle,
    doesn't have dismiss/resolve semantics.
  - Trigger an unscoped library scan from the apply path. Each
    `RewriteIssueSidecarsJob` enqueues a per-issue scoped rescan
    (`skip_rescan = false`); the series-scope apply overrides with
    `skip_rescan = true` and fires one series-scoped rescan after the
    fan-out completes. Library-wide rescans here would O(N²) the work.

  See [`docs/dev/metadata-sidecar-writeback.md`](docs/dev/metadata-sidecar-writeback.md)
  for the architecture, migration recipe, and risk matrix. The M7
  rollout gauge is `folio_metadata_writeback_libraries_remaining` —
  once it stays at zero, the follow-up cleanup PR drops the legacy
  DB-direct apply branch.

- **Cover-image perceptual hashes** (metadata-providers-1.0 M9):
  every new cover (provider-applied or scanner-extracted) gets
  `phash` + `dhash` + `ahash` computed at write time via
  [`crate::metadata::phash`](crates/server/src/metadata/phash.rs).
  Decode failures soft-fail (column stays NULL; backfill job picks
  it up later).

- **Matching engine** (matching-accuracy-1.0): the matcher inverts
  the pre-M4 weighted-bonus model — cover-pHash is now the
  **primary** bucket discriminant. [`Score::bucket`](crates/server/src/metadata/matcher.rs)
  consults `cover_hamming` first using ComicTagger's verbatim
  ladder (`STRONG_SCORE_THRESH=8`, `MIN_SCORE_THRESH=16`,
  `MIN_ALTERNATE_SCORE_THRESH=12` for variant-source matches,
  `MIN_SCORE_DISTANCE=4` for the gap-to-next-best guard). Text
  scoring is the fallback when no phash is available. Don't add a
  weighted `cover_phash` bonus back onto `total` — the M4 inversion
  is intentional and the golden regression suite at
  [`crates/server/tests/matching_accuracy_golden.rs`](crates/server/tests/matching_accuracy_golden.rs)
  enforces it.

  **Pre-filter** ([`orchestrator::pre_filter_series`](crates/server/src/metadata/orchestrator.rs))
  drops candidates BEFORE scoring on (a) hard year gate
  (`cand > local + 1`) and (b) per-library
  `metadata_publisher_blacklist`. Always build via
  `PreFilter::from_library(...)` so bad JSON shape soft-fails
  instead of panicking.

  **Text pipeline**:
  [`metadata::title_norm::sanitize_title`](crates/server/src/metadata/title_norm.rs)
  + [`metadata::ratcliff::three_pass_ratio`](crates/server/src/metadata/ratcliff.rs)
  mirror ComicTagger's normalization step-for-step so the same
  inputs produce the same comparison keys both tools would. The
  23-word article list is lifted verbatim; don't tune per-language
  without re-calibrating against the golden suite.

  **Reviewer heuristics — reject PRs that:**
  - Add a weighted `cover_phash` field back to `Score` or fold
    cover similarity into `score.total`. M4 inverted this
    deliberately.
  - Change any of the cover-Hamming ladder constants
    (`STRONG_SCORE_THRESH`, `MIN_SCORE_THRESH`,
    `MIN_SCORE_DISTANCE`, `MIN_ALTERNATE_SCORE_THRESH`) without
    re-running the golden suite AND adding boundary fixtures.
  - Introduce a new operator-tunable threshold without the full
    settings-registry + `Config` field + `apply_setting` branch +
    UI surface chain. The pattern:
    [`metadata.auto_apply_threshold`](crates/server/src/settings/registry.rs)
    is the template.
  - Read `library.metadata_publisher_blacklist` directly instead
    of `PreFilter::from_library`. The JSON column tolerates bad
    shape; the helper's `as_array().filter_map()` is the only path
    that doesn't panic on operator-written garbage.

  See [`docs/dev/matching-accuracy.md`](docs/dev/matching-accuracy.md)
  for the full pipeline diagram, operator-tunable knob list,
  telemetry recipe, and the fixture-adding playbook.

## Editing rules

- Server tests must hit a real DB via `TestApp::spawn()`; never mock sea-orm.
- Don't write to `audit_log` directly — use `crate::audit::record`.
- New endpoints register in **one** place: the module's `routes() ->
  OpenApiRouter<AppState>` function, via the `utoipa_axum::routes!()`
  macro. Each handler carries a `#[utoipa::path(...)]` annotation; the
  macro pulls method+path from it, axum routing AND the OpenAPI spec
  pick the handler up automatically. The module's `routes()` is then
  `.merge(...)`-ed into either the `bare` or `api` group in
  [`build_openapi_router`](crates/server/src/app.rs). There is no
  separate `paths(...)` or `components(schemas(...))` list to maintain
  (audit-remediation M1).
  - **Note:** `routes!(h1, h2)` combines handlers at the **same** path
    under different methods (e.g. `routes!(get_one, update)` →
    `GET + PATCH /admin/users/{id}`). Handlers on different paths
    must go in separate `.routes()` calls.
  - **Per-route layers** (rate-limits etc.): wrap the handler in its
    own sub-router so `route_layer` doesn't leak —
    `OpenApiRouter::new().routes(routes!(h)).route_layer(...)` —
    then `.merge(...)` it.
- After changing API surface, regenerate the OpenAPI spec with
  `just openapi`. The spec emits from the live router (via
  `OpenApiRouter::split_for_parts`), so any handler missing from the
  spec is also missing from the router. `just openapi-check` (CI gate)
  regenerates both `openapi.json` and `types.generated.ts` into temp
  paths and fails if either drifts from the checked-in copy.
- **Handler tracing** (audit-remediation M6): every `pub async fn` in
  `crates/server/src/api/` carries `#[handler]` from `server_macros`
  (above the `#[utoipa::path]` attribute). The macro applies
  `#[tracing::instrument(skip_all, name = "<fn_name>")]` with a
  `user_id` field auto-extracted from `CurrentUser` / `RequireAdmin`
  args. New handlers add the annotation by hand. See
  [docs/dev/logging.md](docs/dev/logging.md) for the full convention.
- **Request-body validation** (audit-remediation M9): request DTOs
  with non-trivial rules derive `garde::Validate` and the handler
  takes `Validated<T>` from
  [`crate::api::extractors`](crates/server/src/api/extractors.rs)
  instead of bare `Json<T>`. Garde failures land on 422 via the
  canonical envelope; malformed JSON stays at 400. For bounded
  enum-shaped query/path params, prefer typed enums (`MarkerKindFilter`,
  `TagMatchMode`, `UserRole`, `UserState`, etc.) over `String`-then-
  validate — serde rejects bad values at deserialize time. Cross-field
  rules where the required-set depends on a discriminator (e.g.
  `saved_views::validate_create`) stay as free functions; every
  semantic-validation failure returns 422 (the parse-shape errors —
  malformed UUID, malformed RFC3339, malformed JSON — stay 400).
- **Admin-action audit enforcement** (audit-remediation M10.2): the
  `audit-check` binary at `crates/tools/audit-check/` walks every
  handler under `crates/server/src/api/` and fails CI if any
  `RequireAdmin`-gated function returns success without calling
  `record_admin_action!` (or `audit::record(...)`). Read-only admin
  GETs and delegate-pattern wrappers (handlers that thin-wrap an
  audited helper) belong in
  [`allowlist.txt`](crates/tools/audit-check/allowlist.txt) with a
  one-line reason. Run locally via `just audit-check`.
- Memoize TanStack Query keys via the `queryKeys` registry in
  [web/lib/api/queries.ts](web/lib/api/queries.ts) — don't inline tuples at
  call sites.
- Sidebar nav entries that ship for real should drop `placeholder: true` from
  [web/components/admin/nav.ts](web/components/admin/nav.ts).

## Gitignore quirks

- `.dev-data/*` only matches at repo root. Nested `**/.dev-data/` is also
  ignored — important because those nested dirs hold local dev secrets.
- `fixtures/library/` is excluded (CBZ files, non-distributable).

## Where to look

- Architecture decisions per phase: [docs/dev/phase-status.md](docs/dev/phase-status.md)
- Library scanner deep dive: [docs/dev/library-scanner.md](docs/dev/library-scanner.md)
- Reader keyboard map: [docs/dev/reader-shortcuts.md](docs/dev/reader-shortcuts.md)
- OPDS readiness audit: [docs/dev/opds-audit.md](docs/dev/opds-audit.md)
- Runtime-config split (env vs DB): [docs/dev/runtime-configuration.md](docs/dev/runtime-configuration.md)
- OCR pipeline (detector + recognizer + cache + admin surfaces): [docs/dev/ocr.md](docs/dev/ocr.md)
- Logging conventions (#[handler] macro, severity levels, secret-redaction): [docs/dev/logging.md](docs/dev/logging.md)
- Observability two-stream split (Server vs Library stream; `library_events` manifest + writer/`EventCollector`/retention; `scan_batch`; ring-buffer `domain`/`error_code`): [docs/dev/observability.md](docs/dev/observability.md)
- Metadata providers architecture: [docs/dev/metadata-providers.md](docs/dev/metadata-providers.md)
- Metadata providers operator guide (API keys, weekly refresh, troubleshooting): [docs/dev/metadata-operator-guide.md](docs/dev/metadata-operator-guide.md)
- Metadata sidecar writeback (DB-canonical → XML-canonical inversion, per-library opt-in, drift surfacing): [docs/dev/metadata-sidecar-writeback.md](docs/dev/metadata-sidecar-writeback.md)
- Matching accuracy (ComicTagger-derived heuristics, threshold tuning, fixture-adding playbook): [docs/dev/matching-accuracy.md](docs/dev/matching-accuracy.md)
- M0 schema restructure (external_ids + junctions + field_provenance + issue_cover): [docs/dev/schema-restructure.md](docs/dev/schema-restructure.md)
- Active plans live under `~/.claude/plans/`; check the auto-memory index for
  what's currently in flight vs. shipped.
