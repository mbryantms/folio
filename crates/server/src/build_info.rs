//! Build-time version + `User-Agent` strings — the single source of truth.
//!
//! `Cargo.toml` is pinned at `version = "0.0.0"` (workspace `publish = false`;
//! the release ritual uses git tags only), so `CARGO_PKG_VERSION` is *not* the
//! real version. The real one is the `git describe --tags --always --dirty`
//! value that [`build.rs`] captures into the `COMIC_BUILD_TAG` rustc-env
//! (falling back to `"dev"` when the build couldn't reach git, e.g. a Docker
//! build without `.git`). Everything that reports a version — the startup log,
//! `/healthz` + `/readyz`, `/admin/server/info`, and outbound HTTP — reads it
//! from here so they can't drift apart again.

/// Human-readable build version, e.g. `v0.7.17`, `v0.7.17-7-gabcd123-dirty`,
/// or `dev`. Same value the `/admin/server` build card shows.
pub const VERSION: &str = env!("COMIC_BUILD_TAG");

/// Default outbound `User-Agent` for Folio's HTTP clients.
pub const USER_AGENT: &str = concat!("Folio/", env!("COMIC_BUILD_TAG"));

/// `User-Agent` for metadata-provider fetches (ComicVine / Metron).
pub const USER_AGENT_METADATA: &str =
    concat!("Folio/", env!("COMIC_BUILD_TAG"), " (+metadata-fetcher)");

/// `User-Agent` for cover-image fetches.
pub const USER_AGENT_COVER: &str = concat!("Folio/", env!("COMIC_BUILD_TAG"), " (+cover-fetch)");
