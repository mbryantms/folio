# Dependency management — observe, don't operate

Policy for how third-party code enters this repo. The goal is that
updates flow in on their own wherever a machine can verify them, with a
reviewable trail for every change, and that a human is asked for exactly
two things: decisions automation cannot make (majors, breaking 0.x
minors, infrastructure lines) and failures automation cannot fix.

Last reviewed: 2026-09-13.

## The three buckets

### Merges without you

Every one of these still needs the full required-check set green
(`Rust — fmt / clippy / test`, `Rust — cargo deny / audit`,
`Web — lint / typecheck / test`, `OpenAPI — snapshot + breaking-change
check`, `Rust — audit-log enforcement`, `Docker — image smoke test`) and
sits for `minimumReleaseAge: 3 days` first so upstream regressions
surface before we take them. Renovate PRs run the Rust suite with **zero
test retries** (`.config/nextest.toml` `ci-deps`) so a flaky regression
cannot be retried into a pass; the Playwright suite keeps one retry in
CI (`web/playwright.config.ts`) because browser-level flakes are real
and the run is short.

| What | Why it is safe |
| --- | --- |
| cargo + npm **patch/minor** of `>=1.0` packages | Semver-compatible by contract; full suite + image smoke verify it. |
| cargo + npm **patch** of `0.x` packages | `0.x.y -> 0.x.z` is compatible under cargo and npm caret rules. |
| Weekly **web dev-tooling** group (vitest, prettier, tailwind, …) | Dev-only; one Monday PR instead of a trickle. |
| **Lock-file maintenance** (weekly, transitive refresh) | Only moves within declared ranges; the suite is the gate. |
| **Docker digest** refreshes of pinned tags (`postgres:18-alpine@sha256:…`) | Same tag, newer OS packages — the CVE fix we want; smoke test boots it. |
| **GitHub Actions** patch/minor/digest | Digest-pinned; CI is the consumer and proves it. |
| **Rust toolchain** patch/minor (`rust-toolchain.toml` + Dockerfile `rust:` tag) | clippy `-D warnings`, the suite and the Docker build all run on the new compiler. |
| **pnpm** patch/minor (package.json + workflows + `web/Dockerfile`, one PR) | The pin is no longer recorded in the lockfile (`pmOnFail: ignore`), so the bump is verifiable by CI. |
| **Security PRs** from Dependabot/OSV alerts | Skip the 3-day wait; normal automerge rules still apply by update type. |
| Web **render-path minors** (react, next, Radix, TanStack, …) | Covered since 2026-09-13 by the Playwright reader flow + jsdom suites (see "What the gate actually proves"). |

### Waits for you (label on the PR, listed on the Dependency Dashboard)

| Label | What | Why a machine can't decide |
| --- | --- | --- |
| `major-review` | Any major | Migration work by definition; release notes are in the PR body. |
| `zerover-review` | `0.x` **minor** | The breaking step for 0.x under both ecosystems' semver rules. |
| `gated-major` | eslint 10, typescript 7, `@types/node` major | Known upstream blockers, documented in the rule description. |
| `needs-migration-review` | react-hook-form, `@hookform/resolvers`, openapi-typescript, `@tanstack/react-query` | History of breaking-within-minor; codegen shim must move with openapi-typescript. |
| `coordinated-bump` | reqwest/oauth2/openidconnect, sea-orm/sqlx, apalis/redis, testcontainers, RustCrypto | Must co-resolve; grouped so they never arrive alone. |
| `infra-review` | A **tag** change of postgres/redis/dex/alpine/curl or a Dockerfile base line | A postgres major needs a data-dir plan; dev and CI must stay on the tested major. |
| `low-merge-confidence` | Anything Mend Merge Confidence marks **low** | Other repos reported regressions on that exact release. |

To merge one: review the PR body (changelog + Age/Confidence columns),
merge it. To skip a version: close the PR (Renovate remembers). To
re-run CI: tick the PR's rebase box on the Dependency Dashboard.

### Alerts you

GitHub does not notify you about your own PRs, and Renovate runs as your
account — so PR activity is *not* the alert channel. These are:

| Signal | Source | When |
| --- | --- | --- |
| Issue **"Dependency health: attention needed"** | `.github/workflows/advisories.yml`, daily 06:17 UTC | Any of: new RUSTSEC advisory, high npm advisory, OSV hit, HIGH/CRITICAL fixable CVE in the published images, or a Renovate PR that should have auto-merged but is red / >7 days old. Created once, body rewritten daily, commented when the failing set changes, auto-closed when green. |
| Issue **Dependency Dashboard** (#352) | Renovate | Single pane: pending (age-gated), open, held, blocked-by-closed, OSV summary. No notification; look when you want. |
| Release PR (`chore: release vX.Y.Z`) | release-please | Every dependency merge lands in the next release's "Dependencies" section. |
| CI step summary "Image CVE scan" | `docker-smoke` job on every PR | Informational table; never blocks. |

Silence everywhere else: a green automerge leaves only its merge commit,
the release-notes line, and the SBOM artifact.

## The trail

* **PR body** — Renovate's table with the change, release notes / changelog
  excerpt, Mend Age + Confidence badges, and the rule that applied.
* **Merge commit** — `deps: update …` (conventional commit; picked up by
  release-please).
* **SBOM** — every CI run uploads `folio-source-sbom-<sha>.spdx.json`
  (syft over `Cargo.lock` + `pnpm-lock.yaml`, 90-day retention); every
  published image carries an SPDX SBOM + cosign signature (`release.yml`).
* **Pins** — every base image and service image is `tag@sha256:…`; every
  GitHub Action is digest-pinned. `git log -p -- Dockerfile compose.*.yml`
  is the image history.

## What the gate actually proves

The Rust side: ~1,800 tests, nearly every endpoint exercised against real
Postgres 18 + Redis 8, migrations applied per test process, wiremock on
every outbound HTTP client, a real OIDC round-trip, the CBR reader against
a committed RAR5 fixture (`fixtures/synthetic-3page.cbr`), and
`tests/job_worker_loop.rs`, which boots the real apalis `Monitor` and
drives a job through Redis end to end.

The web side, as of 2026-09-13:

* **Playwright reader flow in CI** (`web/tests/e2e/reader-flow.spec.ts`,
  run by the `docker-smoke` job against the booted production images
  through the Rust origin): register the first user → create a library
  over a generated three-page CBZ → scan → series page → reader → page
  turn → progress persisted (API) → reload resumes on page 2 → series CTA
  reads "Continue reading". Plus the CSP-nonce and a11y specs on the same
  stack. This is the only test that exercises hydration, event handlers,
  the proxy hop, cookies + CSRF and the scan pipeline together, and it is
  what lets React / Next / Radix / TanStack minors auto-merge.
* **jsdom render tests** (`web/tests/dom/*`, opt-in per file with
  `// @vitest-environment jsdom`): sign-in/register form, reader chrome,
  marker editor — react-hook-form + zod, Radix popover/menu/tabs, zustand
  wiring, fetch submission, toast contracts.
* Everything else in `web/tests/` is static-markup or pure-function and
  proves shape, not behaviour.

What is still unproven, in priority order:

1. Reader **gestures and webtoon/double modes** — the e2e turns one page
   with the keyboard in single mode.
2. **Search, collections, saved views, admin forms** — no browser coverage;
   vitest static-markup only.
3. **Metadata provider apply** end to end (wiremock covers the clients;
   the apply job runs only via direct handler calls).
4. **CB7** — no decoder in the graph (scaffold only); nothing to test yet.

## Where things are

| Concern | File |
| --- | --- |
| Renovate rules (each has a `description` saying why) | `renovate.json` |
| Rust advisory ignores (documented, dated) | `deny.toml` `[advisories].ignore`, `.cargo/audit.toml` |
| Licence allow-list, banned sources | `deny.toml` |
| npm advisory exceptions | `SECURITY-EXCEPTIONS.md`, `pnpm-workspace.yaml` `overrides` |
| Per-PR gates | `.github/workflows/ci.yml` |
| Daily sweep + tracking issue | `.github/workflows/advisories.yml` |
| Image SBOM + signature | `.github/workflows/release.yml` |
| MSRV | `Cargo.toml` `rust-version` = the `dtolnay/rust-toolchain@…` refs in `ci.yml` (bump together) |
| Dev toolchain | `rust-toolchain.toml` + Dockerfile `rust:` tag (Renovate group `rust-toolchain`) |
| Node | `engines.node >=24`, `.nvmrc`, `node-version: 24` in workflows, `node:24-…` in `web/Dockerfile` (bump together) |

## Adding a dependency

* Prefer crates/packages at `>=1.0` — everything below it needs a human for
  every minor.
* A new npm package with an install script must be listed in
  `pnpm-workspace.yaml` `allowBuilds` (currently every entry is `false`:
  no install script runs, prebuilt binaries only).
* Run `cargo deny check` locally; an unfamiliar licence fails CI.
