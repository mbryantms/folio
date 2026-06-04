//! `GET /admin/library-events` — observability-split M10. Cursor-paginated read
//! over the durable `library_events` manifest (M1-M4 writes it).
//!
//! Powers two surfaces: the per-batch / per-scan "what changed" drill-down
//! (filter by `batch_id` / `scan_run_id`) and the cross-library activity log
//! (M11). Filters are server-side query params — never a client `.filter()`
//! over a truncated page (list-pagination-completeness convention).
//!
//! Read-only admin GET → allowlisted in audit-check.

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, FixedOffset};
use entity::{library, library_event};
use sea_orm::{ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};
use shared::pagination::{CursorPage, decode_cursor, encode_cursor};
use std::collections::{HashMap, HashSet};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::auth::RequireAdmin;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(list_library_events))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LibraryEventView {
    pub id: String,
    pub library_id: String,
    /// Joined `library.name` so the cross-library activity log renders without
    /// an N+1. `null` if the library row was since deleted.
    pub library_name: Option<String>,
    pub scan_run_id: Option<String>,
    pub batch_id: Option<String>,
    pub category: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<String>,
    pub entity_label: Option<String>,
    pub action: String,
    pub severity: String,
    pub summary: String,
    pub detail: Option<serde_json::Value>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub library_id: Option<String>,
    #[serde(default)]
    pub batch_id: Option<String>,
    #[serde(default)]
    pub scan_run_id: Option<String>,
    /// Comma-separated category filter (`issue,series,thumbnail`).
    #[serde(default)]
    pub category: Option<String>,
    /// Comma-separated action filter (`added,updated,removed`).
    #[serde(default)]
    pub action: Option<String>,
    /// Comma-separated severity filter (`warning,error`).
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub cursor: Option<String>,
}

fn parse_uuid(
    raw: Option<&str>,
    field: &'static str,
) -> Result<Option<Uuid>, (StatusCode, &'static str, String)> {
    match raw {
        None | Some("") | Some("all") => Ok(None),
        Some(s) => Uuid::parse_str(s).map(Some).map_err(|_| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                field,
                format!("{field} must be a UUID"),
            )
        }),
    }
}

/// Split a comma list into a non-empty `Vec`, or `None` for absent/empty.
fn csv(raw: Option<&str>) -> Option<Vec<String>> {
    let v: Vec<String> = raw?
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    (!v.is_empty()).then_some(v)
}

#[utoipa::path(
    operation_id = "admin_library_events_list",
    get,
    path = "/admin/library-events",
    params(
        ("library_id" = Option<String>, Query,),
        ("batch_id" = Option<String>, Query,),
        ("scan_run_id" = Option<String>, Query,),
        ("category" = Option<String>, Query,),
        ("action" = Option<String>, Query,),
        ("severity" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
        ("cursor" = Option<String>, Query,),
    ),
    responses(
        (status = 200, body = shared::pagination::CursorPage<LibraryEventView>),
        (status = 403, description = "admin only"),
        (status = 422, description = "invalid filter value"),
    )
)]
#[handler]
pub async fn list_library_events(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    let library_id = match parse_uuid(q.library_id.as_deref(), "library_id") {
        Ok(v) => v,
        Err((s, c, m)) => return error(s, c, &m),
    };
    let batch_id = match parse_uuid(q.batch_id.as_deref(), "batch_id") {
        Ok(v) => v,
        Err((s, c, m)) => return error(s, c, &m),
    };
    let scan_run_id = match parse_uuid(q.scan_run_id.as_deref(), "scan_run_id") {
        Ok(v) => v,
        Err((s, c, m)) => return error(s, c, &m),
    };

    let cursor: Option<(DateTime<FixedOffset>, Uuid)> = match q.cursor.as_deref() {
        None => None,
        Some(c) => match decode_cursor::<(DateTime<FixedOffset>, Uuid)>(c) {
            Ok(parsed) => Some(parsed),
            Err(_) => return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor"),
        },
    };

    let mut query = library_event::Entity::find();
    if let Some(id) = library_id {
        query = query.filter(library_event::Column::LibraryId.eq(id));
    }
    if let Some(id) = batch_id {
        query = query.filter(library_event::Column::BatchId.eq(id));
    }
    if let Some(id) = scan_run_id {
        query = query.filter(library_event::Column::ScanRunId.eq(id));
    }
    if let Some(cats) = csv(q.category.as_deref()) {
        query = query.filter(library_event::Column::Category.is_in(cats));
    }
    if let Some(actions) = csv(q.action.as_deref()) {
        query = query.filter(library_event::Column::Action.is_in(actions));
    }
    if let Some(sevs) = csv(q.severity.as_deref()) {
        query = query.filter(library_event::Column::Severity.is_in(sevs));
    }
    if let Some((c_at, c_id)) = cursor {
        query = query.filter(
            Condition::any()
                .add(library_event::Column::CreatedAt.lt(c_at))
                .add(
                    Condition::all()
                        .add(library_event::Column::CreatedAt.eq(c_at))
                        .add(library_event::Column::Id.lt(c_id)),
                ),
        );
    }
    query = query
        .order_by_desc(library_event::Column::CreatedAt)
        .order_by_desc(library_event::Column::Id)
        .limit(limit + 1);

    let rows = match query.all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "admin list library_events failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let next_cursor = if rows.len() as u64 > limit {
        rows.get((limit - 1) as usize)
            .and_then(|r| encode_cursor(&(r.created_at, r.id)).ok())
    } else {
        None
    };
    let page: Vec<library_event::Model> = rows.into_iter().take(limit as usize).collect();

    // Resolve library names in one round-trip (cross-library activity log).
    let lib_ids: HashSet<Uuid> = page.iter().map(|r| r.library_id).collect();
    let names: HashMap<Uuid, String> = if lib_ids.is_empty() {
        HashMap::new()
    } else {
        library::Entity::find()
            .filter(library::Column::Id.is_in(lib_ids.into_iter().collect::<Vec<_>>()))
            .all(&app.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|l| (l.id, l.name))
            .collect()
    };

    let items: Vec<LibraryEventView> = page
        .into_iter()
        .map(|m| LibraryEventView {
            library_name: names.get(&m.library_id).cloned(),
            id: m.id.to_string(),
            library_id: m.library_id.to_string(),
            scan_run_id: m.scan_run_id.map(|u| u.to_string()),
            batch_id: m.batch_id.map(|u| u.to_string()),
            category: m.category,
            entity_type: m.entity_type,
            entity_id: m.entity_id,
            entity_label: m.entity_label,
            action: m.action,
            severity: m.severity,
            summary: m.summary,
            detail: m.detail,
            created_at: m.created_at.to_rfc3339(),
        })
        .collect();

    Json(CursorPage::<LibraryEventView>::paginated(
        items,
        next_cursor,
        None,
    ))
    .into_response()
}
