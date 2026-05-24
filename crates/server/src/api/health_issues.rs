//! `GET /libraries/{id}/health-issues` and `POST .../{issue_id}/dismiss`.
//!
//! Library Scanner v1, Milestone 5 — surfaces the structured catalog populated
//! by [`crate::library::health::HealthCollector`] (spec §10).

use axum::{
    Extension, Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use entity::library_health_issue;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::auth::RequireAdmin;
use crate::middleware::RequestContext;
use crate::record_admin_action;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(dismiss))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct HealthIssueView {
    pub id: String,
    pub kind: String,
    pub severity: String,
    pub fingerprint: String,
    pub payload: serde_json::Value,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub resolved_at: Option<String>,
    pub dismissed_at: Option<String>,
    pub scan_id: Option<String>,
}

impl From<library_health_issue::Model> for HealthIssueView {
    fn from(m: library_health_issue::Model) -> Self {
        Self {
            id: m.id.to_string(),
            kind: m.kind,
            severity: m.severity,
            fingerprint: m.fingerprint,
            payload: m.payload,
            first_seen_at: m.first_seen_at.to_rfc3339(),
            last_seen_at: m.last_seen_at.to_rfc3339(),
            resolved_at: m.resolved_at.map(|t| t.to_rfc3339()),
            dismissed_at: m.dismissed_at.map(|t| t.to_rfc3339()),
            scan_id: m.scan_id.map(|u| u.to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub include_resolved: Option<bool>,
    #[serde(default)]
    pub include_dismissed: Option<bool>,
}

#[utoipa::path(
    operation_id = "health_issues_list",    get,
    path = "/libraries/{slug}/health-issues",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = Vec<HealthIssueView>),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(slug): AxPath<String>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let lib = match crate::api::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let uuid = lib.id;

    let mut select = library_health_issue::Entity::find()
        .filter(library_health_issue::Column::LibraryId.eq(uuid));
    if !q.include_resolved.unwrap_or(false) {
        select = select.filter(library_health_issue::Column::ResolvedAt.is_null());
    }
    if !q.include_dismissed.unwrap_or(false) {
        select = select.filter(library_health_issue::Column::DismissedAt.is_null());
    }
    select = select
        .order_by_desc(library_health_issue::Column::Severity)
        .order_by_desc(library_health_issue::Column::LastSeenAt);

    match select.all(&app.db).await {
        Ok(rows) => Json(
            rows.into_iter()
                .map(HealthIssueView::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "list health issues failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

#[utoipa::path(
    operation_id = "health_issues_dismiss",    post,
    path = "/libraries/{slug}/health-issues/{issue_id}/dismiss",
    params(
        ("slug" = String, Path,),
        ("issue_id" = String, Path,),
    ),
    responses(
        (status = 204, description = "dismissed"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
    )
)]
#[handler]
pub async fn dismiss(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath((lib_slug, issue_id)): AxPath<(String, String)>,
) -> impl IntoResponse {
    let lib = match crate::api::libraries::find_by_slug(&app.db, &lib_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let lib_uuid = lib.id;
    let Ok(issue_uuid) = Uuid::parse_str(&issue_id) else {
        return error(StatusCode::BAD_REQUEST, "validation", "invalid issue id");
    };
    let Ok(Some(row)) = library_health_issue::Entity::find_by_id(issue_uuid)
        .filter(library_health_issue::Column::LibraryId.eq(lib_uuid))
        .one(&app.db)
        .await
    else {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    };

    let kind = row.kind.clone();
    let mut am: library_health_issue::ActiveModel = row.into();
    am.dismissed_at = Set(Some(chrono::Utc::now().fixed_offset()));
    if let Err(e) = am.update(&app.db).await {
        tracing::error!(error = %e, "dismiss health issue failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.library.health_issue.dismiss",
        target = ("library_health_issue", issue_uuid.to_string()),
        payload = serde_json::json!({"library_id": lib_uuid.to_string(), "kind": kind}),
    );

    StatusCode::NO_CONTENT.into_response()
}
