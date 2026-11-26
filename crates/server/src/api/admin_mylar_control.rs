//! Add-to-Mylar admin endpoints (add-to-mylar 1.0).
//!
//! - `POST /admin/integrations/mylar/test` — check the stored URL + key.
//! - `POST /admin/integrations/mylar/requests` — queue missing entries of
//!   a reading list for adding to Mylar.
//! - `GET  /admin/integrations/mylar/requests?cbl_list_id=` — where that
//!   list's missing entries stand with Mylar.
//!
//! - `GET  /admin/integrations/mylar/wanted-check` — Mylar's live wanted
//!   list against the library.
//! - `POST /admin/integrations/mylar/search` — search the wanted list now.
//! - `POST /admin/integrations/mylar/series/{cv_series_id}/recheck` and
//!   `…/refresh` — rescan a series' files / re-read it from ComicVine.
//! - `POST /admin/integrations/mylar/issues/{cv_issue_id}/unwant` — set a
//!   Wanted issue back to Skipped.
//!
//! Queuing never calls Mylar: it writes rows and wakes
//! [`crate::mylar_control::sweep`]. The others are one admin click each
//! and make their one or two calls inline.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use garde::Validate;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use shared::error::ApiErrorCode;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::extractors::Validated;
use super::respond;
use crate::auth::RequireAdmin;
use crate::middleware::RequestContext;
use crate::mylar_control::client::{MylarClient, MylarError, Progress};
use crate::mylar_control::requests::{self, AddSummary, ListStatus, MAX_ENTRY_IDS};
use crate::mylar_control::sweep;
use crate::mylar_control::wanted::{self, WantedCheck};
use crate::record_admin_action;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(test_connection))
        .routes(routes!(list_requests, create_requests))
        .routes(routes!(wanted_check))
        .routes(routes!(search_now))
        .routes(routes!(recheck_series))
        .routes(routes!(refresh_series))
        .routes(routes!(unwant_issue))
}

/// Mylar did not do what was asked: say why, as a 502.
fn mylar_failed(e: &MylarError) -> Response {
    respond(
        StatusCode::BAD_GATEWAY,
        ApiErrorCode::ServiceUnavailable,
        e.to_string(),
    )
}

fn not_in_mylar(what: &str) -> Response {
    respond(
        StatusCode::NOT_FOUND,
        ApiErrorCode::NotFound,
        format!("Mylar does not have this {what}"),
    )
}

fn internal(e: &sea_orm::DbErr) -> Response {
    tracing::error!(error = %e, "mylar control query failed");
    respond(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Internal,
        "internal",
    )
}

fn not_configured() -> Response {
    respond(
        StatusCode::CONFLICT,
        ApiErrorCode::Conflict,
        "Add a Mylar address and API key first",
    )
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MylarTestResp {
    pub ok: bool,
    /// Mylar's version, when it answered.
    pub version: Option<String>,
    /// Why the check failed.
    pub error: Option<String>,
}

#[utoipa::path(
    operation_id = "admin_mylar_test", post,
    path = "/admin/integrations/mylar/test",
    responses(
        (status = 200, body = MylarTestResp, description = "the check ran; `ok` says how it went"),
        (status = 403, description = "admin only"),
        (status = 409, description = "no Mylar address or key stored"),
    )
)]
#[handler]
pub async fn test_connection(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let Some(client) = MylarClient::from_cfg(&app.cfg()) else {
        return not_configured();
    };
    let resp = match client.version().await {
        Ok(version) => MylarTestResp {
            ok: true,
            version: Some(version),
            error: None,
        },
        Err(e) => MylarTestResp {
            ok: false,
            version: None,
            error: Some(e.to_string()),
        },
    };
    Json(resp).into_response()
}

#[derive(Debug, Deserialize, Validate, utoipa::ToSchema)]
pub struct MylarAddReq {
    /// The reading list whose missing entries are meant.
    #[garde(skip)]
    pub cbl_list_id: Uuid,
    /// Entries to add; absent = every missing entry of the list. Entries
    /// that are not missing are ignored.
    #[garde(length(min = 1, max = MAX_ENTRY_IDS))]
    pub entry_ids: Option<Vec<Uuid>>,
    /// Also mark each chosen issue Wanted once Mylar has its series.
    #[garde(skip)]
    #[serde(default)]
    pub want_issues: bool,
}

#[utoipa::path(
    operation_id = "admin_mylar_add", post,
    path = "/admin/integrations/mylar/requests",
    request_body = MylarAddReq,
    responses(
        (status = 202, body = AddSummary, description = "queued; a worker sends them to Mylar"),
        (status = 403, description = "admin only"),
        (status = 404, description = "no such reading list"),
        (status = 409, description = "no Mylar address or key stored"),
        (status = 422, description = "validation failed"),
    )
)]
#[handler]
pub async fn create_requests(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Validated(req): Validated<MylarAddReq>,
) -> Response {
    if MylarClient::from_cfg(&app.cfg()).is_none() {
        return not_configured();
    }
    match entity::cbl_list::Entity::find_by_id(req.cbl_list_id)
        .one(&app.db)
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return respond(
                StatusCode::NOT_FOUND,
                ApiErrorCode::NotFound,
                "reading list not found",
            );
        }
        Err(e) => return internal(&e),
    }
    let summary = match requests::create(
        &app.db,
        actor.id,
        req.cbl_list_id,
        req.entry_ids.as_deref(),
        req.want_issues,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return internal(&e),
    };
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.integration.mylar.add",
        payload = serde_json::json!({
            "cbl_list_id": req.cbl_list_id,
            "all_missing": req.entry_ids.is_none(),
            "want_issues": req.want_issues,
            "series_requested": summary.series_requested,
            "issues_requested": summary.issues_requested,
        }),
    );
    if summary.series_requested + summary.issues_requested > 0 {
        // Send the first batch now instead of at the next minute tick.
        let state = app.clone();
        tokio::spawn(async move {
            sweep::run(&state).await;
        });
    }
    (StatusCode::ACCEPTED, Json(summary)).into_response()
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct MylarRequestsQuery {
    pub cbl_list_id: Uuid,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MylarListStatusView {
    /// A Mylar address and key are stored, so entries can be added.
    pub configured: bool,
    #[serde(flatten)]
    pub status: ListStatus,
}

#[utoipa::path(
    operation_id = "admin_mylar_requests", get,
    path = "/admin/integrations/mylar/requests",
    params(MylarRequestsQuery),
    responses(
        (status = 200, body = MylarListStatusView),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn list_requests(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<MylarRequestsQuery>,
) -> Response {
    let configured = MylarClient::from_cfg(&app.cfg()).is_some();
    match requests::list_status(&app.db, q.cbl_list_id).await {
        Ok(status) => Json(MylarListStatusView { configured, status }).into_response(),
        Err(e) => internal(&e),
    }
}

#[utoipa::path(
    operation_id = "admin_mylar_wanted_check", get,
    path = "/admin/integrations/mylar/wanted-check",
    responses(
        (status = 200, body = WantedCheck),
        (status = 403, description = "admin only"),
        (status = 409, description = "no Mylar address or key stored"),
        (status = 502, description = "Mylar did not answer"),
    )
)]
#[handler]
pub async fn wanted_check(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let Some(client) = MylarClient::from_cfg(&app.cfg()) else {
        return not_configured();
    };
    let list = match client.wanted().await {
        Ok(list) => list,
        Err(e) => return mylar_failed(&e),
    };
    match wanted::check(&app.db, list).await {
        Ok(check) => Json(check).into_response(),
        Err(e) => internal(&e),
    }
}

/// Whether Mylar finished, or took the request and is still working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MylarActionOutcome {
    Done,
    Started,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MylarActionResp {
    pub outcome: MylarActionOutcome,
}

impl From<Progress> for MylarActionResp {
    fn from(p: Progress) -> Self {
        Self {
            outcome: match p {
                Progress::Done => MylarActionOutcome::Done,
                Progress::Started => MylarActionOutcome::Started,
            },
        }
    }
}

#[utoipa::path(
    operation_id = "admin_mylar_search", post,
    path = "/admin/integrations/mylar/search",
    responses(
        (status = 200, body = MylarActionResp, description = "Mylar is searching its wanted list (or already finished)"),
        (status = 403, description = "admin only"),
        (status = 409, description = "no Mylar address or key stored"),
        (status = 502, description = "Mylar did not take the request"),
    )
)]
#[handler]
pub async fn search_now(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
) -> Response {
    let Some(client) = MylarClient::from_cfg(&app.cfg()) else {
        return not_configured();
    };
    let progress = match client.force_search().await {
        Ok(p) => p,
        Err(e) => return mylar_failed(&e),
    };
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.integration.mylar.search",
        payload = serde_json::json!({}),
    );
    Json(MylarActionResp::from(progress)).into_response()
}

#[utoipa::path(
    operation_id = "admin_mylar_recheck_series", post,
    path = "/admin/integrations/mylar/series/{cv_series_id}/recheck",
    params(("cv_series_id" = i64, Path, description = "ComicVine volume id")),
    responses(
        (status = 200, body = MylarActionResp, description = "Mylar rescanned the series' folder (or is still at it)"),
        (status = 403, description = "admin only"),
        (status = 404, description = "Mylar does not have this series"),
        (status = 409, description = "no Mylar address or key stored"),
        (status = 502, description = "Mylar did not take the request"),
    )
)]
#[handler]
pub async fn recheck_series(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(cv_series_id): Path<i64>,
) -> Response {
    let Some(client) = MylarClient::from_cfg(&app.cfg()) else {
        return not_configured();
    };
    match client.has_series(cv_series_id).await {
        Ok(true) => {}
        Ok(false) => return not_in_mylar("series"),
        Err(e) => return mylar_failed(&e),
    }
    let progress = match client.recheck_files(cv_series_id).await {
        Ok(p) => p,
        Err(e) => return mylar_failed(&e),
    };
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.integration.mylar.recheck",
        payload = serde_json::json!({ "cv_series_id": cv_series_id }),
    );
    Json(MylarActionResp::from(progress)).into_response()
}

#[utoipa::path(
    operation_id = "admin_mylar_refresh_series", post,
    path = "/admin/integrations/mylar/series/{cv_series_id}/refresh",
    params(("cv_series_id" = i64, Path, description = "ComicVine volume id")),
    responses(
        (status = 200, body = MylarActionResp, description = "Mylar queued a refresh of the series"),
        (status = 403, description = "admin only"),
        (status = 404, description = "Mylar does not have this series"),
        (status = 409, description = "no Mylar address or key stored"),
        (status = 502, description = "Mylar did not take the request"),
    )
)]
#[handler]
pub async fn refresh_series(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(cv_series_id): Path<i64>,
) -> Response {
    let Some(client) = MylarClient::from_cfg(&app.cfg()) else {
        return not_configured();
    };
    match client.has_series(cv_series_id).await {
        Ok(true) => {}
        Ok(false) => return not_in_mylar("series"),
        Err(e) => return mylar_failed(&e),
    }
    if let Err(e) = client.refresh_series(cv_series_id).await {
        return mylar_failed(&e);
    }
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.integration.mylar.refresh",
        payload = serde_json::json!({ "cv_series_id": cv_series_id }),
    );
    // Mylar only queues a refresh; it runs on Mylar's own schedule.
    Json(MylarActionResp::from(Progress::Started)).into_response()
}

#[utoipa::path(
    operation_id = "admin_mylar_unwant_issue", post,
    path = "/admin/integrations/mylar/issues/{cv_issue_id}/unwant",
    params(("cv_issue_id" = i64, Path, description = "ComicVine issue id")),
    responses(
        (status = 200, body = MylarActionResp, description = "the issue is Skipped in Mylar now"),
        (status = 403, description = "admin only"),
        (status = 404, description = "Mylar does not know this issue"),
        (status = 409, description = "the issue is not Wanted in Mylar, or no Mylar address or key is stored"),
        (status = 502, description = "Mylar did not take the request"),
    )
)]
#[handler]
pub async fn unwant_issue(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(cv_issue_id): Path<i64>,
) -> Response {
    let Some(client) = MylarClient::from_cfg(&app.cfg()) else {
        return not_configured();
    };
    // Mylar's unqueue writes a row for any id, known or not, and would
    // also demote a Downloaded issue: only a Wanted one goes through.
    match client.issue_status(cv_issue_id).await {
        Ok(Some(status)) if status == "Wanted" => {}
        Ok(Some(status)) => {
            return respond(
                StatusCode::CONFLICT,
                ApiErrorCode::Conflict,
                format!("Mylar has this issue as {status}, not Wanted"),
            );
        }
        Ok(None) => return not_in_mylar("issue"),
        Err(e) => return mylar_failed(&e),
    }
    if let Err(e) = client.unqueue_issue(cv_issue_id).await {
        return mylar_failed(&e);
    }
    // Folio's own record of the want: the request row, and the stored
    // snapshot's line (the next push replaces the snapshot anyway).
    let forget = async {
        entity::mylar_add_request::Entity::delete_many()
            .filter(entity::mylar_add_request::Column::CvIssueId.eq(cv_issue_id))
            .exec(&app.db)
            .await?;
        entity::mylar_wanted::Entity::delete_many()
            .filter(entity::mylar_wanted::Column::CvIssueId.eq(cv_issue_id))
            .exec(&app.db)
            .await
    };
    if let Err(e) = forget.await {
        return internal(&e);
    }
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.integration.mylar.unwant",
        payload = serde_json::json!({ "cv_issue_id": cv_issue_id }),
    );
    Json(MylarActionResp::from(Progress::Done)).into_response()
}
