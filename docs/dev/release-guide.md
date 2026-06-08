# How to push a release

A plain-language, start-to-finish guide for shipping a new version of Folio.
No prior context needed. If you want the deeper mechanics, see
[releasing.md](releasing.md); for the day-to-day change workflow this builds
on, see [workflow-cheatsheet.md](workflow-cheatsheet.md).

## The 30-second version

1. Get every change you want to ship merged into `main` (each via its own PR).
2. Make sure `CHANGELOG.md` has good notes under `## [Unreleased]`.
3. Go to **GitHub → Actions → prepare release → Run workflow**, type the new
   version (e.g. `0.9.6`), and click the green button.
4. Wait. The robots stamp the changelog, tag the release, and publish the
   Docker images and GitHub Release for you.
5. Check the **Releases** page to confirm it shipped.

That's the whole thing. The rest of this doc explains each step.

---

## Words you'll see

| Term | Plain English |
|------|---------------|
| **`main`** | The protected, always-shippable copy of the code. You never edit it directly. |
| **branch** | A cheap, throwaway copy of the code where you make a change before it's allowed into `main`. |
| **PR** (pull request) | The request to merge your branch into `main`. It's where CI runs and the change gets approved. |
| **CI** | The automated checks GitHub runs on every PR. `main` refuses anything that isn't green. |
| **`[Unreleased]`** | The top section of `CHANGELOG.md` where finished-but-not-yet-published changes pile up. |
| **release / tag** | Publishing a version as `vX.Y.Z`. Builds the Docker images and creates a GitHub Release. **Irreversible.** |

---

## Step 1 — How to commit changes (so they're ready to release)

Releases don't start at release time — they start with how each change lands.
**Every change rides in on a branch and a PR.** There's no "too small to bother"
threshold; the PR is how CI gets to vet the change before it reaches the
shippable branch.

For each feature or fix:

1. **Start from a fresh `main`** on a new branch:
   ```sh
   git fetch origin && git checkout -b feat/<short-name> origin/main
   # use fix/<short-name> for a bug fix, chore/<short-name> for internal work
   ```
2. **Make the change.** Run the project's checks until clean:
   ```
   /check
   ```
   If you touched the API surface, also run `just openapi`.
3. **Write a changelog line.** Add one bullet to the `## [Unreleased]` section
   of `CHANGELOG.md`, in user-facing language, under the right heading:
   - **Added** — new features
   - **Changed** — behavior changes to existing features
   - **Fixed** — bug fixes
   - **Removed** — things taken out
   - **Internal** — no user impact (tooling, CI, refactors)
4. **Open the PR and let CI run:**
   ```sh
   git push -u origin feat/<short-name>
   gh pr create --base main --fill
   ```
5. **Merge when green** (squash is the tidy default) and delete the branch.

Repeat for every change. They all accumulate under `[Unreleased]` and ship
together at the next release.

> **Why bother with a PR when you're the only dev?** Because `main` is
> protected and CI is the gatekeeper. The PR is what gives the checks a chance
> to catch a problem *before* it's in the shippable branch.

---

## Step 2 — Prep for the release

Before you push the button, three quick checks:

1. **Everything's merged.** All the work you want in this release is on `main`.
   Anything still in an open PR will miss the release.

2. **The changelog reads well.** Open `CHANGELOG.md` and look at the
   `## [Unreleased]` section. This is exactly what becomes the release notes,
   so:
   - Every shipped change has a bullet.
   - Bullets are grouped under Added / Changed / Fixed / Removed / Internal.
   - The language is for *users*, not commit messages.

   The Unreleased section must **not** be empty — the automation refuses to run
   if there's nothing to release.

3. **Pick the version number.** Folio is pre-1.0, so:
   - **Minor** bump (`0.9.x` → `0.10.0`) when the release contains new features.
   - **Patch** bump (`0.9.5` → `0.9.6`) when it's only fixes / internal work.

   You don't need a leading `v` — just the number, e.g. `0.9.6`.

> Optional sanity check — see exactly what will ship since the last release:
> ```sh
> git fetch origin --tags
> git diff $(git describe --tags --abbrev=0 origin/main)..origin/main --stat
> ```

---

## Step 3 — Push the release

You have one job: start the **prepare release** workflow. Two ways to do it.

### Option A — the button (easiest)

1. Open the repo on GitHub.
2. **Actions** tab → **prepare release** in the left sidebar.
3. Click **Run workflow** (top right).
4. Type the version (e.g. `0.9.6`) and click the green **Run workflow** button.

### Option B — the command line

```sh
gh workflow run prepare-release.yml -f version=0.9.6
```

Optional inputs (you almost never need these — sensible defaults are used):

```sh
gh workflow run prepare-release.yml \
  -f version=0.9.6 \
  -f previous_tag=v0.9.5 \      # defaults to the latest v* tag
  -f release_date=2026-06-07    # defaults to today (UTC)
```

### What happens after you click (all automatic)

You don't do anything for these — just watch:

1. **Validates** the version and that the tag doesn't already exist.
2. **Stamps the changelog** — moves your `[Unreleased]` notes into a dated
   `## [X.Y.Z] - YYYY-MM-DD` section and updates the compare links.
3. **Opens a changelog PR** and turns on auto-merge.
4. **Waits** for that PR to pass CI and land on `main`.
5. **Waits** for `main`'s checks to go green on the merged commit.
6. **Creates and pushes the `vX.Y.Z` tag.**
7. **Triggers the release build** — which builds and signs both Docker images,
   attaches SBOMs, and creates the GitHub Release from your changelog notes.

To follow along from the terminal:

```sh
gh run watch $(gh run list --workflow prepare-release.yml --limit 1 \
  --json databaseId --jq '.[0].databaseId')
```

> **One-time setup:** the workflow needs a repository secret named
> `FOLIO_RELEASE_TOKEN` (a fine-grained PAT or GitHub App token with
> **Contents: write** and **Pull requests: write**). If it's missing, the run
> fails immediately with a message telling you so. It's needed because a PR
> opened by GitHub's default token can't trigger the required checks, so
> auto-merge would hang forever. This is already configured for the repo — you
> only touch it if the token expires.

---

## Step 4 — Follow-up

Once the workflow finishes green:

1. **Confirm the GitHub Release exists.** Repo → **Releases** → you should see
   `vX.Y.Z` with the notes pulled from your changelog.
2. **Confirm the images published.** Repo → **Packages** (or the
   `ghcr.io/<owner>/folio` and `…/folio-web` package pages) show the new
   `X.Y.Z` tag alongside `latest`.
3. **Refresh your local `main`** so it includes the auto-merged changelog
   commit:
   ```sh
   git fetch origin --tags
   ```
   (If you keep a local `main` checked out: `git checkout main && git pull`.)
4. **Nothing to push by hand.** Unlike the old flow, there's no manual tag push
   — the workflow already did it.

### If something goes wrong

- **"Set a FOLIO_RELEASE_TOKEN secret…"** → the token is missing or expired.
  Add/refresh it in repo **Settings → Secrets and variables → Actions**, then
  re-run.
- **"Unreleased section is empty"** → add changelog notes under
  `## [Unreleased]`, merge that via a PR, then re-run.
- **"tag vX.Y.Z already exists"** → that version was already released; pick the
  next number.
- **The changelog PR or `main` checks fail** → the workflow stops before
  tagging, so nothing was published. Fix the failing check on `main` (via a
  normal PR), then re-run **prepare release**.

Because the tag (the irreversible, publishing step) only happens *after* the
changelog PR and `main` checks are green, a failed run never leaves a
half-published release behind.

---

## At a glance

| You do | The robots do |
|--------|---------------|
| Land each change via a branch + PR | Run CI on every PR |
| Write changelog notes under `[Unreleased]` | — |
| Pick the version number | — |
| Click **Run workflow** (or `gh workflow run`) | Stamp changelog, open + merge its PR, tag, build & sign images, write the GitHub Release |
| Confirm the Release + images, pull `main` | — |
