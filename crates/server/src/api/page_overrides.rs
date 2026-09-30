//! Manual spread controls for the double-page reader (roadmap WP-4.3,
//! audit R18 / UX-2).
//!
//! Automatic pairing (`web/lib/reader/spreads.ts`) keys off ComicInfo's
//! `<Page DoublePage>` flag and the page aspect ratio. Offset scans and
//! unflagged spreads defeat both, so the reader lets a user correct the
//! pairing for one issue. The correction is **per user, per issue** and
//! stored server-side (`issue_page_overrides`) so it follows the user
//! across devices.
//!
//! - `GET    /me/issues/{issue_id}/page-overrides` — current overrides
//!   (all-default view when the user has none).
//! - `PUT    /me/issues/{issue_id}/page-overrides` — replace the whole
//!   set. An all-default body deletes the row.
//! - `DELETE /me/issues/{issue_id}/page-overrides` — reset to automatic.
//!
//! Visibility follows the issue: a caller who can't see the issue (library
//! ACL or age-rating cap) gets the same 404 as a missing issue, so the
//! endpoint never leaks existence and can't be used to write rows against
//! hidden issues.

use std::collections::BTreeSet;

use axum::{
    Json,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{issue, issue_page_override};
use sea_orm::{EntityTrait, sea_query::OnConflict};
use serde::{Deserialize, Serialize};
use shared::error::ApiErrorCode;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::extractors::Validated;
use super::respond;
use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;
use server_macros::handler;

/// Upper bound on each page list. Far above any real issue's page count;
/// exists only so a hostile body can't make us store a huge JSON array.
pub const MAX_OVERRIDE_PAGES: usize = 4096;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_overrides, put_overrides, delete_overrides))
}

/// A user's spread overrides for one issue. Pages are 0-based indices.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PageOverridesView {
    pub issue_id: String,
    /// Shift the pairing parity by one page (fixes an offset scan).
    pub shift_pairing: bool,
    /// Pages forced to render solo as a two-page spread, regardless of
    /// `DoublePage` metadata or aspect ratio. Sorted, unique.
    pub spread_pages: Vec<i32>,
    /// Pages forced to pair as ordinary single pages even when flagged
    /// `DoublePage` or landscape. Sorted, unique, disjoint from
    /// `spread_pages`.
    pub single_pages: Vec<i32>,
    /// RFC 3339 timestamp of the last write; `null` when the user has no
    /// overrides for this issue.
    pub updated_at: Option<String>,
}

/// Replacement body for `PUT`. Omitted fields default to "no override".
#[derive(Debug, Default, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct PutPageOverridesReq {
    #[serde(default)]
    #[garde(skip)]
    pub shift_pairing: bool,
    #[serde(default)]
    #[garde(length(max = MAX_OVERRIDE_PAGES), inner(range(min = 0)))]
    pub spread_pages: Vec<i32>,
    #[serde(default)]
    #[garde(length(max = MAX_OVERRIDE_PAGES), inner(range(min = 0)))]
    pub single_pages: Vec<i32>,
}

fn internal(ctx: &str, e: &dyn std::fmt::Display) -> Response {
    tracing::error!(error = %e, "page_overrides: {ctx}");
    respond(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Internal,
        "internal",
    )
}

/// Load the issue and enforce library ACL + age-rating visibility. An
/// invisible issue is reported as missing (404).
async fn visible_issue(
    app: &AppState,
    user: &CurrentUser,
    issue_id: &str,
) -> Result<issue::Model, Response> {
    let row = issue::Entity::find_by_id(issue_id.to_owned())
        .one(&app.db)
        .await
        .map_err(|e| internal("issue fetch failed", &e))?;
    let not_found = || {
        respond(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            "issue not found",
        )
    };
    let Some(row) = row else {
        return Err(not_found());
    };
    if !access::issue_visible(app, user, &row).await {
        return Err(not_found());
    }
    Ok(row)
}

fn pages_from_json(v: &serde_json::Value) -> Vec<i32> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(serde_json::Value::as_i64)
                .filter_map(|n| i32::try_from(n).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn to_view(issue_id: String, row: Option<issue_page_override::Model>) -> PageOverridesView {
    match row {
        Some(r) => PageOverridesView {
            issue_id,
            shift_pairing: r.shift_pairing,
            spread_pages: pages_from_json(&r.spread_pages),
            single_pages: pages_from_json(&r.single_pages),
            updated_at: Some(r.updated_at.to_rfc3339()),
        },
        None => PageOverridesView {
            issue_id,
            shift_pairing: false,
            spread_pages: Vec::new(),
            single_pages: Vec::new(),
            updated_at: None,
        },
    }
}

/// Sort + de-duplicate both lists and enforce the semantic rules garde
/// can't express: the lists are disjoint (a page can't be both forced
/// spread and forced single) and every index is inside the issue.
fn normalize(
    req: PutPageOverridesReq,
    page_count: Option<i32>,
) -> Result<(bool, Vec<i32>, Vec<i32>), Response> {
    let spread: BTreeSet<i32> = req.spread_pages.into_iter().collect();
    let single: BTreeSet<i32> = req.single_pages.into_iter().collect();
    if let Some(p) = spread.intersection(&single).next() {
        return Err(respond(
            StatusCode::UNPROCESSABLE_ENTITY,
            ApiErrorCode::Validation,
            format!("page {p} cannot be both a forced spread and a forced single"),
        ));
    }
    if let Some(count) = page_count.filter(|c| *c > 0)
        && let Some(p) = spread.iter().chain(single.iter()).find(|p| **p >= count)
    {
        return Err(respond(
            StatusCode::UNPROCESSABLE_ENTITY,
            ApiErrorCode::Validation,
            format!("page {p} is out of range (issue has {count} pages)"),
        ));
    }
    Ok((
        req.shift_pairing,
        spread.into_iter().collect(),
        single.into_iter().collect(),
    ))
}

async fn find_row(
    app: &AppState,
    user_id: uuid::Uuid,
    issue_id: &str,
) -> Result<Option<issue_page_override::Model>, Response> {
    issue_page_override::Entity::find_by_id((user_id, issue_id.to_owned()))
        .one(&app.db)
        .await
        .map_err(|e| internal("override fetch failed", &e))
}

async fn delete_row(app: &AppState, user_id: uuid::Uuid, issue_id: &str) -> Result<(), Response> {
    issue_page_override::Entity::delete_by_id((user_id, issue_id.to_owned()))
        .exec(&app.db)
        .await
        .map(|_| ())
        .map_err(|e| internal("override delete failed", &e))
}

#[utoipa::path(
    operation_id = "page_overrides_get",
    get,
    path = "/me/issues/{issue_id}/page-overrides",
    params(("issue_id" = String, Path,)),
    responses(
        (status = 200, body = PageOverridesView),
        (status = 404, description = "issue not found or not visible"),
    )
)]
#[handler]
pub async fn get_overrides(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(issue_id): AxPath<String>,
) -> Response {
    if let Err(resp) = visible_issue(&app, &user, &issue_id).await {
        return resp;
    }
    match find_row(&app, user.id, &issue_id).await {
        Ok(row) => Json(to_view(issue_id, row)).into_response(),
        Err(resp) => resp,
    }
}

#[utoipa::path(
    operation_id = "page_overrides_put",
    put,
    path = "/me/issues/{issue_id}/page-overrides",
    params(("issue_id" = String, Path,)),
    request_body = PutPageOverridesReq,
    responses(
        (status = 200, body = PageOverridesView),
        (status = 404, description = "issue not found or not visible"),
        (status = 422, description = "validation error"),
    )
)]
#[handler]
pub async fn put_overrides(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(issue_id): AxPath<String>,
    Validated(req): Validated<PutPageOverridesReq>,
) -> Response {
    let issue_row = match visible_issue(&app, &user, &issue_id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let (shift, spread, single) = match normalize(req, issue_row.page_count) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    // An all-default body is a reset: keep the table free of no-op rows.
    if !shift && spread.is_empty() && single.is_empty() {
        return match delete_row(&app, user.id, &issue_id).await {
            Ok(()) => Json(to_view(issue_id, None)).into_response(),
            Err(resp) => resp,
        };
    }
    let now = chrono::Utc::now().fixed_offset();
    let am = issue_page_override::ActiveModel {
        user_id: sea_orm::Set(user.id),
        issue_id: sea_orm::Set(issue_id.clone()),
        shift_pairing: sea_orm::Set(shift),
        spread_pages: sea_orm::Set(serde_json::json!(spread)),
        single_pages: sea_orm::Set(serde_json::json!(single)),
        updated_at: sea_orm::Set(now),
    };
    let res = issue_page_override::Entity::insert(am)
        .on_conflict(
            OnConflict::columns([
                issue_page_override::Column::UserId,
                issue_page_override::Column::IssueId,
            ])
            .update_columns([
                issue_page_override::Column::ShiftPairing,
                issue_page_override::Column::SpreadPages,
                issue_page_override::Column::SinglePages,
                issue_page_override::Column::UpdatedAt,
            ])
            .to_owned(),
        )
        .exec(&app.db)
        .await;
    if let Err(e) = res {
        return internal("override upsert failed", &e);
    }
    match find_row(&app, user.id, &issue_id).await {
        Ok(row) => Json(to_view(issue_id, row)).into_response(),
        Err(resp) => resp,
    }
}

#[utoipa::path(
    operation_id = "page_overrides_delete",
    delete,
    path = "/me/issues/{issue_id}/page-overrides",
    params(("issue_id" = String, Path,)),
    responses(
        (status = 204, description = "overrides cleared"),
        (status = 404, description = "issue not found or not visible"),
    )
)]
#[handler]
pub async fn delete_overrides(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(issue_id): AxPath<String>,
) -> Response {
    if let Err(resp) = visible_issue(&app, &user, &issue_id).await {
        return resp;
    }
    match delete_row(&app, user.id, &issue_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(resp) => resp,
    }
}
