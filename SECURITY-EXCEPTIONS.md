# npm dependency advisory exceptions

This file documents accepted `pnpm audit` advisories that are **not** remediated
by a version bump or `pnpm.overrides` entry, with a rationale for each. It mirrors
the discipline of the Rust `deny.toml` `[advisories].ignore` list. Revisit every
dependency-bump pass.

For Rust crates, see [`deny.toml`](deny.toml). Remediated npm advisories live in
the root [`package.json`](package.json) `pnpm.overrides` block.

## Accepted

Each entry is listed in `pnpm-workspace.yaml` `auditConfig.ignoreGhsas` (the
`pnpm audit` gates) and mirrored in `osv-scanner.toml` (the daily OSV sweep).

### GHSA-vfj7-8cjw-p6xm — `braces` <=3.0.3 stack-exhaustion DoS (high)

- **Accepted:** 2026-10-03.
- **Why no fix:** no patched release exists; 3.0.3 is the latest `braces`, so
  there is nothing to pin an override to.
- **Reach:** dev dependencies only —
  `eslint-config-next` / `@next/eslint-plugin-next` > `fast-glob` >
  `micromatch` > `braces`. It runs during linting over the repository's own
  glob patterns, never on user input, and is not in the production images
  (`pnpm audit --prod` is clean).
- **Delete when:** `braces` ships a patched release (then remove the ignore
  from both files and let the lockfile pick it up, adding an override if a
  consumer's range pins the old version).

## Resolved

The `brace-expansion` DoS pair (GHSA-mh99-v99m-4gvg, and GHSA-rgw5-rvv9-x895
which bypasses the first one's mitigation) was accepted for the 1.1.x / 2.1.x
copies on the grounds that no patched backport existed for those majors, and
that forcing them onto 5.0.8 broke minimatch v3 (`TypeError: expand is not a
function` — its CJS build named-exports `expand` where v3 `require()`s a bare
function). That stated delete-when condition — "a patched 1.x/2.x backport
ships" — has now been met: rgw5 shipped 1.1.18 / 2.1.4 / 5.0.9 and mh99's
backports (1.1.17 / 2.1.3) landed with them. Each line is now pinned to its own
patched release, so every consumer stays inside its original major and the
export-shape break never comes up. Verified: `pnpm audit --audit-level=high`
clean, eslint runs with 0 errors.

The previously-accepted `uuid` < 11.1.1 advisory (GHSA-w5hq-g745-h8pq) reached
the graph only through the Docusaurus docs site (`docs-site` →
`webpack-dev-server` → `sockjs` → `uuid`). That workspace was removed, so the
advisory no longer applies.
