//! Add-to-Mylar admin endpoints (add-to-mylar 1.0).
//!
//! - `POST /admin/integrations/mylar/test` — check the stored URL + key.
//! - `POST /admin/integrations/mylar/requests` — queue missing entries of
//!   a reading list for adding to Mylar.
//! - `GET  /admin/integrations/mylar/requests?cbl_list_id=` — where that
//!   list's missing entries stand with Mylar.
//!
//! The handlers themselves never call Mylar except `test`: queuing writes
//! rows and wakes [`crate::mylar_control::sweep`].

use axum::{
    Extension, Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use garde::Validate;
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};
use shared::error::ApiErrorCode;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::extractors::Validated;
use super::respond;
use crate::auth::RequireAdmin;
use crate::middleware::RequestContext;
use crate::mylar_control::client::MylarClient;
use crate::mylar_control::requests::{self, AddSummary, ListStatus, MAX_ENTRY_IDS};
use crate::mylar_control::sweep;
use crate::record_admin_action;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(test_connection))
        .routes(routes!(list_requests, create_requests))
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
