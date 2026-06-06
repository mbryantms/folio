# Dev workflow cheat sheet

A plain-language guide to getting changes into Folio: new features, small
fixes, and dependency updates — and which parts happen on their own. For the
release mechanics this references, see [releasing.md](releasing.md).

## The one mental model

Everything below is the same shape:

> **`main` is protected and always shippable. You never edit it directly.
> Every change rides in on a short-lived _branch_, gets reviewed as a _pull
> request_, and is only allowed in once the robots (CI) say it's green. A
> _release_ is a separate, deliberate step you take when you decide the
> accumulated changes are worth publishing.**

Keep that picture and the three flows are just variations on it.

### Words you'll see (plain English)

| Term | What it means here |
|------|--------------------|
| **branch** | A private copy of the code to work on, so `main` stays clean until you're done. Cheap and disposable. |
| **PR** (pull request) | A request to merge your branch into `main`. It's where CI runs and a human (or auto-merge) approves. |
| **CI** | The 8 automated checks GitHub runs on every PR: Rust build/clippy/tests, web lint/typecheck/tests, OpenAPI drift, audit-check, docs build, Docker build, security. `main` refuses changes until they pass. |
| **Renovate** | A bot that watches your dependencies and opens PRs to update them, every Monday. |
| **release / tag** | Publishing a version. Done by pushing a `vX.Y.Z` git tag, which builds the Docker images and creates a GitHub Release. Irreversible. |
| **`[Unreleased]`** | The top section of `CHANGELOG.md` where finished-but-not-yet-released changes pile up until the next release. |

---

## 1) Adding a new feature (with Claude)

A feature is user-facing and usually several files. Aim: land it on `main`
through a PR, then it ships in the next release.

1. **Start clean.** Begin from an up-to-date `main` on a new branch:
   ```sh
   git fetch origin && git checkout -b feat/<short-name> origin/main
   ```
2. **Build it with Claude.** Describe what you want; let Claude implement and
   iterate. For anything touching an external library (Next, sea-orm, axum…)
   Claude pulls current docs first.
3. **Prove it's green.** Run the project's check skill until clean:
   ```
   /check
   ```
   If you changed the API surface, regenerate the spec: `just openapi`.
4. **Optional but recommended:** ask Claude for a self-review with `/review`
   (convention check) or `/code-review` (bug hunt) before you open the PR.
5. **Write it down.** Add a bullet to the `## [Unreleased]` section of
   `CHANGELOG.md` under **Added**, in user-facing language.
6. **Open the PR.** Claude can do this for you:
   ```sh
   git push -u origin feat/<short-name>
   gh pr create --base main --fill
   ```
7. **Let CI run, then merge.** When all 8 checks are green, merge (squash is
   the tidy default). Delete the branch — GitHub offers a button, or
   `git branch -d feat/<short-name>`.

The feature now sits in `main` and in `[Unreleased]`. It goes live the next
time you cut a release (see §4).

> **Why a PR even when you're the only dev?** `main` is protected so CI is the
> gatekeeper — a PR is how the checks get a chance to catch a problem *before*
> it's in the shippable branch. (Admins *can* force a push straight to `main`,
> but that skips the safety net — treat it as an emergency-only escape hatch,
> not the normal path.)

---

## 2) Adding a minor change (with Claude)

A bug fix, copy tweak, or small polish. **Same flow as a feature, just
lighter** — don't skip the branch + PR, because that's how CI gets to vet it.

1. `git checkout -b fix/<short-name> origin/main`
2. Make the change with Claude; run `/check`.
3. Add a `CHANGELOG.md` `[Unreleased]` bullet under **Fixed** (skip only for
   pure internal/no-user-impact changes — those go under **Internal**).
4. `git push -u origin fix/<short-name> && gh pr create --base main --fill`
5. Merge when green; delete the branch.

Small fixes accumulate in `[Unreleased]` alongside features and all ship
together at the next release. There's no "small enough to skip the process"
threshold — the process is cheap, and skipping it is what bypasses CI.

---

## 3) Dependency updates (mostly hands-off)

You rarely start these — **Renovate does**. Every Monday before 6am UTC it
opens PRs for outdated packages (and security fixes *any* time). Your job is
only to handle the ones it's told not to merge by itself.

What happens automatically vs. what needs you:

| Renovate opens a PR to… | What happens | Your move |
|---|---|---|
| Bump a **stable** (non-`0.x`) package by patch/minor | **Auto-merges itself** once CI is green | Nothing |
| Do weekly **lockfile maintenance** | Auto-handled | Nothing |
| Bump a **`0.x`** package (any size) | Waits — labeled `zerover-review` | Review & merge (0.x can break on any bump) |
| Do a **major** version bump | Waits — labeled `major-review` | Review; may need code changes |
| Bump a **specially-gated** package (`eslint`, `@types/node`, the `rust-crypto` group) | Waits — labeled `gated-major` / `needs-migration-review` | Review carefully; these have known constraints noted in `renovate.json` |
| Fix a **security** vulnerability | Opens immediately, labeled `security` | Prioritize |

**Where to see it all:** Renovate keeps a single **Dependency Dashboard**
issue in the repo — that's your control panel for what's pending, paused, or
waiting on you.

**When a gated PR needs real work** (a major bump that breaks the build), hand
it to Claude:
```sh
gh pr checkout <pr-number>      # jump onto Renovate's branch
# ask Claude: "upgrade to the new major and fix the breakage"
/check                          # confirm green
git push                        # updates the same PR; CI re-runs
```
Then merge once green. Related breaking bumps are pre-grouped into one PR
(e.g. `radix-ui`, `tanstack`, `rust-crypto`) so you migrate them together.

---

## 4) Cutting a release (the deliberate step)

Features and fixes pile up in `[Unreleased]`. When you decide to publish:

1. Make sure everything's merged to `main` and `main` is clean.
2. In `CHANGELOG.md`, move the `[Unreleased]` items into a new
   `## [X.Y.Z]` section (pre-1.0: **minor** = new features, **patch** =
   fixes only) and add the compare link at the bottom.
3. From a worktree that can run the full suite:
   ```sh
   just release X.Y.Z       # checks guards, runs all tests, stamps date, tags
   git push origin main
   git push origin vX.Y.Z   # ← this publishes: images + GitHub Release
   ```

`just release` does **not** push for you — the tag push is the irreversible,
image-publishing action, so it stays a manual decision. Full detail in
[releasing.md](releasing.md).

---

## 5) What's automated vs. manual

### Already automated — runs without you

- **Renovate** finds outdated deps and opens PRs every Monday (security fixes
  any time).
- **Safe dependency updates auto-merge** themselves once CI passes (stable
  patch/minor).
- **CI** runs all 8 checks on every PR and every push to `main`.
- **`:edge` Docker images** are built on every merge to `main`.
- **Releasing** — pushing a `vX.Y.Z` tag builds + signs both images and writes
  the GitHub Release notes from your changelog.
- **Changelog date stamping** during `just release`.

### Still manual on purpose

- **Reviewing & merging gated dep PRs** (0.x, majors, security) — they can
  break things, so a human (with Claude's help) decides.
- **The release tag push** — irreversible and publishes images, so it's a
  deliberate act.
- **Writing changelog entries** — these are human-facing highlights.

### Could be automated later (options, not requirements)

- **Auto-merge your *own* feature/fix PRs** once green (GitHub's built-in
  "auto-merge" toggle, or `gh pr merge --auto`) — removes the "remember to
  click merge" step while keeping CI as the gate.
- **A scheduled Claude routine** (see the `/schedule` skill) to triage the
  Renovate Dependency Dashboard weekly — e.g. attempt the gated majors on a
  branch and report what it could and couldn't fix.
- **Automated version + changelog** (release-please style) that proposes the
  next version from commit messages. Folio deliberately keeps the *tag push*
  manual, but the changelog/version prep around it could be drafted for you.

> Rule of thumb: automate the **safe and repetitive** (green dep bumps, CI,
> image builds) and keep a human on the **risky and irreversible** (breaking
> upgrades, publishing a release).
