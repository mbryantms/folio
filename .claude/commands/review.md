---
description: Opinionated review of staged + unstaged changes against this project's conventions. Surfaces violations and suggests fixes.
argument-hint: "[base-ref]  (default: main)"
allowed-tools: Bash(git diff:*), Bash(git status:*), Bash(git log:*), Bash(git show:*), Bash(rg:*), Bash(grep:*)
---

Review the working-tree diff against the project's conventions and suggest
concrete changes. The base ref to diff against is `$1` (default `main`).

**Read the diff first**:
- `git status --short`
- `git diff --stat $1...HEAD` (committed changes vs base)
- `git diff` (uncommitted)
- Read [CLAUDE.md](CLAUDE.md) — that's the convention spec for this project.

**Review against these specific conventions** (in priority order):

1. **Error envelope**: every server error path produces
   `{"error": {"code": "...", "message": "..."}}`. Flag any handler returning
   bare strings, `anyhow::Error::to_string()` leaks, or non-conforming bodies.

2. **Admin guard placement**: `if user.role != "admin"` must be the first
   thing in admin handlers, before any DB read. Catches handlers that leak
   existence of resources to non-admins.

3. **Audit log emissions**: every mutating admin handler that changes user-
   visible state should `crate::audit::record(...)` with a dotted action name.
   Skipping is OK only for idempotent no-ops; flag anything else.

4. **CSRF / auth boundary**: new POST/PATCH/DELETE routes inherit the global
   CSRF middleware — verify there's no inadvertent path-exempt change in
   [crates/server/src/auth/csrf.rs](crates/server/src/auth/csrf.rs).

5. **OpenAPI registration**: new utoipa-annotated handlers must be added to
   `paths(...)` AND new `ToSchema` structs to `components(schemas(...))` in
   [crates/server/src/app.rs](crates/server/src/app.rs). Missing entries
   silently drop the type from `web/lib/api/openapi.json`.

6. **types.ts sync**: every API-surface change in Rust should have a matching
   edit in [web/lib/api/types.ts](web/lib/api/types.ts) (until codegen
   lands). Suggest running `/api-sync` if you spot drift.

7. **Query key registry**: TanStack Query `queryKey` arrays must come from
   the `queryKeys` registry in
   [web/lib/api/queries.ts](web/lib/api/queries.ts). Inline tuples drift
   silently and break invalidation.

8. **React 19 patterns**: flag `useEffect` calls whose body only `setState`
   based on a prop (use the prev-value-during-render idiom instead — the
   `react-hooks/set-state-in-effect` rule will fail lint anyway). Flag JSX
   text containing unescaped `'` (`react/no-unescaped-entities`).

9. **Comments / scope creep**: per CLAUDE.md, no narration comments, no
   "added for X" comments, no scope expansion past the task. Flag any
   comment that explains *what* code does rather than *why*.

10. **Test colocation**: server integration tests in `crates/server/tests/`,
    web tests in `web/tests/`. Server tests must use `TestApp::spawn()` —
    flag any sea-orm mocking. Web tests that mock `next/navigation` and
    `apiGet` — confirm the existing pattern is followed.

**Output format**:

For each finding, write a 1-3 line block:
```
[severity] file:line — short title
  what's wrong, in one sentence.
  → suggested fix in one sentence.
```

Severity is `BLOCKER` / `MAJOR` / `MINOR` / `NIT`. Sort by severity desc.

End with:
- A 1-2 sentence overall verdict.
- A "Run before merge" checklist (e.g., `/check`, `/api-sync`, manual smoke).

Cap the report at ~40 lines. If everything's clean, say so and stop.
