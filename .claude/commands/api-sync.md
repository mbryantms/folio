---
description: Regenerate OpenAPI spec from utoipa and verify TypeScript types are in sync with the Rust API surface.
allowed-tools: Bash(cargo run:*), Bash(diff:*), Bash(jq:*), Bash(grep:*), Bash(pnpm:*), Bash(git:*)
---

Regenerate the OpenAPI spec and verify `web/lib/api/types.ts` stays in sync
with the Rust API surface. The repo's `pnpm openapi:gen` script points at the
wrong path; this command does it correctly.

**Steps**:

1. Snapshot the current spec for diff: `cp web/lib/api/openapi.json /tmp/openapi.before.json` (silently ignore if missing).

2. Regenerate: `cargo run --bin server -- --emit-openapi 2>/dev/null > web/lib/api/openapi.json`

3. Diff the spec:
   - `diff -u /tmp/openapi.before.json web/lib/api/openapi.json | head -120`
   - If no diff, report "No API surface change" and exit.

4. Identify what changed. Use `jq` to enumerate added/removed paths and changed
   schemas:
   - Added paths: `jq -r '.paths | keys[]'` on each, sort, comm.
   - Changed schemas: list `components.schemas` keys whose subtree changed.

5. For each net-new path or schema, check whether the corresponding
   TypeScript type exists in `web/lib/api/types.ts`. The convention is:
   - utoipa `ToSchema` struct `FooView` → TS `export type FooView`
   - request bodies named `XxxReq` → TS `XxxReq`
   - The hand-written types file (until codegen lands) is authoritative for
     the web; missing additions there will type-error in the consumers.

6. **If types.ts needs updates**, edit it in place to add the new types
   matching the Rust shapes. Mirror naming exactly. Map types:
   - `String` → `string`
   - `Option<T>` → `T | null`
   - `Vec<T>` → `T[]`
   - `serde_json::Value` → `unknown`
   - `DateTime` → `string` (ISO 8601)
   - `Uuid` → `string`

7. Run `pnpm --filter web run typecheck` to confirm consumers compile.

8. **Report** (terse):
   - Files changed: `openapi.json`, optionally `types.ts`.
   - Added paths: `[GET /admin/foo, POST /admin/foo/{id}]`
   - Added/changed schemas: `[FooView, BarReq]`
   - Typecheck: ✓ or ✗ with the offending file:line.

Do not commit. If the user wants a commit, they'll ask.

Args: `$ARGUMENTS` — `--no-typecheck` to skip step 7 (faster iteration).
