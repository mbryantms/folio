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
use chrono::{DateTime, FixedOffset};
use entity::{library, library_health_issue};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    Set,
};
use serde::{Deserialize, Serialize};
use shared::pagination::{CursorPage, decode_cursor, encode_cursor};
use std::collections::HashMap;
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
        .routes(routes!(admin_list))
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

/// Cross-library health-issue row. Adds library context fields so the
/// admin findings table can render a Library column without N+1 lookups.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CrossLibHealthIssueView {
    #[serde(flatten)]
    pub base: HealthIssueView,
    pub library_id: String,
    pub library_name: String,
    pub library_slug: String,
}

#[derive(Debug, Deserialize)]
pub struct AdminListQuery {
    /// Restrict to one library (UUID). Omit / `all` for cross-library.
    #[serde(default)]
    pub library_id: Option<String>,
    /// Restrict to one `IssueKind` variant (e.g. `UnreadableArchive`).
    #[serde(default)]
    pub kind: Option<String>,
    /// `error` | `warning` | `info`. Unknown values 422.
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub include_resolved: Option<bool>,
    #[serde(default)]
    pub include_dismissed: Option<bool>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[utoipa::path(
    operation_id = "admin_health_issues_list",    get,
    path = "/admin/health-issues",
    params(
        ("library_id" = Option<String>, Query,),
        ("kind" = Option<String>, Query,),
        ("severity" = Option<String>, Query,),
        ("include_resolved" = Option<bool>, Query,),
        ("include_dismissed" = Option<bool>, Query,),
        ("limit" = Option<u64>, Query,),
        ("cursor" = Option<String>, Query,),
    ),
    responses(
        (status = 200, body = shared::pagination::CursorPage<CrossLibHealthIssueView>),
        (status = 403, description = "admin only"),
        (status = 422, description = "invalid filter value"),
    )
)]
#[handler]
pub async fn admin_list(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<AdminListQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    // Severity filter is bounded — reject unknown values rather than
    // silently returning an empty list (saves an operator from chasing
    // a typo).
    let severity_filter = match q.severity.as_deref() {
        None | Some("") | Some("all") => None,
        Some(s @ ("error" | "warning" | "info")) => Some(s.to_owned()),
        Some(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.severity",
                "severity must be one of: error, warning, info, all",
            );
        }
    };

    let library_filter = match q.library_id.as_deref() {
        None | Some("") | Some("all") => None,
        Some(raw) => match Uuid::parse_str(raw) {
            Ok(u) => Some(u),
            Err(_) => {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation.library_id",
                    "library_id must be a UUID or 'all'",
                );
            }
        },
    };

    let cursor: Option<(DateTime<FixedOffset>, Uuid)> = match q.cursor.as_deref() {
        None => None,
        Some(c) => match decode_cursor::<(DateTime<FixedOffset>, Uuid)>(c) {
            Ok(parsed) => Some(parsed),
            Err(_) => return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor"),
        },
    };

    let mut select = library_health_issue::Entity::find();
    if let Some(lib_id) = library_filter {
        select = select.filter(library_health_issue::Column::LibraryId.eq(lib_id));
    }
    if let Some(kind) = q.kind.as_deref().filter(|s| !s.is_empty()) {
        select = select.filter(library_health_issue::Column::Kind.eq(kind));
    }
    if let Some(sev) = severity_filter.as_deref() {
        select = select.filter(library_health_issue::Column::Severity.eq(sev));
    }
    if !q.include_resolved.unwrap_or(false) {
        select = select.filter(library_health_issue::Column::ResolvedAt.is_null());
    }
    if !q.include_dismissed.unwrap_or(false) {
        select = select.filter(library_health_issue::Column::DismissedAt.is_null());
    }
    if let Some((c_at, c_id)) = cursor {
        // Strictly-after-cursor in DESC order on (last_seen_at, id).
        select = select.filter(
            Condition::any()
                .add(library_health_issue::Column::LastSeenAt.lt(c_at))
                .add(
                    Condition::all()
                        .add(library_health_issue::Column::LastSeenAt.eq(c_at))
                        .add(library_health_issue::Column::Id.lt(c_id)),
                ),
        );
    }
    select = select
        .order_by_desc(library_health_issue::Column::LastSeenAt)
        .order_by_desc(library_health_issue::Column::Id)
        .limit(limit + 1);

    let rows = match select.all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "admin list health issues failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let next_cursor = if rows.len() as u64 > limit {
        rows.get((limit - 1) as usize)
            .and_then(|r| encode_cursor(&(r.last_seen_at, r.id)).ok())
    } else {
        None
    };
    let page: Vec<library_health_issue::Model> = rows.into_iter().take(limit as usize).collect();

    // Batch-resolve library names so the table can render a Library
    // column without one /libraries/{slug} request per row.
    let library_ids: std::collections::HashSet<Uuid> = page.iter().map(|r| r.library_id).collect();
    let library_meta: HashMap<Uuid, (String, String)> = if library_ids.is_empty() {
        HashMap::new()
    } else {
        match library::Entity::find()
            .filter(library::Column::Id.is_in(library_ids))
            .all(&app.db)
            .await
        {
            Ok(libs) => libs.into_iter().map(|l| (l.id, (l.name, l.slug))).collect(),
            Err(e) => {
                tracing::error!(error = %e, "library lookup for health issues failed");
                HashMap::new()
            }
        }
    };

    let items: Vec<CrossLibHealthIssueView> = page
        .into_iter()
        .map(|m| {
            let lib_id = m.library_id;
            let (name, slug) = library_meta
                .get(&lib_id)
                .cloned()
                .unwrap_or_else(|| (String::from("(deleted library)"), String::new()));
            CrossLibHealthIssueView {
                library_id: lib_id.to_string(),
                library_name: name,
                library_slug: slug,
                base: HealthIssueView::from(m),
            }
        })
        .collect();

    Json(CursorPage::<CrossLibHealthIssueView>::paginated(
        items,
        next_cursor,
        None,
    ))
    .into_response()
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
