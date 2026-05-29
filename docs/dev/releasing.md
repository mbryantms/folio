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

Use the guard-railed recipe:

```sh
just release 0.7.2
```

It refuses to proceed unless the working tree is clean, you're on `main`, and
`main` is in sync with `origin`. It then runs the full check suite, verifies
`CHANGELOG.md` has a section for the version, stamps that section's date,
commits the stamp if needed, creates the annotated tag, and prints the exact
`git push` commands — it does **not** push for you (pushing the tag is the
irreversible, image-publishing step, so it stays a deliberate manual action).

### Before you run it

1. **Land all the work** for the release on `main`.
2. **Update `CHANGELOG.md`**: move items out of `## [Unreleased]` into a new
   `## [X.Y.Z] - YYYY-MM-DD` section (the date can be left as a placeholder;
   `just release` stamps today's date), and add the `compare` link at the
   bottom. Group entries under Added / Changed / Fixed / Removed.
3. Sanity-check the diff that will ship: `git diff vLAST..HEAD --stat`.

### Manual fallback

If you're not using the recipe, the steps are:

```sh
just check            # full suite must be green
# edit CHANGELOG.md: new section, dated, compare link
git commit -am "docs: changelog for vX.Y.Z"
git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin main
git push origin vX.Y.Z   # ← triggers the release workflow
```

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
