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

- **Error envelope**: every server error is `{"error": {"code": "...", "message": "..."}}`.
  See `error()` helper in [crates/server/src/api/libraries.rs](crates/server/src/api/libraries.rs).
- **Admin guard**: inline `if user.role != "admin" { return error(...) }`. No
  middleware-level enforcement yet. The `CurrentUser` extractor lives at
  [crates/server/src/auth/extractor.rs](crates/server/src/auth/extractor.rs).
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
- Active plans live under `~/.claude/plans/`; check the auto-memory index for
  what's currently in flight vs. shipped.
