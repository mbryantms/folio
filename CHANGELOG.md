# Changelog

All notable changes to Folio are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (pre-1.0:
minor = features, patch = fixes/polish).

Versioning note: the crate/package manifests stay at `0.0.0` on purpose —
**the git tag is the version**. The running build reports it via
`COMIC_BUILD_TAG` (set from the tag at image-build time). See
[docs/dev/releasing.md](docs/dev/releasing.md) for the release ritual.

Releases before v0.7.2 are recorded only as Git tags + GitHub Releases;
this file starts at the first release that ships with a curated changelog.

## [Unreleased]

## [0.7.5] - 2026-05-29

### Fixed

- **`GET /libraries/{id}` 404'd when called with a UUID.** The endpoint
  resolved only by slug, but the fetch-metadata dialog holds the issue's
  `library_id` UUID — so the lookup missed, the library never loaded, and
  `metadata_writeback_enabled` read as false. That silently broke the
  apply→wait-for-rescan flow (the dialog closed onto a stale issue page).
  The read endpoint now accepts a slug **or** a UUID.

## [0.7.4] - 2026-05-29

### Fixed

- **Candidate cover images failed to load in the fetch-metadata view.** The
  service worker's cross-origin guard was a no-op (serwist's `defaultCache`
  registered a second fetch listener that still intercepted provider covers);
  the resulting opaque cross-origin response is incompatible with the
  document's `COEP: credentialless`, so the browser blocked the images
  (`NS_ERROR_INTERCEPTION_FAILED`). The SW now hands cross-origin requests to
  the browser's native loader. Existing clients pick up the fix on the next
  service-worker update (hard refresh / close all tabs).

## [0.7.3] - 2026-05-29

### Added

- **"Re-download missing variant covers" button** in the admin Metadata
  dashboard. Triggers the variant-cover backfill (previously API-only) to
  recover provider covers whose local file is missing, looping in batches
  and reporting any that can't be refetched (stale provider URL).

### Changed

- **Error and 404 pages rebuilt** to be theme-aware and on-brand, replacing
  the legacy top-bar shell. A shared `StatusScreen`/`StatusCard` now backs the
  404, the per-area error boundaries, a new root-level not-found, and a new
  `global-error` boundary that catches root-layout crashes.

### Fixed

- **Page title wrapped despite available space.** The page header now extends
  on one line (ellipsizing only when genuinely out of room), matching the
  reading-list header instead of breaking onto two lines.
- **Renaming a page left a dead sidebar link.** The left nav is rendered in the
  server layout, which soft navigation preserved — so its link kept pointing at
  the old slug and 404'd. Renames now refresh the layout so the link updates.

## [0.7.2] - 2026-05-29

### Added

- **Page-editor image adjustments.** The archive page editor can now apply
  non-destructive image transforms per page — brightness/contrast, levels
  clip, sharpen (unsharp mask), despeckle (median filter), and crop — with a
  live canvas preview and a draggable crop box. Transforms are applied at
  archive-rewrite time across CBZ/CBT/CBR, after rotation and before
  re-encode; pages needing no encode still stream-copy verbatim. Frontend and
  backend share an identical transform chain for preview/output parity.
- **Loading-skeleton framework, rebuilt per surface.** Each area now renders a
  shape-matched skeleton inside its real shell instead of one generic cover
  grid in the legacy auth shell: home rails, series detail (hero + stats +
  tabs + issue grid), bookmarks, collections, admin (header + tabs/table),
  and settings (form cards). The top-level fallback is now shell-agnostic.

### Fixed

- **Reader loading flash on iPad.** The reader inherited the library's
  light/cover-grid loading fallback, flashing white before the dark reader
  painted. It now has its own dark, reader-shaped skeleton driven by a shared
  `--reader-bg` token, so the background can't drift between skeleton and
  reader. The reader's server-side prefetches (`/progress`, `/auth/me`) now
  run concurrently, shortening time-to-reader.
- **Variant covers wiped by the nightly orphan sweep.** Downloaded provider
  covers live under `thumbs/issues/…`; the thumbnail orphan sweep read
  `issues` as an issue id and `remove_dir_all`'d the whole tree every night,
  leaving "cover unavailable" 404s and gray gallery boxes. The sweep now skips
  the reserved tree and reclaims only covers of genuinely inactive issues; the
  variant-cover backfill re-downloads rows whose file went missing.
- **Page rename navigated to a 404.** Renaming a custom page reallocates its
  slug, but the post-rename refresh re-rendered the stale `/pages/<old-slug>`
  URL and hit `notFound()`. The rename now navigates to the new slug when on
  the page's detail route. Long page titles also wrap instead of truncating.

### Removed

- Dropped the vestigial `metadata_run_candidate.dismissed_at` column.

[Unreleased]: https://github.com/mbryantms/folio/compare/v0.7.5...HEAD
[0.7.5]: https://github.com/mbryantms/folio/compare/v0.7.4...v0.7.5
[0.7.4]: https://github.com/mbryantms/folio/compare/v0.7.3...v0.7.4
[0.7.3]: https://github.com/mbryantms/folio/compare/v0.7.2...v0.7.3
[0.7.2]: https://github.com/mbryantms/folio/compare/v0.7.1...v0.7.2
