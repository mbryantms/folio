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
//!
//! **Bulk continuation allowance.** `check-all` / `refresh-all` stop starting
//! lists after a fixed time budget and report the rest `attempted: false`;
//! the client then sends those ids back as `list_ids`, possibly over several
//! rounds. A whole check or refresh still costs **one** token
//! ([`consume_bulk`] / [`store_continuation`]):
//!
//! - After the work, the **set of list ids** the run has still not reached
//!   is stored as a single-use allowance (one value per [`BulkOp`] and user,
//!   [`CONTINUATION_TTL_SECS`]): the previous allowance minus the ids this
//!   call asked for, plus the ids it reports `attempted: false`. Ids the
//!   client has not sent yet (it sends at most 1000 per round) stay covered;
//!   ids it asked for and that were dropped (deleted, no longer refreshable)
//!   fall out. Nothing is stored when the set is empty, or when it is over
//!   [`MAX_STORED_CONTINUATION_IDS`] (the next call then simply pays).
//! - A call with `list_ids` takes the stored set atomically (`GETDEL` of one
//!   serialised value) **before** any work. It is free only when every id it
//!   names (after de-duplication) is in that set; otherwise it pays a token
//!   like any other call, and the set it took is gone.
//! - A call without `list_ids` starts a fresh run: it pays first and only
//!   once the token was granted clears the stored allowance, so a refused
//!   call (429) or a second tab cannot wipe a running check's allowance.
//!
//! The bound the quota exists for therefore holds exactly: a free call only
//! names lists the run has not reached yet, each reached list leaves the set
//! for good, and the take happens before the work, so the allowance cannot
//! be replayed or spent by two calls at once. **Per token, each of the
//! caller's lists is checked (or refreshed) at most once**, which is what one
//! pass cost before the bulk time limit existed. Redis failures fail open as
//! elsewhere here: a failed read means no allowance, so the call pays.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use entity::cbl_list;
use entity::user::UserRole;
use redis::AsyncCommands;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use std::collections::HashSet;

use crate::auth::CurrentUser;
use crate::state::AppState;

/// Import-class operations allowed per user per hour.
pub const IMPORTS_PER_HOUR: i64 = 20;
/// Length of the fixed quota window.
pub const WINDOW_SECS: i64 = 60 * 60;
/// Standing cap on server-fetched (`url` / `catalog`) lists per user.
pub const MAX_REMOTE_LISTS_PER_USER: u64 = 50;

/// Redis key of a user's hourly import window.
pub fn window_key(user_id: uuid::Uuid) -> String {
    format!("cbl_import_quota:{user_id}")
}

fn is_exempt(user: &CurrentUser) -> bool {
    user.role == UserRole::Admin
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

/// The bulk operation a continuation allowance belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkOp {
    Check,
    Refresh,
}

impl BulkOp {
    fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Refresh => "refresh",
        }
    }
}

/// How long a continuation allowance lives. A round takes under a minute and
/// the client asks again at once; a few minutes covers a slow network.
pub const CONTINUATION_TTL_SECS: u64 = 5 * 60;

/// Most list ids a stored allowance holds. A run that leaves more lists
/// unreached stores none and its next call pays: that is far beyond what a
/// caller owns in practice (50 server-fetched lists per user, uploads at 20
/// an hour), and it keeps one Redis value small (about 150 KiB at most).
pub const MAX_STORED_CONTINUATION_IDS: usize = 4000;

/// Redis key of a user's continuation allowance for `op`.
pub fn allowance_key(op: BulkOp, user_id: uuid::Uuid) -> String {
    format!("cbl_bulk_continuation:{}:{user_id}", op.as_str())
}

/// What [`consume_bulk`] granted; [`store_continuation`] needs it to compute
/// the next allowance.
#[derive(Debug, Default)]
pub struct BulkGrant {
    /// The quota does not apply (admin, rate limit off): store nothing.
    exempt: bool,
    /// The allowance this call was free under; empty when it paid.
    previous: HashSet<uuid::Uuid>,
    /// The distinct ids the call named.
    asked: HashSet<uuid::Uuid>,
}

/// The quota gate of a `check-all` / `refresh-all` call (see the module
/// docs). `list_ids` is the call's `list_ids`, `None` when it sent no body.
pub async fn consume_bulk(
    app: &AppState,
    user: &CurrentUser,
    op: BulkOp,
    list_ids: Option<&[uuid::Uuid]>,
) -> Result<BulkGrant, Response> {
    if is_exempt(user) || !app.cfg().rate_limit_enabled {
        return Ok(BulkGrant {
            exempt: true,
            ..BulkGrant::default()
        });
    }
    let mut redis = app.jobs.redis.clone();
    let key = allowance_key(op, user.id);
    let Some(ids) = list_ids else {
        consume_import(app, user).await?;
        if let Err(e) = redis.del::<_, ()>(&key).await {
            tracing::warn!(error = %e, user_id = %user.id, "cbl quota: allowance DEL failed");
        }
        return Ok(BulkGrant::default());
    };
    let asked: HashSet<uuid::Uuid> = ids.iter().copied().collect();
    let stored: Option<HashSet<uuid::Uuid>> = match redis.get_del::<_, Option<String>>(&key).await {
        Ok(v) => v.and_then(|s| serde_json::from_str(&s).ok()),
        Err(e) => {
            tracing::warn!(error = %e, user_id = %user.id, "cbl quota: allowance GETDEL failed; no allowance");
            None
        }
    };
    if let Some(previous) = stored
        && asked.is_subset(&previous)
    {
        return Ok(BulkGrant {
            exempt: false,
            previous,
            asked,
        });
    }
    consume_import(app, user).await?;
    Ok(BulkGrant {
        exempt: false,
        previous: HashSet::new(),
        asked,
    })
}

/// Store the next allowance after a call's work: what [`consume_bulk`]
/// granted it under, minus what it asked for, plus `unreached` (the ids it
/// reported `attempted: false`). Nothing is stored when that is empty or
/// over [`MAX_STORED_CONTINUATION_IDS`].
pub async fn store_continuation(
    app: &AppState,
    user: &CurrentUser,
    op: BulkOp,
    grant: BulkGrant,
    unreached: impl IntoIterator<Item = uuid::Uuid>,
) {
    if grant.exempt {
        return;
    }
    let mut next: HashSet<uuid::Uuid> = grant
        .previous
        .into_iter()
        .filter(|id| !grant.asked.contains(id))
        .collect();
    next.extend(unreached);
    if next.is_empty() || next.len() > MAX_STORED_CONTINUATION_IDS {
        return;
    }
    let Ok(value) = serde_json::to_string(&next) else {
        return;
    };
    let mut redis = app.jobs.redis.clone();
    if let Err(e) = redis
        .set_ex::<_, _, ()>(allowance_key(op, user.id), value, CONTINUATION_TTL_SECS)
        .await
    {
        tracing::warn!(error = %e, user_id = %user.id, "cbl quota: allowance SET failed");
    }
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
