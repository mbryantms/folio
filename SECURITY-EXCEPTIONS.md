# npm dependency advisory exceptions

This file documents accepted `pnpm audit` advisories that are **not** remediated
by a version bump or `pnpm.overrides` entry, with a rationale for each. It mirrors
the discipline of the Rust `deny.toml` `[advisories].ignore` list. Revisit every
dependency-bump pass.

For Rust crates, see [`deny.toml`](deny.toml). Remediated npm advisories live in
the root [`package.json`](package.json) `pnpm.overrides` block.

## Accepted

### GHSA-w5hq-g745-h8pq — `uuid` < 11.1.1 (moderate)

- **Reaches the graph via:** `docs-site` → `@docusaurus/core` → `webpack-dev-server` → `sockjs` → `uuid@8.3.2`.
- **Why accepted:**
  1. **Not shipped / dev-only.** `uuid` enters only through `webpack-dev-server`'s
     `sockjs` dependency, which runs solely during `docusaurus start` (local dev
     server). It is never in the production docs build output and never in the
     Folio app (`web`) or the Rust server.
  2. **Vulnerable code path not exercised.** The advisory concerns a missing buffer
     bounds check in `uuid` **v3/v5/v6** when a caller supplies a `buf` argument.
     `sockjs` calls `uuid.v4()` (random, no `buf`), which is not the affected path.
  3. **No compatible fix available.** The patched line is `uuid >= 11.1.1`; the v8
     line `sockjs` consumes has no patched release, and forcing v11+ breaks
     `sockjs` (the v11 ESM named-export API is incompatible with `sockjs`'s
     `require('uuid')` default-import usage).
- **Revisit when:** `webpack-dev-server` / `sockjs` (via a Docusaurus bump) moves to a `uuid` major that carries the fix.
