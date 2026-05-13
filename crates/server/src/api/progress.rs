//! `POST /progress` (upsert), `GET /progress?since=…` (sync delta).
//!
//! Phase 2 storage layer for reading progress (§9.7). Writes back to the
//! `progress_records` table; replaced by Automerge sync in Phase 4.
//!
//! Forward-compat: every response carries `X-Progress-Api: 1`. Phase 4 bumps
//! the value and old clients receive 410 Gone via a different code path.

use axum::{
    Json, Router,
    extract::{Path as AxPath, Query, State},
    http::{HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
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

use crate::auth::CurrentUser;
use crate::state::AppState;

const API_VERSION_HEADER: &str = "x-progress-api";
const API_VERSION: &str = "1";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/progress", post(upsert).get(list))
        .route("/series/{slug}/progress", post(upsert_series))
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
}

impl From<progress_record::Model> for ProgressView {
    fn from(m: progress_record::Model) -> Self {
        Self {
            issue_id: m.issue_id,
            page: m.last_page,
            percent: m.percent,
            finished: m.finished,
            updated_at: m.updated_at.to_rfc3339(),
        }
    }
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
        return error(StatusCode::BAD_REQUEST, "validation", "page must be >= 0");
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

    let result = upsert_for(&app, user.id, &issue_row, req.page, req.finished, req.device).await;
    match result {
        Ok(model) => versioned(StatusCode::OK, Json(ProgressView::from(model)).into_response()),
        Err(e) => {
            tracing::warn!(error = %e, "progress upsert failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
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
            let am = ProgressAM {
                user_id: Unchanged(user_id),
                issue_id: Unchanged(issue_row.id.clone()),
                last_page: Set(page),
                percent: Set(percent),
                finished: Set(next_finished),
                updated_at: Set(now),
                device: Set(device),
            };
            am.update(&app.db).await
        }
        None => {
            let am = ProgressAM {
                user_id: Set(user_id),
                issue_id: Set(issue_row.id.clone()),
                last_page: Set(page),
                percent: Set(percent),
                finished: Set(finished.unwrap_or(false)),
                updated_at: Set(now),
                device: Set(device),
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
    versioned(
        StatusCode::OK,
        Json(serde_json::json!({"records": views})).into_response(),
    )
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
    post,
    path = "/series/{slug}/progress",
    params(("slug" = String, Path,)),
    request_body = UpsertSeriesReq,
    responses(
        (status = 200, body = UpsertSeriesResp),
        (status = 404, description = "series not found"),
    )
)]
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
            Some(_) => {
                let am = ProgressAM {
                    user_id: Unchanged(user.id),
                    issue_id: Unchanged(iss.id.clone()),
                    last_page: Set(target_page),
                    percent: Set(target_percent),
                    finished: Set(req.finished),
                    updated_at: Set(now),
                    device: Set(req.device.clone()),
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
                    updated_at: Set(now),
                    device: Set(req.device.clone()),
                };
                if let Err(e) = am.insert(&app.db).await {
                    tracing::warn!(error = %e, issue_id = %iss.id, "series-progress insert failed");
                    continue;
                }
                updated += 1;
            }
        }
    }

    versioned(
        StatusCode::OK,
        Json(UpsertSeriesResp { updated, skipped }).into_response(),
    )
}

fn versioned(status: StatusCode, mut resp: Response) -> Response {
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        HeaderName::from_static(API_VERSION_HEADER),
        HeaderValue::from_static(API_VERSION),
    );
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
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

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    versioned(
        status,
        axum::Json(serde_json::json!({"error": {"code": code, "message": message}}))
            .into_response(),
    )
}
