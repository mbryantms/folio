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
- **Error envelope**: every server error is `{"error": {"code": "...", "message": "..."}}`.
  See `error()` helper in [crates/server/src/api/libraries.rs](crates/server/src/api/libraries.rs).
- **Admin guard**: inline `if user.role != "admin" { return error(...) }`. No
  middleware-level enforcement yet. The `CurrentUser` extractor lives at
  [crates/server/src/auth/extractor.rs](crates/server/src/auth/extractor.rs).
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
- **Hand-written `web/lib/api/types.ts`**: Authoritative until
  `openapi-typescript` codegen is wired into CI. `just openapi` writes
  the spec to `web/lib/api/openapi.json` and then generates a reference
  copy at `web/lib/api/types.generated.ts` — diff against `types.ts`
  to spot drift, but don't replace `types.ts` until codegen becomes
  the source of truth.
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

## Editing rules

- Server tests must hit a real DB via `TestApp::spawn()`; never mock sea-orm.
- Don't write to `audit_log` directly — use `crate::audit::record`.
- New admin endpoints register in three places: `routes()` in the module,
  `paths(...)` and `components(schemas(...))` in
  [crates/server/src/app.rs](crates/server/src/app.rs), and the route
  `.merge(...)` line in `router()`.
- After changing API surface, regenerate the OpenAPI spec and update
  `web/lib/api/types.ts` (or run `/api-sync`).
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
- Active plans live under `~/.claude/plans/`; check the auto-memory index for
  what's currently in flight vs. shipped.
