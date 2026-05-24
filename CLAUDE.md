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

- Server integration tests use `testcontainers` to spin up Postgres + Redis per
  test process. They're slow (≈25s warmup). Run a single file with
  `cargo test -p server --test <name>` to iterate.
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
  `{"error": {"code": "...", "message": "..."}}`. The **only**
  construction site is
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
- Active plans live under `~/.claude/plans/`; check the auto-memory index for
  what's currently in flight vs. shipped.
