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
| **CI** | The automated checks GitHub runs on every PR (Rust build/clippy/tests, web lint/typecheck/tests, OpenAPI drift, audit-check, cargo-deny). The required ones gate the merge; the slow Docker/docs builds run but don't block. |
| **merge queue** | GitHub serializes queued PRs and tests each against the projected `main` before merging, so "branch out of date" never blocks you. Arm it with `gh pr merge --auto`. |
| **Renovate** | A bot that watches your dependencies and opens PRs to update them, every Monday. |
| **release-please** | A bot that maintains a "Release PR" from your conventional-commit titles (next version + `CHANGELOG.md`). Merging it tags `vX.Y.Z` and publishes. Irreversible. |
| **Release PR** | The auto-maintained PR release-please keeps open; merging it is how you cut a release. |

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
5. **Name it for the changelog.** Use a clear conventional-commit PR title —
   `feat(scope): user-facing summary`. That title (not a hand-edited
   `CHANGELOG.md`) becomes the release note via release-please.
6. **Open the PR.** Claude can do this for you:
   ```sh
   git push -u origin feat/<short-name>
   gh pr create --base main --fill
   ```
7. **Let CI run, then merge.** When the required checks are green, merge (squash
   is the tidy default — the squash title is what release-please reads). Delete
   the branch — GitHub offers a button, or `git branch -d feat/<short-name>`.

The feature now sits in `main` and is picked up by the next Release PR. It goes
live when you merge that (see §4).

> **Why a PR even when you're the only dev?** `main` is protected so CI is the
> gatekeeper — a PR is how the checks get a chance to catch a problem _before_
> it's in the shippable branch. (Admins _can_ force a push straight to `main`,
> but that skips the safety net — treat it as an emergency-only escape hatch,
> not the normal path.)

---

## 2) Adding a minor change (with Claude)

A bug fix, copy tweak, or small polish. **Same flow as a feature, just
lighter** — don't skip the branch + PR, because that's how CI gets to vet it.

1. `git checkout -b fix/<short-name> origin/main`
2. Make the change with Claude; run `/check`.
3. Give the PR a `fix(scope): …` title (or `chore:`/`docs:`/`refactor:` for
   no-user-impact changes — those are hidden from the changelog automatically).
4. `git push -u origin fix/<short-name> && gh pr create --base main --fill`
5. Merge when green; delete the branch.

Small fixes accumulate on `main` and all ship together when you merge the next
Release PR. There's no "small enough to skip the process"
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

Releases are driven by **release-please** — you don't stamp changelogs or push
tags by hand:

1. Land your work on `main` via normal PRs with conventional-commit titles
   (`feat(scope): …`, `fix(scope): …`). Those titles become the changelog.
2. A bot keeps a single **"Release PR"** up to date on every push to `main`
   (next version from the commits + regenerated `CHANGELOG.md`).
3. When you're ready to ship, **merge the Release PR.** That tags `vX.Y.Z` and
   publishes both signed images + the GitHub Release.

Pre-1.0: `feat:` → minor, `fix:` → patch. Want prose instead of one-line
entries? Edit `CHANGELOG.md` on the Release PR before merging. Full detail in
[releasing.md](releasing.md).

---

## 5) What's automated vs. manual

### Already automated — runs without you

- **Renovate** finds outdated deps and opens PRs every Monday (security fixes
  any time).
- **Safe dependency updates auto-merge** themselves once CI passes (stable
  patch/minor).
- **CI** runs on every PR (via the merge queue) and every push to `main`. The
  slow Docker image builds run but don't _gate_ merges; they build for real on
  release.
- **`:edge` Docker images** are built on every merge to `main`.
- **Version + changelog + release** — **release-please** maintains a "Release
  PR" (version + `CHANGELOG.md` from conventional commits); merging it tags
  `vX.Y.Z` and `release.yml` builds + signs both images and creates the GitHub
  Release.

### Still manual on purpose

- **Reviewing & merging gated dep PRs** (0.x, majors, security) — they can
  break things, so a human (with Claude's help) decides.
- **Merging the Release PR** — irreversible and publishes images, so it's a
  deliberate act (this is the one click that cuts a release).
- **Writing good commit titles** — they _are_ the changelog (edit the Release
  PR's `CHANGELOG.md` for prose when it matters).

> Rule of thumb: automate the **safe and repetitive** (green dep bumps, CI,
> image builds) and keep a human on the **risky and irreversible** (breaking
> upgrades, publishing a release).
