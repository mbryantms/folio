//! `GET /admin/server/latest-release` — upstream-release lookup with a
//! 1-hour server-side cache.
//!
//! Surfaces "v0.1.9 available" in the `/admin/server` build card so
//! self-hosters don't have to check GitHub manually. The HTTP fetch
//! runs once per server-process per hour regardless of how many admins
//! poll; failures cache as `None` so a flaky GitHub doesn't get
//! hammered on every request.
//!
//! Disabled paths (return `204 No Content`):
//!   - Runtime setting `updates.check_upstream_releases` is `false`.
//!   - `COMIC_BUILD_REPO_URL` is empty or non-GitHub. (We only know
//!     how to talk to GitHub's release API; gitlab / forgejo have
//!     different shapes. Air-gapped operators get the same code path
//!     since their builds typically don't have a public repo URL.)
//!   - Last fetch errored (cached as `None` for the TTL window).
//!
//! Privacy: no user data leaves the server. The fetch is a plain
//! `GET https://api.github.com/repos/{owner}/{repo}/releases/latest`
//! with a 5s timeout. Operators who want zero outbound traffic flip
//! the toggle off via `/admin/server`.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::auth::RequireAdmin;
use crate::state::AppState;

/// In-memory cache slot for the latest-release lookup. `fetched_at`
/// is `None` before the first fetch; on every call the handler checks
/// staleness against `TTL`.
#[derive(Default)]
pub struct ReleaseCache {
    pub fetched_at: Option<Instant>,
    /// `Some(_)` on a successful fetch, `None` when the last fetch
    /// errored — cached identically so we don't retry on every poll.
    pub value: Option<LatestReleaseView>,
}

const TTL: Duration = Duration::from_secs(3600);
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LatestReleaseView {
    /// Release tag (e.g. `"v0.1.9"`).
    pub tag: String,
    /// Browse URL on the host's release page.
    pub html_url: String,
    /// Publication timestamp from GitHub (RFC 3339).
    pub published_at: String,
}

/// Minimal subset of the GitHub release API payload we care about.
/// Field names match the API; serde maps them onto our view.
#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    published_at: String,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/admin/server/latest-release", get(latest_release))
}

#[utoipa::path(
    get,
    path = "/admin/server/latest-release",
    responses(
        (status = 200, body = LatestReleaseView),
        (status = 204, description = "update check disabled, repo not on GitHub, or last fetch errored"),
        (status = 403, description = "admin only"),
    )
)]
pub async fn latest_release(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let cfg = app.cfg();
    if !cfg.check_upstream_releases {
        return StatusCode::NO_CONTENT.into_response();
    }
    // Only GitHub for now; the URL-shape gate filters out non-GitHub
    // remotes and the empty fallback for builds without a `.git` dir.
    let Some(repo_url) = option_env!("COMIC_BUILD_REPO_URL").filter(|s| !s.is_empty()) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let Some((owner, repo)) = parse_github_repo(repo_url) else {
        return StatusCode::NO_CONTENT.into_response();
    };

    // Cache check first — `fetched_at` <= TTL old ⇒ reuse.
    {
        let cache = app.latest_release_cache.lock().await;
        if let Some(when) = cache.fetched_at
            && when.elapsed() < TTL
        {
            return match cache.value.clone() {
                Some(v) => Json(v).into_response(),
                None => StatusCode::NO_CONTENT.into_response(),
            };
        }
    }

    // Cache miss / stale → fetch.
    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
    let fetched = fetch_release(&url).await;

    // Store result (success OR None on error) so a flaky GitHub doesn't
    // get hammered for the next TTL window.
    {
        let mut cache = app.latest_release_cache.lock().await;
        cache.fetched_at = Some(Instant::now());
        cache.value = fetched.clone();
    }

    match fetched {
        Some(v) => Json(v).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

/// Pluck `(owner, repo)` from a URL like `https://github.com/owner/repo`.
/// Returns `None` for non-GitHub URLs or shapes that don't fit.
pub(crate) fn parse_github_repo(url: &str) -> Option<(&str, &str)> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let trimmed = rest.trim_end_matches('/');
    let (owner, repo) = trimmed.split_once('/')?;
    // Reject paths with extra segments (`/owner/repo/issues/123`).
    if repo.contains('/') {
        return None;
    }
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner, repo))
}

/// Issue a single GET to `{url}` and parse the GitHub release shape.
/// `pub` so the wiremock integration test in `tests/server_releases.rs`
/// can drive it against a controllable upstream without going through
/// the full Axum stack.
pub async fn fetch_release(url: &str) -> Option<LatestReleaseView> {
    let client = match reqwest::Client::builder().timeout(FETCH_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "failed to build release-check client");
            return None;
        }
    };
    let res = client
        .get(url)
        // GitHub returns 403 when the User-Agent is missing.
        .header(reqwest::header::USER_AGENT, "Folio-release-check/1.0")
        .header(
            reqwest::header::ACCEPT,
            "application/vnd.github+json",
        )
        .send()
        .await;
    let res = match res {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, %url, "release-check fetch failed");
            return None;
        }
    };
    if !res.status().is_success() {
        tracing::warn!(status = %res.status(), %url, "release-check non-2xx");
        return None;
    }
    let body = match res.json::<GithubRelease>().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, %url, "release-check parse failed");
            return None;
        }
    };
    Some(LatestReleaseView {
        tag: body.tag_name,
        html_url: body.html_url,
        published_at: body.published_at,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_github_repo;

    #[test]
    fn parse_canonical_https_url() {
        assert_eq!(
            parse_github_repo("https://github.com/mbryantms/folio"),
            Some(("mbryantms", "folio"))
        );
    }

    #[test]
    fn parse_trailing_slash() {
        assert_eq!(
            parse_github_repo("https://github.com/mbryantms/folio/"),
            Some(("mbryantms", "folio"))
        );
    }

    #[test]
    fn reject_non_github() {
        assert!(parse_github_repo("https://gitlab.com/mbryantms/folio").is_none());
        assert!(parse_github_repo("https://forgejo.example.com/foo/bar").is_none());
    }

    #[test]
    fn reject_extra_segments() {
        assert!(parse_github_repo("https://github.com/mbryantms/folio/issues/123").is_none());
    }

    #[test]
    fn reject_empty_owner_or_repo() {
        assert!(parse_github_repo("https://github.com//folio").is_none());
        assert!(parse_github_repo("https://github.com/mbryantms/").is_none());
    }
}
