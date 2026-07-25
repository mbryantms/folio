# npm dependency advisory exceptions

This file documents accepted `pnpm audit` advisories that are **not** remediated
by a version bump or `pnpm.overrides` entry, with a rationale for each. It mirrors
the discipline of the Rust `deny.toml` `[advisories].ignore` list. Revisit every
dependency-bump pass.

For Rust crates, see [`deny.toml`](deny.toml). Remediated npm advisories live in
the root [`package.json`](package.json) `pnpm.overrides` block.

## Accepted

### GHSA-mh99-v99m-4gvg — brace-expansion ≤5.0.7 DoS (high), 1.x/2.x copies only

- **What**: DoS via unbounded expansion length causing an out-of-memory crash.
  The advisory range spans every major; the only patched release is 5.0.8.
- **Remediated part**: all 5.x copies are forced to 5.0.8 via the
  `pnpm-workspace.yaml` override — that covers the prod-graph path
  (`@serwist/next → glob → minimatch v10`).
- **Accepted part**: the 1.1.x / 2.1.x copies (minimatch v3/v9 chains —
  eslint, transitive dev/build tooling). No patched backport exists for
  those majors, and forcing 5.0.8 breaks them: its CJS build named-exports
  `expand` while minimatch v3 `require()`s a bare function — verified
  eslint crash (`TypeError: expand is not a function`).
- **Exposure**: these copies expand developer-authored glob patterns
  (eslint config matching, build-time file globbing). No attacker-supplied
  input reaches them at runtime; worst case is a self-inflicted OOM of a
  dev/CI process.
- **Delete when**: a patched 1.x/2.x backport ships, or the consuming
  minimatch chains move to brace-expansion ≥5.0.8 — then remove the
  `ignoreGhsas` entry AND the override together (`pnpm why brace-expansion`
  must show no <5.0.8 consumers).

The previously-accepted `uuid` < 11.1.1 advisory (GHSA-w5hq-g745-h8pq) reached
the graph only through the Docusaurus docs site (`docs-site` →
`webpack-dev-server` → `sockjs` → `uuid`). That workspace was removed, so the
advisory no longer applies.
