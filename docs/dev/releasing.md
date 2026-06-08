# Releasing Folio

Folio ships as two container images — `ghcr.io/<owner>/folio` (Rust server)
and `ghcr.io/<owner>/folio-web` (Next.js front) — published by
[`.github/workflows/release.yml`](../../.github/workflows/release.yml).

Releases are driven by **release-please**: you don't stamp changelogs or push
tags by hand. Merge normal PRs with conventional-commit titles; a bot keeps a
single "Release PR" up to date; merging that PR publishes the release.

## The version is the Git tag

The crate and package manifests deliberately stay at `version = "0.0.0"`.
**A release is the annotated `vMAJOR.MINOR.PATCH` tag** (release-please creates
it on merge). There is no manifest version to bump — don't add one; it would
contradict this ritual and drift from the tag. release-please tracks the
version only in [`.release-please-manifest.json`](../../.release-please-manifest.json),
never in a source manifest.

At image-build time the workflow passes the tag into the build as
`COMIC_BUILD_TAG`, and the server surfaces it at `GET /admin/server/info`
(see [`server_info.rs`](../../crates/server/src/api/server_info.rs)). A dev
build with no tag reports `dev`.

Pre-1.0 semver (encoded in [`release-please-config.json`](../../release-please-config.json)
via `bump-minor-pre-major`):

- **patch** (`v0.7.1` → `v0.7.2`) — `fix:` commits, polish, internal changes.
- **minor** (`v0.7.x` → `v0.8.0`) — `feat:` commits (user-facing features).
- **major** stays `0` until 1.0; reserve `feat!:` / `BREAKING CHANGE` for then.

## Cutting a release

1. **Merge your work to `main` via normal PRs**, with conventional-commit
   titles (`feat(scope): …`, `fix(scope): …`). These titles become the
   changelog — write them for humans.
2. **release-please maintains a "Release PR"** automatically on every push to
   `main` (the [`release-please`](../../.github/workflows/release-please.yml)
   workflow). It computes the next version from the commits since the last
   release and regenerates the top of `CHANGELOG.md`.
3. **When you're ready to ship, merge the Release PR.** That is the release
   gate. Merging it creates the `vX.Y.Z` tag + GitHub Release, which triggers
   `release.yml` to build, sign, and publish the images.

That's it — no manual changelog stamping, no manual tag push.

### Want prose, not just commit lines?

The Release PR is a normal PR: edit `CHANGELOG.md` directly on its branch
(`release-please--branches--main`) to add narrative under the generated
bullets before merging. release-please preserves manual edits.

### What's shown vs hidden in the changelog

Only `feat`/`fix`/`perf`/`revert` commits appear in release notes (mapped to
**Added** / **Fixed** / **Changed**). `chore`, `ci`, `docs`, `style`,
`refactor`, `test`, `build` are hidden — so the Renovate `chore(deps)` stream
doesn't drown the notes. Adjust `changelog-sections` in
[`release-please-config.json`](../../release-please-config.json) to retune.

### The release token

The [`release-please`](../../.github/workflows/release-please.yml) workflow uses
the `FOLIO_RELEASE_TOKEN` secret (a fine-grained PAT / GitHub App token with
**Contents: write** + **Pull requests: write**) rather than the built-in
`GITHUB_TOKEN`, for two reasons: (a) so the Release PR triggers the required CI
checks, and (b) so the **tag** release-please creates triggers `release.yml`
(tags pushed by the default Actions token don't start downstream workflows).

## What a tag push triggers

Merging the Release PR pushes `vX.Y.Z`, which runs `release.yml`:

1. Builds + pushes both images to GHCR, tagged with the immutable `vX.Y.Z`,
   the floating `:latest` (non-prerelease only), plus `:sha-<short>`.
2. Attaches an SPDX SBOM and a keyless Sigstore (cosign) signature to each
   manifest.
3. **Appends** the pull-from-GHCR + `cosign verify` snippet to the GitHub
   Release that release-please already created (the `release-notes` job uses
   `append_body`, so it never overwrites the generated notes).

A push to `main` (no tag) publishes only the floating `:edge` / `:sha-<short>`
images — not a release.

## Rollback

Images are immutable per tag; you can't overwrite a published `vX.Y.Z`.
To "roll back" a bad release, land the fix and merge the next Release PR (a new
patch). A pushed tag can be deleted (`git push origin :vX.Y.Z`) but any
already-pulled images persist — prefer rolling forward.
