# Releasing Folio

Folio ships as two container images — `ghcr.io/<owner>/folio` (Rust server)
and `ghcr.io/<owner>/folio-web` (Next.js front) — published by
[`.github/workflows/release.yml`](../../.github/workflows/release.yml).

## The version is the Git tag

The crate and package manifests deliberately stay at `version = "0.0.0"`.
**A release is created by pushing an annotated `vMAJOR.MINOR.PATCH` tag.**
There is no manifest version to bump — don't add one; it would contradict
this ritual and drift from the tag.

At image-build time the workflow passes the tag into the build as
`COMIC_BUILD_TAG`, and the server surfaces it at `GET /admin/server/info`
(see [`server_info.rs`](../../crates/server/src/api/server_info.rs)). A dev
build with no tag reports `dev`.

Pre-1.0 semver convention:

- **patch** (`v0.7.1` → `v0.7.2`) — bug fixes, polish, internal changes.
- **minor** (`v0.7.x` → `v0.8.0`) — user-facing features.
- **major** stays `0` until the 1.0 stabilization.

## What a tag push triggers

Pushing `vX.Y.Z` runs `release.yml`, which:

1. Builds + pushes both images to GHCR, tagged with the immutable
   `vX.Y.Z`, the floating `:latest` (non-prerelease only), plus
   `:sha-<short>`.
2. Attaches an SPDX SBOM and a keyless Sigstore (cosign) signature to each
   manifest.
3. Creates a GitHub Release whose body is the **`CHANGELOG.md` section for
   this version** followed by GitHub's auto-generated commit notes and the
   pull-from-GHCR + `cosign verify` snippets.

A push to `main` (no tag) publishes only the floating `:edge` / `:sha-<short>`
images — not a release.

## Cutting a release

Use the automated workflow:

1. Make sure `CHANGELOG.md` has useful notes under `## [Unreleased]`.
2. Run **Actions → prepare release** with the next SemVer version
   (`0.7.2`, no leading `v`).

The workflow stamps the changelog into a dated `## [X.Y.Z] - YYYY-MM-DD`
section, opens a `docs: changelog for vX.Y.Z` PR, enables auto-merge, waits for
the PR to land on protected `main`, waits for `main` checks, creates the
annotated tag, then dispatches `release.yml` at that tag. The release workflow
builds/pushes the images, signs them, attaches SBOMs, and creates the GitHub
Release.

The workflow requires a repository secret named `FOLIO_RELEASE_TOKEN`: a
fine-grained PAT or GitHub App installation token with **Contents: write** and
**Pull requests: write**. The built-in `GITHUB_TOKEN` is used only for the final
tag + workflow dispatch; it is not used for the changelog PR because events
created by `GITHUB_TOKEN` do not trigger the required PR checks.

### Before you run it

1. **Land all the work** for the release on `main` (via PRs / the merge queue).
2. **Write the curated changelog notes under `## [Unreleased]`.** The
   automation moves the whole Unreleased block into the release section, so keep
   it grouped under Added / Changed / Fixed / Removed.
3. Optional but useful: sanity-check the diff that will ship:
   `git diff vLAST..origin/main --stat`.

### Manual fallback

If you're not using the workflow, the steps are:

```sh
# 1. Changelog via a PR — main is protected, so no direct push:
git checkout -b docs/changelog-vX.Y.Z
# edit CHANGELOG.md: new dated [X.Y.Z] section + compare link
git commit -am "docs: changelog for vX.Y.Z"
git push -u origin docs/changelog-vX.Y.Z
gh pr create --base main --fill        # then merge it via the queue

# 2. From an up-to-date main, tag and publish:
git checkout main && git pull
just check                             # full suite must be green
git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin vX.Y.Z                 # ← the only direct push; triggers release
```

Branch protection covers `main`, not tags, so `git push origin vX.Y.Z` is the
one push allowed directly — and it's what publishes the release.

## Good release notes

Notes are assembled from two sources, so both matter:

- **`CHANGELOG.md`** — the curated, human-written highlights. This is the
  top of the GitHub Release body. Keep entries user-facing and grouped.
- **Commit messages** — GitHub auto-appends a commit/PR list. Write
  conventional-ish subjects (`feat(scope): …`, `fix(scope): …`) so the
  auto-generated portion reads well.

## Rollback

Images are immutable per tag; you can't overwrite a published `vX.Y.Z`.
To "roll back" a bad release, cut a new patch tag with the fix and move
deployments to it. A pushed tag can be deleted (`git push origin :vX.Y.Z`)
but any already-pulled images persist — prefer rolling forward.
