# npm dependency advisory exceptions

This file documents accepted `pnpm audit` advisories that are **not** remediated
by a version bump or `pnpm.overrides` entry, with a rationale for each. It mirrors
the discipline of the Rust `deny.toml` `[advisories].ignore` list. Revisit every
dependency-bump pass.

For Rust crates, see [`deny.toml`](deny.toml). Remediated npm advisories live in
the root [`package.json`](package.json) `pnpm.overrides` block.

## Accepted

_None currently._

The previously-accepted `uuid` < 11.1.1 advisory (GHSA-w5hq-g745-h8pq) reached
the graph only through the Docusaurus docs site (`docs-site` →
`webpack-dev-server` → `sockjs` → `uuid`). That workspace was removed, so the
advisory no longer applies.
