# Dependency management — observe, don't operate

Policy for how third-party code enters this repo. The goal is that
updates flow in on their own wherever a machine can verify them, with a
reviewable trail for every change, and that a human is asked for exactly
two things: decisions automation cannot make (majors, breaking 0.x
minors, infrastructure lines) and failures automation cannot fix.

Last reviewed: 2026-11-08.

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

| Label                             | What                                                                                                                                                                                | Why a machine can't decide                                                                                                                                        |
| --------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `major-review`                    | Any major                                                                                                                                                                           | Migration work by definition; release notes are in the PR body.                                                                                                   |
| `zerover-review`                  | `0.x` **minor**                                                                                                                                                                     | The breaking step for 0.x under both ecosystems' semver rules.                                                                                                    |
| `gated-major`                     | eslint 10, `typescript` major, `@types/node` major                                                                                                                                  | Known upstream blockers, documented in the rule description.                                                                                                      |
| `needs-migration-review`          | react-hook-form, `@hookform/resolvers`, openapi-typescript, `@tanstack/react-query`                                                                                                 | History of breaking-within-minor; codegen shim must move with openapi-typescript.                                                                                 |
| `coordinated-bump`                | reqwest/oauth2/openidconnect, sea-orm/sqlx, apalis/redis, utoipa/utoipa-axum, testcontainers, RustCrypto                                                                            | Must co-resolve; grouped so they never arrive alone.                                                                                                              |
| `infra-review`                    | A **tag** change of postgres/redis/dex/alpine/curl or a Dockerfile base line                                                                                                        | A postgres major needs a data-dir plan; dev and CI must stay on the tested major.                                                                                 |
| _(none; branch `shadcn/refresh`)_ | The weekly shadcn registry pull (`.github/workflows/shadcn-refresh.yml`, Mondays 06:41 UTC): the upstream diff of every file under `web/components/ui/`, which is vendored verbatim | A component change is visual; CI proves it compiles and passes the smoke run, a human eyeballs it. Red = an upstream API change needs a wrapper or call-site fix. |

TypeScript runs side by side. In `web/package.json`, `@typescript/native` is
`typescript@7` and owns the `tsc` binary that `pnpm run typecheck` uses, and
`typescript` is an alias of `@typescript/typescript6`, so typescript-eslint,
openapi-typescript and `next build` keep the TS 6 compiler API that TS 7.0
does not ship. Replace both with one plain `typescript` entry once those
tools support the TS 7.1 API.

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

| Signal                                          | Source                                              | When                                                                                                                                                                                                                                                                                                                       |
| ----------------------------------------------- | --------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Issue **"Dependency health: attention needed"** | `.github/workflows/advisories.yml`, daily 06:17 UTC | Any of: new RUSTSEC advisory, high npm advisory, OSV hit, HIGH/CRITICAL fixable CVE in the published images, a Renovate PR that should have auto-merged but is red / >7 days old, or a silent Renovate runner (below). Created once, body rewritten daily, commented when the failing set changes, auto-closed when green. |
| Issue **Dependency Dashboard** (#352)           | Renovate                                            | Single pane: pending (age-gated), open, held, blocked-by-closed, OSV summary. No notification; look when you want.                                                                                                                                                                                                         |
| Release PR (`chore: release vX.Y.Z`)            | release-please                                      | Every dependency merge lands in the next release's "Dependencies" section.                                                                                                                                                                                                                                                 |
| CI step summary "Image CVE scan"                | `docker-smoke` job on every PR                      | Informational table; never blocks.                                                                                                                                                                                                                                                                                         |

**Silent runner.** Renovate runs on a self-hosted LAN container (Proxmox
CT 135) that no GitHub Action can reach, and PR-based checks cannot tell a
dead runner from a quiet week: a host reboot on 2026-11-01 stopped it for
7 days with the sweep green. So the sweep reads the two marks the runner
leaves on GitHub, and goes red as **Renovate runner looks dead** when
both the Dashboard's last edit and the newest `renovate/*` PR are over
**4 days** old. That line clears the morning after the runner is back.

Why both: new PRs stop whenever `prConcurrentLimit` is full, and a Dashboard
with nothing to change is not edited. Why 4 days, replaying every sweep since
2026-07-03: the longest healthy silence was 3.1 days (2026-08-19..22,
force-pushes prove the runner was up; no Dashboard history that old, so it
is assumed silent), and the two outages (2026-11-01..08, and 2026-07-29..08-07,
inferred from no activity of any kind followed by 15 PRs in four minutes)
would have gone red on 11-06 and 08-03. What it cannot see: an outage under 4 days (the 34-hour one of
2026-09-13), or a dead runner while someone ticks a Dashboard box, since a
tick is a body edit too and restarts the clock. A runner that is up but
failing before it touches the repo looks the same as a stopped one, so check
`journalctl -u renovate.service` as well as `pct status 135`.

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
| Node                                                 | `engines.node >=24` (floor), `.nvmrc`, `node-version: 26` in workflows, `node:26-…` + distroless `nodejs<major>` in `web/Dockerfile` (bump together) |
| OCR model files (not Renovate-managed)               | `crates/server/src/ocr/models.rs` (Hugging Face commit + SHA-256 per file, checked at runtime); update steps in [ocr.md](ocr.md#pinned-model-files) |
| Leptonica + Tesseract sources and tessdata compiled/baked into the image (not Renovate-managed) | `scripts/docker/tesseract-inputs.sh` (URL + commit + SHA-256 per file), run by the `Dockerfile` `deps` stage; see "tesseract-rs build inputs" below |
| `hf-hub` (Renovate updates disabled)                 | A direct dependency only so `ocr::models` uses the same cache path as `comic-text-detector` and `manga-ocr`, which holds while Cargo resolves one `hf-hub` for all three. Bump it by hand, together with those crates; a test in `ocr::models` fails if `Cargo.lock` holds two copies |

## tesseract-rs build inputs

`tesseract-rs` (feature `build-tesseract`) compiles Leptonica and
Tesseract from source in its build script. The script downloads, with no
checksum, the two GitHub tag archives named in its `build.rs`
(0.4.0: Leptonica `1.87.0`, Tesseract `5.5.2`) and `eng` + `tur`
traineddata from the moving `main` branch of `tessdata_best`, into
`~/.tesseract-rs/`. It skips each download when the target already
exists: `third_party/leptonica/` and `third_party/tesseract/` (a
directory check) and `tessdata/<lang>.traineddata` (a file check).

The image build uses that: `scripts/docker/tesseract-inputs.sh` runs in
the `Dockerfile` `deps` stage before `cargo chef cook` and

- **`fetch`** downloads the same two tag archives and the two
  traineddata files from a pinned `tessdata_best` commit, checks every
  file against its pinned SHA-256 (a mismatch fails the build before
  anything is unpacked or compiled), unpacks each archive the way the
  build script does (its single top-level folder becomes
  `third_party/<name>/`) and records a per-file digest manifest of what
  it placed;
- **`check-crate`** runs after the crate sources are fetched (`cargo
  chef cook --no-build` + `cargo fetch`, nothing compiled) and fails if
  the registry's tesseract-rs `build.rs` contains any https URL other
  than the pinned archives and the tessdata base, a language list other
  than `eng` + `tur`, or no longer has the skip-if-present checks;
- **`verify`** runs after `cargo chef cook` and again after the final
  `cargo build`: every placed source file still has its manifest digest
  (except `src/environ.h` and `prog/makefile.static`, which the build
  script edits in place), and `tessdata/` holds exactly the two pinned
  files, which the runtime image copies to `/app/tessdata`.

What `verify` proves: the build script compiled from the verified trees
and the image carries only the verified tessdata. If the script had
fallen back to downloading, it would have had to replace a placed tree
or add a file, and the check would fail. It does not prove that no
network request was made, and it cannot cover the two edited Leptonica
files beyond their presence. The image build cannot run without
network access (cargo fetches crates and `ort-sys` downloads its
SHA-256-checked ONNX Runtime binaries), so the build is not network
isolated.

**When Renovate bumps `tesseract-rs`** and the new `build.rs` names
other versions, `check-crate` fails the image build. To move the pins:

1. Copy the new archive URLs from the crate's `build.rs`
   (`~/.cargo/registry/src/*/tesseract-rs-<ver>/build.rs`, the
   `LEPTONICA_URL` / `TESSERACT_URL` constants) and re-read its
   skip-if-present logic.
2. `curl -fsSL -o l.zip <leptonica url> && sha256sum l.zip` (same for
   Tesseract). Download each twice a few minutes apart and compare the
   digests: GitHub tag archives are expected to be stable, but a changed
   digest means investigate before pinning.
3. Record the commit each tag points at: `git ls-remote <repo>
   'refs/tags/<tag>*'` (for an annotated tag the `^{}` line is the
   commit); `unzip -z l.zip` prints the same commit.
4. Update `LEPTONICA_URL` / `LEPTONICA_SHA256` / `TESSERACT_URL` /
   `TESSERACT_SHA256` and their comments in
   `scripts/docker/tesseract-inputs.sh`, and the version line in the
   `Dockerfile` `deps` comment.

**Moving tessdata** (only if wanted; `tessdata_best` `main` has not
changed since 2024-03-09): `git ls-remote
https://github.com/tesseract-ocr/tessdata_best refs/heads/main` gives
the commit; `curl -fsSL -O
https://github.com/tesseract-ocr/tessdata_best/raw/<commit>/eng.traineddata`
(and `tur`) then `sha256sum` gives the digests for `TESSDATA_COMMIT`,
`ENG_SHA256` and `TUR_SHA256`.

Renovate does not update any of this. The archive versions are dictated
by the crate's `build.rs`, not chosen independently, and Renovate cannot
recompute a file's SHA-256 for a regex-managed URL, so a bot PR could
only move a reference and leave its digest stale.

**Local builds are not covered.** A `cargo build` outside Docker runs
upstream's build script as is: on the first build it downloads the four
files unchecked into `~/.tesseract-rs/` and reuses them afterwards
(the directory survives `cargo clean`). The script is upstream's and
has no hook for a checksum. A developer who wants the verified bytes
locally can run the same steps by hand (the digests are in
`scripts/docker/tesseract-inputs.sh`) or compare an existing
`~/.tesseract-rs/tessdata/*.traineddata` with `sha256sum`; the image
build is the verified path. The CI jobs that compile the `server` crate
on a runner (`ci.yml`: `rust-clippy` incl. its doctests, the `rust-test`
shards, `sidecar-interop`, `openapi-drift`; `perf-plans.yml`:
`perf-explain.sh`) download these inputs unverified too; none of them
publishes a binary.

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

A Rust advisory the RustSec db has no id for yet — GitHub-only, as the
2026-10-29 `zip` batch (GHSA-m665-v7pm-7grj and four siblings) and
`block-buffer` GHSA-qwgh-2vcv-g2f7 were — cannot go in either `ignore`
array: an id the db does not know only warns as `advisory-not-detected`
and reads as stale, while osv-scanner, which ingests GitHub's advisories
directly, keeps reporting it. Write the rationale in deny.toml as usual
and put the GHSA on a comment line `# osv-only: GHSA-…` under it, plus
the osv-scanner.toml entry; the drift gate backs that entry from the
`osv-only:` line, and the daily stale check reports the day the RustSec
db lists the GHSA as an alias, which is when the id moves into the
`ignore` arrays under its new RUSTSEC id.

## Adding a dependency

- Prefer crates/packages at `>=1.0` — everything below it needs a human for
  every minor.
- A new npm package with an install script must be listed in
  `pnpm-workspace.yaml` `allowBuilds` (currently every entry is `false`:
  no install script runs, prebuilt binaries only).
- Run `cargo deny check` locally; an unfamiliar licence fails CI.
