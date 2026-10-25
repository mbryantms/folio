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

| What                                                                            | Why it is safe                                                                                               |
| ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| cargo + npm **patch/minor** of `>=1.0` packages                                 | Semver-compatible by contract; full suite + image smoke verify it.                                           |
| cargo + npm **patch** of `0.x` packages                                         | `0.x.y -> 0.x.z` is compatible under cargo and npm caret rules.                                              |
| Weekly **web dev-tooling** group (vitest, prettier, tailwind, …)                | Dev-only; one Monday PR instead of a trickle.                                                                |
| **Lock-file maintenance** (weekly, transitive refresh)                          | Only moves within declared ranges; the suite is the gate.                                                    |
| **Docker digest** refreshes of pinned tags (`postgres:18-alpine@sha256:…`)      | Same tag, newer OS packages — the CVE fix we want; smoke test boots it.                                      |
| **GitHub Actions** patch/minor/digest                                           | Digest-pinned; CI is the consumer and proves it.                                                             |
| **Rust toolchain** patch/minor (`rust-toolchain.toml` + Dockerfile `rust:` tag) | clippy `-D warnings`, the suite and the Docker build all run on the new compiler.                            |
| **pnpm** patch/minor (package.json + workflows + `web/Dockerfile`, one PR)      | The pin is no longer recorded in the lockfile (`pmOnFail: ignore`), so the bump is verifiable by CI.         |
| **Security PRs** from Dependabot/OSV alerts                                     | Skip the 3-day wait; normal automerge rules still apply by update type.                                      |
| Web **render-path minors** (react, next, Radix, TanStack, …)                    | Covered since 2026-09-13 by the Playwright reader flow + jsdom suites (see "What the gate actually proves"). |

### Waits for you (label on the PR, listed on the Dependency Dashboard)

| Label                    | What                                                                                                     | Why a machine can't decide                                                        |
| ------------------------ | -------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| `major-review`           | Any major                                                                                                | Migration work by definition; release notes are in the PR body.                   |
| `zerover-review`         | `0.x` **minor**                                                                                          | The breaking step for 0.x under both ecosystems' semver rules.                    |
| `gated-major`            | eslint 10, typescript 7, `@types/node` major                                                             | Known upstream blockers, documented in the rule description.                      |
| `needs-migration-review` | react-hook-form, `@hookform/resolvers`, openapi-typescript, `@tanstack/react-query`                      | History of breaking-within-minor; codegen shim must move with openapi-typescript. |
| `coordinated-bump`       | reqwest/oauth2/openidconnect, sea-orm/sqlx, apalis/redis, utoipa/utoipa-axum, testcontainers, RustCrypto | Must co-resolve; grouped so they never arrive alone.                              |
| `infra-review`           | A **tag** change of postgres/redis/dex/alpine/curl or a Dockerfile base line                             | A postgres major needs a data-dir plan; dev and CI must stay on the tested major. |

Mend Merge Confidence is display-only here: the Age/Confidence badges in
PR bodies render without credentials, but a `matchConfidence` package
rule needs a Merge Confidence API token the self-hosted runner does not
have. Without one Renovate aborts every run with
`missing-api-credentials` before touching a single branch (2026-09-13,
a ~34-hour outage). Do not reintroduce such a rule without adding the
`merge-confidence` host rule on the runner first.

To merge one: review the PR body (changelog + Age/Confidence columns),
merge it. To skip a version: close the PR (Renovate remembers). To
re-run CI: tick the PR's rebase box on the Dependency Dashboard.

### Alerts you

GitHub does not notify you about your own PRs, and Renovate runs as your
account — so PR activity is _not_ the alert channel. These are:

| Signal                                          | Source                                              | When                                                                                                                                                                                                                                                                                     |
| ----------------------------------------------- | --------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Issue **"Dependency health: attention needed"** | `.github/workflows/advisories.yml`, daily 06:17 UTC | Any of: new RUSTSEC advisory, high npm advisory, OSV hit, HIGH/CRITICAL fixable CVE in the published images, or a Renovate PR that should have auto-merged but is red / >7 days old. Created once, body rewritten daily, commented when the failing set changes, auto-closed when green. |
| Issue **Dependency Dashboard** (#352)           | Renovate                                            | Single pane: pending (age-gated), open, held, blocked-by-closed, OSV summary. No notification; look when you want.                                                                                                                                                                       |
| Release PR (`chore: release vX.Y.Z`)            | release-please                                      | Every dependency merge lands in the next release's "Dependencies" section.                                                                                                                                                                                                               |
| CI step summary "Image CVE scan"                | `docker-smoke` job on every PR                      | Informational table; never blocks.                                                                                                                                                                                                                                                       |

Silence everywhere else: a green automerge leaves only its merge commit,
the release-notes line, and the SBOM artifact.

## The trail

- **PR body** — Renovate's table with the change, release notes / changelog
  excerpt, Mend Age + Confidence badges, and the rule that applied.
- **Merge commit** — `deps: update …` (conventional commit; picked up by
  release-please).
- **SBOM** — every CI run uploads `folio-source-sbom-<sha>.spdx.json`
  (syft over `Cargo.lock` + `pnpm-lock.yaml`, 90-day retention); every
  published image carries an SPDX SBOM + cosign signature (`release.yml`).
- **Pins** — every base image and service image is `tag@sha256:…`; every
  GitHub Action is digest-pinned. `git log -p -- Dockerfile compose.*.yml`
  is the image history.

## apalis 1.0 (job queue) — what changed and what survives an upgrade

`apalis` / `apalis-redis` / `redis` moved 0.7 / 0.7 / 0.32 → 1.0 / 1.0 / 1.7
together (2026-10, replaces Renovate #379). Operator-visible facts:

- **Redis key names are unchanged.** `jobs::storage_config` names each
  queue after the job type, exactly the namespace 0.7 derived, so
  `{queue}:active`, `:scheduled`, `:dead`, `:data` and `:inflight:{worker}`
  keep their names. 0.7's `:consumers` ZSET became `:workers` (members are
  worker names), and per-job metadata moved from the stored payload into a
  `{queue}:meta:{task_id}` hash (`attempts`, `status`, `result`, `done_at`).
  The dead set is now a plain SET; the kill time is `done_at` in the meta
  hash.
- **Payloads are not compatible.** 0.7 stored the whole `Request` JSON;
  1.0 stores the bare args. `JobRuntime::purge_legacy_queue_state` runs at
  boot: any queue that still has a 0.7 `:consumers` key is dropped
  wholesale and a `WARN` names it. Every job type is re-derivable (scans
  re-trigger, thumbnail marks re-seed from the DB, metadata runs re-queue),
  so the loss is only the backlog that was queued when the old build
  stopped. Drain queues before upgrading if that matters.
- **Retry budget stays 5.** apalis-redis 1.0 reschedules failures until a
  cut-off hardcoded in its Lua (25) and ignores per-task `max_attempts`, so
  `jobs::with_retry_budget` wraps every handler and reports the failure on
  attempt `JOB_MAX_ATTEMPTS` as an `AbortError` → `Killed` → dead set.
- **Orphan window stays 300 s**, expressed as `heartbeat_interval (30 s) ×
missed_heartbeats (10)`.
- **Fetch cadence is explicit.** 0.7 polled Redis every 100 ms; a 1.0 worker
  parks after an empty fetch and only wakes on the heartbeat unless a poll
  strategy is attached. The first 1.0 build shipped without one, so every
  queue hop waited 0–30 s (a metadata apply → rewrite → rescan chain took
  about a minute on prod). Every worker now gets
  `poll_with_interval(jobs::JOB_POLL_INTERVAL)` (500 ms);
  `tests/job_worker_loop.rs` pins idle-worker pickup under 10 s.

## What the gate actually proves

The Rust side: ~1,800 tests, nearly every endpoint exercised against real
Postgres 18 + Redis 8, migrations applied per test process, wiremock on
every outbound HTTP client, a real OIDC round-trip, the CBR reader against
a committed RAR5 fixture (`fixtures/synthetic-3page.cbr`), and
`tests/job_worker_loop.rs`, which boots the real apalis `Monitor` and
drives a job through Redis end to end.

The web side, as of 2026-09-13:

- **Playwright reader flow in CI** (`web/tests/e2e/reader-flow.spec.ts`,
  run by the `docker-smoke` job against the booted production images
  through the Rust origin): register the first user → create a library
  over a generated three-page CBZ → scan → series page → reader → page
  turn → progress persisted (API) → reload resumes on page 2 → series CTA
  reads "Continue reading". The registration, library and scan run once in
  the Playwright `setup` project (`admin.setup.ts`), whose saved admin
  session the reader-flow and relationship-review specs share (WP-8.5).
  Plus the CSP-nonce and a11y specs on the same stack. This is the only test that exercises hydration, event handlers,
  the proxy hop, cookies + CSRF and the scan pipeline together, and it is
  what lets React / Next / Radix / TanStack minors auto-merge.
- **jsdom render tests** (`web/tests/dom/*`, opt-in per file with
  `// @vitest-environment jsdom`): sign-in/register form, reader chrome,
  marker editor — react-hook-form + zod, Radix popover/menu/tabs, zustand
  wiring, fetch submission, toast contracts.
- Everything else in `web/tests/` is static-markup or pure-function and
  proves shape, not behaviour.

What is still unproven, in priority order:

1. Reader **gestures and webtoon/double modes** — the e2e turns one page
   with the keyboard in single mode.
2. **Search, collections, saved views, admin forms** — no browser coverage;
   vitest static-markup only.
3. **Metadata provider apply** end to end (wiremock covers the clients;
   the apply job runs only via direct handler calls).
4. **CB7** — `sevenz-rust2` bumps are covered by the archive-crate
   fixture suite (`crates/archive/tests/cb7_fixture.rs`: non-solid, solid,
   encrypted, bomb, traversal names) and the scan-time conversion tests,
   but no browser e2e — a CB7 only reaches the reader after conversion to
   CBZ.

## Where things are

| Concern                                              | File                                                                                                            |
| ---------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| Renovate rules (each has a `description` saying why) | `renovate.json`                                                                                                 |
| Rust advisory ignores (documented, dated)            | `deny.toml` `[advisories].ignore`, `.cargo/audit.toml` — see "Accepted advisories" below                        |
| OSV mirror of every accepted advisory (Rust + npm)   | `osv-scanner.toml` — generated by hand, gated by `scripts/ci/check-advisory-ignores.py`                         |
| Licence allow-list, banned sources                   | `deny.toml`                                                                                                     |
| npm advisory exceptions                              | `SECURITY-EXCEPTIONS.md`, `pnpm-workspace.yaml` `auditConfig.ignoreGhsas` + `overrides`                         |
| Per-PR gates                                         | `.github/workflows/ci.yml`                                                                                      |
| Daily sweep + tracking issue                         | `.github/workflows/advisories.yml`                                                                              |
| Image SBOM + signature                               | `.github/workflows/release.yml`                                                                                 |
| MSRV                                                 | `Cargo.toml` `rust-version` = the `dtolnay/rust-toolchain@…` refs in `ci.yml` (bump together)                   |
| Dev toolchain                                        | `rust-toolchain.toml` + Dockerfile `rust:` tag (Renovate group `rust-toolchain`)                                |
| Node                                                 | `engines.node >=24`, `.nvmrc`, `node-version: 24` in workflows, `node:24-…` in `web/Dockerfile` (bump together) |

## Accepted advisories

An advisory we cannot fix — the patched release is a breaking line our
direct dependencies have not moved to, or the vulnerable copy is a
transitive pin inside an optional stack — is **accepted**, never
silenced. Four files are involved and three scanners read them, each
blind to the others:

| File                              | Read by                                          | Carries                                                                                                                                                                                          |
| --------------------------------- | ------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `deny.toml` `[advisories].ignore` | cargo-deny (per-PR CI + daily)                   | Every Rust id, with the full rationale: which crate, how it reaches the graph, why it is acceptable, and a **`Revisit when…` / `Delete when…`** condition. This is the Rust rationale of record. |
| `.cargo/audit.toml`               | cargo-audit `--deny unsound` (per-PR CI + daily) | The same ids, plus ids only cargo-audit sees (`unsound`/informational, lockfile-only crates). Short comment pointing at deny.toml.                                                               |
| `osv-scanner.toml`                | osv-scanner (daily)                              | Every id from the two files above **and** every GHSA in `pnpm-workspace.yaml` `ignoreGhsas`, each with a one-line reason. osv-scanner honours none of the other files, so this is a pure mirror. |
| `SECURITY-EXCEPTIONS.md`          | people                                           | The npm rationale of record, one section per GHSA.                                                                                                                                               |

Two checks keep that honest, because hand-maintained mirrors drift —
2026-10-25's #1208 was four ids accepted in deny.toml + audit.toml that
never reached osv-scanner.toml, plus seven audit.toml entries for crates
that had long left the graph:

- **Drift gate** (`scripts/ci/check-advisory-ignores.py`, run in the
  per-PR `Rust — cargo deny / audit` job and the daily sweep): every id
  in deny.toml ∪ audit.toml ∪ `ignoreGhsas` is in osv-scanner.toml, and
  every id in osv-scanner.toml is backed by one of them. A PR that adds
  an ignore to one file without the mirror fails CI.
- **Stale check** (same script with `--deny-json` + `--advisory-db`, daily
  only): an accepted id that no longer matches any lockfile version —
  cargo-deny's `advisory-not-detected` for deny.toml ids, and the same
  patched/unaffected-range test over Cargo.lock for audit.toml-only ids —
  turns the tracking issue red as **stale ignores**. Stale means the
  upstream fix landed or the crate left the graph: delete the entry from
  every file, and act on its `Revisit when…` note (the pin that held the
  advisory in place can probably move now).

Adding an exception: write the full entry in deny.toml (or audit.toml for
an unsound/lockfile-only id, or `SECURITY-EXCEPTIONS.md` + `ignoreGhsas`
for npm), restate it in osv-scanner.toml, run the script, and say in the
PR what has to change upstream for the entry to go away.

## Adding a dependency

- Prefer crates/packages at `>=1.0` — everything below it needs a human for
  every minor.
- A new npm package with an install script must be listed in
  `pnpm-workspace.yaml` `allowBuilds` (currently every entry is `false`:
  no install script runs, prebuilt binaries only).
- Run `cargo deny check` locally; an unfamiliar licence fails CI.
