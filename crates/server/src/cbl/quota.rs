//! Per-user CBL import quota (security audit M-5, WP-6.3).
//!
//! The SSRF guard on `kind = 'url'` imports (H-1) closed the internal-network
//! probe, but a registered user could still make the server fetch remote
//! documents as fast as they liked — directly (create / manual refresh) or on
//! a recurring cadence (each URL- or catalog-sourced list with a
//! `refresh_schedule` is re-fetched by the hourly sweep). Two bounds:
//!
//! 1. **Rate** — at most [`IMPORTS_PER_HOUR`] import-class operations per
//!    user per fixed one-hour window: every create (upload, URL, catalog)
//!    and every manual refresh (`refresh_one`, `refresh_all`). Redis-backed
//!    (`INCR` + `EXPIRE` on first hit) so a restart can't reset it; denial is
//!    `429 rate_limited` with `Retry-After` = the window's remaining TTL.
//! 2. **Standing count** — at most [`MAX_REMOTE_LISTS_PER_USER`] lists that
//!    the server fetches on the user's behalf (`source_kind` `url` or
//!    `catalog`). That bounds the recurring-refresh amplification the rate
//!    window alone can't (20/hour × weeks would otherwise accumulate
//!    thousands of scheduled fetches). Denial is `422 cbl.remote_list_limit`.
//!
//! Admins are exempt from both: they already hold every capability the quota
//! protects (and system lists are admin-curated). Redis failures fail open,
//! matching [`crate::auth::failed_auth`] — a degraded backend shouldn't block
//! legitimate imports, and the standing-count bound is DB-backed regardless.
//! The `auth.rate_limit_enabled` kill switch disables the rate window (not
//! the standing count, which is a resource cap rather than a rate limit).

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use entity::cbl_list;
use redis::AsyncCommands;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};

use crate::auth::CurrentUser;
use crate::state::AppState;

/// Import-class operations allowed per user per hour.
pub const IMPORTS_PER_HOUR: i64 = 20;
/// Length of the fixed quota window.
pub const WINDOW_SECS: i64 = 60 * 60;
/// Standing cap on server-fetched (`url` / `catalog`) lists per user.
pub const MAX_REMOTE_LISTS_PER_USER: u64 = 50;

fn window_key(user_id: uuid::Uuid) -> String {
    format!("cbl_import_quota:{user_id}")
}

fn is_exempt(user: &CurrentUser) -> bool {
    user.role == "admin"
}

/// Consume one import-class token for `user`. `Err(response)` is the 429 the
/// handler returns verbatim.
pub async fn consume_import(app: &AppState, user: &CurrentUser) -> Result<(), Response> {
    if is_exempt(user) || !app.cfg().rate_limit_enabled {
        return Ok(());
    }
    let mut redis = app.jobs.redis.clone();
    let key = window_key(user.id);
    let count: i64 = match redis.incr(&key, 1).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, user_id = %user.id, "cbl quota: INCR failed; fail-open");
            return Ok(());
        }
    };
    if count == 1 {
        let _: Result<(), _> = redis.expire(&key, WINDOW_SECS).await;
    }
    if count <= IMPORTS_PER_HOUR {
        return Ok(());
    }
    let ttl: i64 = redis.ttl(&key).await.unwrap_or(WINDOW_SECS);
    // A key with no expiry (-1) would never reset — repair it.
    if ttl < 0 {
        let _: Result<(), _> = redis.expire(&key, WINDOW_SECS).await;
    }
    let retry_after = if ttl > 0 { ttl } else { WINDOW_SECS };
    metrics::counter!("folio_rate_limit_denied_total", "bucket" => "cbl_import").increment(1);
    tracing::info!(user_id = %user.id, count, "cbl import quota exceeded");
    let mut resp = crate::api::error(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        &format!(
            "CBL import quota exceeded ({IMPORTS_PER_HOUR} imports or refreshes per hour); \
             retry in {retry_after}s"
        ),
    );
    if let Ok(v) = HeaderValue::from_str(&retry_after.to_string()) {
        resp.headers_mut().insert(header::RETRY_AFTER, v);
    }
    Err(resp)
}

/// Refuse a new server-fetched list once `user` already owns
/// [`MAX_REMOTE_LISTS_PER_USER`] of them.
pub async fn check_remote_list_cap(app: &AppState, user: &CurrentUser) -> Result<(), Response> {
    if is_exempt(user) {
        return Ok(());
    }
    let count = match cbl_list::Entity::find()
        .filter(cbl_list::Column::OwnerUserId.eq(user.id))
        .filter(cbl_list::Column::SourceKind.is_in(["url", "catalog"]))
        .count(&app.db)
        .await
    {
        Ok(n) => n,
        Err(e) => {
            tracing::error!(error = %e, "cbl quota: remote-list count failed");
            return Err(crate::api::error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    if count >= MAX_REMOTE_LISTS_PER_USER {
        return Err(crate::api::error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "cbl.remote_list_limit",
            &format!(
                "you already have {count} URL/catalog reading lists (limit \
                 {MAX_REMOTE_LISTS_PER_USER}); delete one or upload the file instead"
            ),
        ));
    }
    Ok(())
}
