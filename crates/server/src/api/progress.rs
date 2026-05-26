//! `POST /progress` (upsert), `GET /progress?since=…` (sync delta).
//!
//! Authoritative storage layer for reading progress, backed by the
//! `progress_records` table. Multi-device conflicts are resolved by
//! `max(last_page)` on the server. The spec's original §9 plan to
//! swap this for Automerge CRDT sync was reconsidered and dropped on
//! 2026-05-15 (see spec §9 decision note).
//!
//! Error envelope: every error response flows through the shared
//! `crate::api::error` helper. The `X-Progress-Api` header that used to
//! ride every response was dropped in audit-remediation M3 — the
//! versioning shim was unused by clients and represented premature
//! infrastructure.

use axum::{
    Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::{
    issue, library_user_access,
    progress_record::{self, ActiveModel as ProgressAM, Entity as ProgressEntity},
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, Unchanged,
};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::error;
use crate::auth::CurrentUser;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        // Single-record `/progress` endpoints aren't in the OpenAPI spec —
        // they predate the `/me/progress/bulk` endpoints below and are kept
        // as a transitional surface. Register them as plain routes so the
        // spec only documents the bulk shape clients should target.
        .route(
            "/progress",
            axum::routing::post(upsert).get(list),
        )
        .routes(routes!(upsert_series))
        .routes(routes!(upsert_bulk))
        .routes(routes!(upsert_series_bulk))
}

#[derive(Debug, Deserialize)]
pub struct UpsertReq {
    pub issue_id: String,
    pub page: i32,
    /// Optional — when present, sets the `finished` flag explicitly
    /// (e.g. "Mark as read", "Mark as unread", or the reader's
    /// last-page auto-finish). When `None`, the existing `finished`
    /// flag is preserved so per-page progress writes from the reader
    /// cannot accidentally clear a previously-finished issue (e.g.
    /// when the user jumps mid-issue via a bookmark deep-link).
    #[serde(default)]
    pub finished: Option<bool>,
    #[serde(default)]
    pub device: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProgressView {
    pub issue_id: String,
    pub page: i32,
    pub percent: f64,
    pub finished: bool,
    pub updated_at: String,
    /// Authoritative timestamp the issue was flipped to finished;
    /// `None` for in-progress / unread rows.
    pub finished_at: Option<String>,
}

impl From<progress_record::Model> for ProgressView {
    fn from(m: progress_record::Model) -> Self {
        Self {
            issue_id: m.issue_id,
            page: m.last_page,
            percent: m.percent,
            finished: m.finished,
            updated_at: m.updated_at.to_rfc3339(),
            finished_at: m.finished_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// Resolve the new `finished_at` value for an upsert. Centralized so
/// the four write paths (`upsert_for`, `upsert_series`, `upsert_bulk`,
/// `upsert_series_bulk`) can't drift on the flip semantics.
///
///   - false → true:  stamp `now`
///   - true  → false: clear to `None`
///   - true  → true:  keep the previous timestamp (backfilled rows
///     may have inherited their `updated_at`; respect that)
///   - false → false: stay `None`
pub(crate) fn resolve_finished_at(
    prev_finished: bool,
    next_finished: bool,
    prev_finished_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    match (prev_finished, next_finished) {
        (false, true) => Some(now),
        (true, false) => None,
        (true, true) => prev_finished_at.or(Some(now)),
        (false, false) => None,
    }
}

/// Resolve the `is_backfill` value for a write. The flag means "this
/// finish came from a catalog/sync write, not active reading"; it's
/// `true` only when (a) the new state is finished and (b) the caller
/// explicitly opted in via `backfill: true`. Every unread write
/// clears it back to `false`, regardless of the previous row's flag
/// — the user just said this issue isn't done, so the catalog/sync
/// origin is no longer load-bearing.
///
/// Per-issue reader writes pass `req_backfill = false` unconditionally
/// (the reader is by definition active reading). Only the bulk-mark
/// and whole-series endpoints expose the toggle.
pub(crate) fn resolve_is_backfill(next_finished: bool, req_backfill: bool) -> bool {
    next_finished && req_backfill
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// RFC 3339 timestamp; only rows updated strictly after this are returned.
    pub since: Option<String>,
}

pub async fn upsert(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<UpsertReq>,
) -> Response {
    if req.page < 0 {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "page must be >= 0",
        );
    }
    // ACL: confirm the user can see the issue at all.
    let issue_row = match issue::Entity::find_by_id(req.issue_id.clone())
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "issue not found"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    if !visible(&app, &user, issue_row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    let result = upsert_for(
        &app,
        user.id,
        &issue_row,
        req.page,
        req.finished,
        req.device,
    )
    .await;
    match result {
        Ok(model) => (StatusCode::OK, Json(ProgressView::from(model))).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "progress upsert failed");
            super::error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

/// Crate-wide upsert helper used by `POST /progress` AND the OPDS
/// progress endpoints. The caller is responsible for ACL: this fn
/// trusts that `issue_row` is one the `user_id` is allowed to read.
/// Keeps the `finished`-is-sticky semantics intact across all callers.
pub(crate) async fn upsert_for(
    app: &AppState,
    user_id: uuid::Uuid,
    issue_row: &issue::Model,
    page: i32,
    finished: Option<bool>,
    device: Option<String>,
) -> Result<progress_record::Model, sea_orm::DbErr> {
    let percent = match issue_row.page_count.unwrap_or(0) {
        n if n > 0 => (page as f64 / n as f64).clamp(0.0, 1.0),
        _ => 0.0,
    };
    let now = Utc::now().fixed_offset();
    let existing = ProgressEntity::find_by_id((user_id, issue_row.id.clone()))
        .one(&app.db)
        .await?;
    match existing {
        Some(prev) => {
            // `finished` is sticky on per-page writes: when the caller
            // omits it, we keep whatever was there. Mark-as-read /
            // mark-as-unread / last-page-auto-finish all send an
            // explicit value, so user-intended toggles still flow
            // through.
            let next_finished = finished.unwrap_or(prev.finished);
            let next_finished_at =
                resolve_finished_at(prev.finished, next_finished, prev.finished_at, now);
            let am = ProgressAM {
                user_id: Unchanged(user_id),
                issue_id: Unchanged(issue_row.id.clone()),
                last_page: Set(page),
                percent: Set(percent),
                finished: Set(next_finished),
                finished_at: Set(next_finished_at),
                updated_at: Set(now),
                device: Set(device),
                // Per-issue reader writes are always active reading
                // — clear any previously-set backfill flag.
                is_backfill: Set(false),
            };
            am.update(&app.db).await
        }
        None => {
            let next_finished = finished.unwrap_or(false);
            let am = ProgressAM {
                user_id: Set(user_id),
                issue_id: Set(issue_row.id.clone()),
                last_page: Set(page),
                percent: Set(percent),
                finished: Set(next_finished),
                finished_at: Set(if next_finished { Some(now) } else { None }),
                updated_at: Set(now),
                device: Set(device),
                is_backfill: Set(false),
            };
            am.insert(&app.db).await
        }
    }
}

pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> Response {
    let mut query = ProgressEntity::find()
        .filter(progress_record::Column::UserId.eq(user.id))
        .order_by_asc(progress_record::Column::UpdatedAt);
    if let Some(since) = q.since.as_deref() {
        match chrono::DateTime::parse_from_rfc3339(since) {
            Ok(ts) => {
                query = query.filter(progress_record::Column::UpdatedAt.gt(ts));
            }
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "validation",
                    "since must be RFC3339",
                );
            }
        }
    }
    let rows = match query.all(&app.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "progress list failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let views: Vec<ProgressView> = rows.into_iter().map(Into::into).collect();
    (StatusCode::OK, Json(serde_json::json!({"records": views}))).into_response()
}

/// Body for `POST /series/{id}/progress` — bulk read/unread for every active
/// issue in the series. `finished=true` writes a "fully read" record for each
/// (page = page_count - 1, percent = 1.0); `finished=false` resets to "unread"
/// (page = 0, percent = 0.0). Soft-deleted issues are excluded — they're not
/// visible to the user and writing progress for them would clutter the table.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpsertSeriesReq {
    pub finished: bool,
    /// Optional client tag; mirrors the per-issue endpoint so the same
    /// device label rolls through for sync-aware UIs.
    #[serde(default)]
    pub device: Option<String>,
    /// "Updating my collection — don't count toward today's reading
    /// activity." When `true` and `finished == true`, every written
    /// row carries `is_backfill = true` and is filtered out of the
    /// reading log, heatmap, daily-pages stat, streak counter, and
    /// the Just Finished sort. `false` (default) preserves the
    /// pre-v0.5.7 behaviour of treating bulk-marks as active reading.
    /// Ignored when `finished == false` — unread writes always clear
    /// the flag.
    #[serde(default)]
    pub backfill: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpsertSeriesResp {
    /// Issue rows that received a new or updated progress record.
    pub updated: u32,
    /// Issues skipped because the user already has a matching `finished`
    /// state. Avoids unnecessary write traffic on idle re-clicks.
    pub skipped: u32,
}

#[utoipa::path(
    operation_id = "progress_upsert_series",    post,
    path = "/series/{slug}/progress",
    params(("slug" = String, Path,)),
    request_body = UpsertSeriesReq,
    responses(
        (status = 200, body = UpsertSeriesResp),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn upsert_series(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Json(req): Json<UpsertSeriesReq>,
) -> Response {
    let srow = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible(&app, &user, srow.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "series not found");
    }

    // Only active, on-disk issues. The user can't read a removed issue, so
    // the read-state for those is meaningless and the row would clutter
    // future "unread series" queries.
    let issues = match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(srow.id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "series-progress issue lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let now = Utc::now().fixed_offset();
    let mut updated: u32 = 0;
    let mut skipped: u32 = 0;

    for iss in issues {
        let target_page = if req.finished {
            iss.page_count.unwrap_or(1).max(1) - 1
        } else {
            0
        };
        let target_percent = if req.finished { 1.0 } else { 0.0 };

        // Read existing record so a redundant click doesn't bump
        // updated_at (the sync-delta endpoint orders on updated_at).
        let existing = match ProgressEntity::find_by_id((user.id, iss.id.clone()))
            .one(&app.db)
            .await
        {
            Ok(opt) => opt,
            Err(e) => {
                tracing::warn!(error = %e, "series-progress lookup failed");
                continue;
            }
        };

        match existing {
            Some(row) if row.finished == req.finished && row.last_page == target_page => {
                skipped += 1;
                continue;
            }
            Some(prev) => {
                let next_finished_at =
                    resolve_finished_at(prev.finished, req.finished, prev.finished_at, now);
                let am = ProgressAM {
                    user_id: Unchanged(user.id),
                    issue_id: Unchanged(iss.id.clone()),
                    last_page: Set(target_page),
                    percent: Set(target_percent),
                    finished: Set(req.finished),
                    finished_at: Set(next_finished_at),
                    updated_at: Set(now),
                    device: Set(req.device.clone()),
                    is_backfill: Set(resolve_is_backfill(req.finished, req.backfill)),
                };
                if let Err(e) = am.update(&app.db).await {
                    tracing::warn!(error = %e, issue_id = %iss.id, "series-progress update failed");
                    continue;
                }
                updated += 1;
            }
            None => {
                let am = ProgressAM {
                    user_id: Set(user.id),
                    issue_id: Set(iss.id.clone()),
                    last_page: Set(target_page),
                    percent: Set(target_percent),
                    finished: Set(req.finished),
                    finished_at: Set(if req.finished { Some(now) } else { None }),
                    updated_at: Set(now),
                    device: Set(req.device.clone()),
                    is_backfill: Set(resolve_is_backfill(req.finished, req.backfill)),
                };
                if let Err(e) = am.insert(&app.db).await {
                    tracing::warn!(error = %e, issue_id = %iss.id, "series-progress insert failed");
                    continue;
                }
                updated += 1;
            }
        }
    }

    (StatusCode::OK, Json(UpsertSeriesResp { updated, skipped })).into_response()
}

/// Body for `POST /me/progress/bulk` — bulk read/unread for an
/// arbitrary list of issue ids. Used by the multi-select toolbar
/// across the series / collection / view / CBL list pages. See
/// `~/.claude/plans/multi-select-bulk-actions-1.0.md` (M2).
///
/// Differs from `upsert_series` in two ways: (1) the caller supplies
/// the explicit issue-id list rather than "every active issue in
/// series X"; (2) ACL filtering walks each issue's library_id and
/// drops anything the caller can't see. The response counts skipped
/// rows separately so the toast can surface "N marked read, M
/// already read."
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpsertBulkReq {
    pub issue_ids: Vec<String>,
    pub finished: bool,
    #[serde(default)]
    pub device: Option<String>,
    /// See [`UpsertSeriesReq::backfill`] — same semantics: when
    /// `true` and `finished == true`, every row written carries
    /// `is_backfill = true` and is excluded from time-bound activity
    /// surfaces. Defaults to `false` for back-compat.
    #[serde(default)]
    pub backfill: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpsertBulkResp {
    /// Issues whose progress row was created or updated.
    pub updated: u32,
    /// Issues already in the target state (no DB write — cheap re-click).
    pub skipped: u32,
    /// Issues the caller doesn't have library access to read. Silent
    /// from the user's perspective; surfaced here so an admin
    /// debugging a "mark read did nothing" report can see the
    /// filter fired.
    pub forbidden: u32,
    /// Issues whose id didn't resolve to a row (removed / never
    /// existed / typo). Treated the same as forbidden — the caller
    /// doesn't get to distinguish a missing row from one they're
    /// not allowed to see.
    pub not_found: u32,
}

#[utoipa::path(
    operation_id = "progress_upsert_bulk",    post,
    path = "/me/progress/bulk",
    request_body = UpsertBulkReq,
    responses(
        (status = 200, body = UpsertBulkResp),
        (status = 400, description = "validation"),
    )
)]
#[handler]
pub async fn upsert_bulk(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<UpsertBulkReq>,
) -> Response {
    // Cap the per-request id list to keep the loop bounded — a malicious
    // client could otherwise enqueue thousands of round-trips inside
    // one request. 500 covers any realistic multi-select session;
    // larger batches should be done via the per-series endpoint.
    const MAX_IDS: usize = 500;
    if req.issue_ids.len() > MAX_IDS {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            &format!("issue_ids cap is {MAX_IDS}"),
        );
    }
    // Empty list is a 200 OK with all-zero counts — easier on clients
    // that compute their selection list dynamically.
    if req.issue_ids.is_empty() {
        return (
            StatusCode::OK,
            Json(UpsertBulkResp {
                updated: 0,
                skipped: 0,
                forbidden: 0,
                not_found: 0,
            }),
        )
            .into_response();
    }
    // Dedup ids — a client that double-checks the same card shouldn't
    // get a 2x cost. Preserves order for predictable iteration.
    let mut seen = std::collections::HashSet::with_capacity(req.issue_ids.len());
    let ids: Vec<String> = req
        .issue_ids
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect();

    let rows = match issue::Entity::find()
        .filter(issue::Column::Id.is_in(ids.clone()))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "bulk-progress issue lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let found_ids: std::collections::HashSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    let not_found = ids
        .iter()
        .filter(|id| !found_ids.contains(id.as_str()))
        .count() as u32;

    // Pre-fetch the library-access set for non-admin users in one
    // query, so the per-issue ACL check is a HashSet hit rather than
    // a SELECT per row.
    let allowed_libraries: Option<std::collections::HashSet<uuid::Uuid>> = if user.role == "admin" {
        None
    } else {
        match library_user_access::Entity::find()
            .filter(library_user_access::Column::UserId.eq(user.id))
            .all(&app.db)
            .await
        {
            Ok(v) => Some(v.into_iter().map(|r| r.library_id).collect()),
            Err(e) => {
                tracing::warn!(error = %e, "bulk-progress acl lookup failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    };

    let now = Utc::now().fixed_offset();
    let mut updated: u32 = 0;
    let mut skipped: u32 = 0;
    let mut forbidden: u32 = 0;

    for iss in rows {
        if let Some(allowed) = &allowed_libraries
            && !allowed.contains(&iss.library_id)
        {
            forbidden += 1;
            continue;
        }
        let target_page = if req.finished {
            iss.page_count.unwrap_or(1).max(1) - 1
        } else {
            0
        };
        let target_percent = if req.finished { 1.0 } else { 0.0 };

        let existing = match ProgressEntity::find_by_id((user.id, iss.id.clone()))
            .one(&app.db)
            .await
        {
            Ok(opt) => opt,
            Err(e) => {
                tracing::warn!(error = %e, issue_id = %iss.id, "bulk-progress lookup failed");
                continue;
            }
        };

        match existing {
            Some(row) if row.finished == req.finished && row.last_page == target_page => {
                skipped += 1;
            }
            Some(prev) => {
                let next_finished_at =
                    resolve_finished_at(prev.finished, req.finished, prev.finished_at, now);
                let am = ProgressAM {
                    user_id: Unchanged(user.id),
                    issue_id: Unchanged(iss.id.clone()),
                    last_page: Set(target_page),
                    percent: Set(target_percent),
                    finished: Set(req.finished),
                    finished_at: Set(next_finished_at),
                    updated_at: Set(now),
                    device: Set(req.device.clone()),
                    is_backfill: Set(resolve_is_backfill(req.finished, req.backfill)),
                };
                if let Err(e) = am.update(&app.db).await {
                    tracing::warn!(error = %e, issue_id = %iss.id, "bulk-progress update failed");
                    continue;
                }
                updated += 1;
            }
            None => {
                let am = ProgressAM {
                    user_id: Set(user.id),
                    issue_id: Set(iss.id.clone()),
                    last_page: Set(target_page),
                    percent: Set(target_percent),
                    finished: Set(req.finished),
                    finished_at: Set(if req.finished { Some(now) } else { None }),
                    updated_at: Set(now),
                    device: Set(req.device.clone()),
                    is_backfill: Set(resolve_is_backfill(req.finished, req.backfill)),
                };
                if let Err(e) = am.insert(&app.db).await {
                    tracing::warn!(error = %e, issue_id = %iss.id, "bulk-progress insert failed");
                    continue;
                }
                updated += 1;
            }
        }
    }

    (
        StatusCode::OK,
        Json(UpsertBulkResp {
            updated,
            skipped,
            forbidden,
            not_found,
        }),
    )
        .into_response()
}

/// Body for `POST /me/progress/series-bulk` — bulk read/unread
/// applied across every active issue of an arbitrary list of
/// series. Sister endpoint to `upsert_bulk` (which takes
/// issue_ids); used by the multi-select toolbar on filter views
/// where the cards are series. Each series in `series_ids` is
/// expanded server-side to its active issues, then walked through
/// the same `upsert_for` helper.
///
/// Plan: `~/.claude/plans/multi-select-bulk-actions-1.0.md`
/// (M6 extension per user request 2026-05-17 — filter views
/// originally weren't going to support mark-read since they're
/// series-only, but operating per-series-then-per-issue is the
/// natural semantics).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpsertSeriesBulkReq {
    pub series_ids: Vec<uuid::Uuid>,
    pub finished: bool,
    #[serde(default)]
    pub device: Option<String>,
    /// See [`UpsertSeriesReq::backfill`].
    #[serde(default)]
    pub backfill: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpsertSeriesBulkResp {
    /// Issue rows that received a new or updated progress record.
    pub updated: u32,
    /// Issues already in the target state.
    pub skipped: u32,
    /// Series the caller can't see (library access denied).
    pub forbidden_series: u32,
    /// Series whose id didn't resolve.
    pub not_found_series: u32,
}

#[utoipa::path(
    operation_id = "progress_upsert_series_bulk",    post,
    path = "/me/progress/series-bulk",
    request_body = UpsertSeriesBulkReq,
    responses(
        (status = 200, body = UpsertSeriesBulkResp),
        (status = 400, description = "validation"),
    )
)]
#[handler]
pub async fn upsert_series_bulk(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<UpsertSeriesBulkReq>,
) -> Response {
    // 100-series cap so a runaway client can't kick off a O(N*M)
    // walk across a library's whole index. With ~50 issues per
    // series typical, 100 series → ~5k progress writes max.
    const MAX_SERIES: usize = 100;
    if req.series_ids.len() > MAX_SERIES {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            &format!("series_ids cap is {MAX_SERIES}"),
        );
    }
    if req.series_ids.is_empty() {
        return (
            StatusCode::OK,
            Json(UpsertSeriesBulkResp {
                updated: 0,
                skipped: 0,
                forbidden_series: 0,
                not_found_series: 0,
            }),
        )
            .into_response();
    }
    // Dedup series ids.
    let mut seen = std::collections::HashSet::with_capacity(req.series_ids.len());
    let series_ids: Vec<uuid::Uuid> = req
        .series_ids
        .into_iter()
        .filter(|id| seen.insert(*id))
        .collect();

    // Pre-fetch series rows (need library_id for ACL).
    let series_rows = match entity::series::Entity::find()
        .filter(entity::series::Column::Id.is_in(series_ids.clone()))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "series-bulk-progress series lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let found_ids: std::collections::HashSet<uuid::Uuid> =
        series_rows.iter().map(|r| r.id).collect();
    let not_found_series = series_ids
        .iter()
        .filter(|id| !found_ids.contains(id))
        .count() as u32;

    // Pre-fetch library-access set for non-admins.
    let allowed_libraries: Option<std::collections::HashSet<uuid::Uuid>> = if user.role == "admin" {
        None
    } else {
        match library_user_access::Entity::find()
            .filter(library_user_access::Column::UserId.eq(user.id))
            .all(&app.db)
            .await
        {
            Ok(v) => Some(v.into_iter().map(|r| r.library_id).collect()),
            Err(e) => {
                tracing::warn!(error = %e, "series-bulk-progress acl lookup failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    };

    let now = chrono::Utc::now().fixed_offset();
    let mut updated: u32 = 0;
    let mut skipped: u32 = 0;
    let mut forbidden_series: u32 = 0;

    for srow in series_rows {
        if let Some(allowed) = &allowed_libraries
            && !allowed.contains(&srow.library_id)
        {
            forbidden_series += 1;
            continue;
        }
        // Pull every active, on-disk issue for the series in one
        // query. Mirrors `upsert_series`'s filter.
        let issues = match issue::Entity::find()
            .filter(issue::Column::SeriesId.eq(srow.id))
            .filter(issue::Column::State.eq("active"))
            .filter(issue::Column::RemovedAt.is_null())
            .all(&app.db)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, series_id = %srow.id, "series-bulk issue lookup failed");
                continue;
            }
        };
        for iss in issues {
            let target_page = if req.finished {
                iss.page_count.unwrap_or(1).max(1) - 1
            } else {
                0
            };
            let target_percent = if req.finished { 1.0 } else { 0.0 };
            let existing = match ProgressEntity::find_by_id((user.id, iss.id.clone()))
                .one(&app.db)
                .await
            {
                Ok(opt) => opt,
                Err(e) => {
                    tracing::warn!(error = %e, issue_id = %iss.id, "series-bulk lookup failed");
                    continue;
                }
            };
            match existing {
                Some(row) if row.finished == req.finished && row.last_page == target_page => {
                    skipped += 1;
                }
                Some(prev) => {
                    let next_finished_at =
                        resolve_finished_at(prev.finished, req.finished, prev.finished_at, now);
                    let am = ProgressAM {
                        user_id: Unchanged(user.id),
                        issue_id: Unchanged(iss.id.clone()),
                        last_page: Set(target_page),
                        percent: Set(target_percent),
                        finished: Set(req.finished),
                        finished_at: Set(next_finished_at),
                        updated_at: Set(now),
                        device: Set(req.device.clone()),
                        is_backfill: Set(resolve_is_backfill(req.finished, req.backfill)),
                    };
                    if let Err(e) = am.update(&app.db).await {
                        tracing::warn!(error = %e, issue_id = %iss.id, "series-bulk update failed");
                        continue;
                    }
                    updated += 1;
                }
                None => {
                    let am = ProgressAM {
                        user_id: Set(user.id),
                        issue_id: Set(iss.id.clone()),
                        last_page: Set(target_page),
                        percent: Set(target_percent),
                        finished: Set(req.finished),
                        finished_at: Set(if req.finished { Some(now) } else { None }),
                        updated_at: Set(now),
                        device: Set(req.device.clone()),
                        is_backfill: Set(resolve_is_backfill(req.finished, req.backfill)),
                    };
                    if let Err(e) = am.insert(&app.db).await {
                        tracing::warn!(error = %e, issue_id = %iss.id, "series-bulk insert failed");
                        continue;
                    }
                    updated += 1;
                }
            }
        }
    }

    (
        StatusCode::OK,
        Json(UpsertSeriesBulkResp {
            updated,
            skipped,
            forbidden_series,
            not_found_series,
        }),
    )
        .into_response()
}

async fn visible(app: &AppState, user: &CurrentUser, lib_id: uuid::Uuid) -> bool {
    if user.role == "admin" {
        return true;
    }
    library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .filter(library_user_access::Column::LibraryId.eq(lib_id))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_some()
}
