//! Reconciliation surface: list pending removals, restore, confirm-removal.
//!
//! Library Scanner v1, Milestone 7 (spec §4.7).
//!
//! Routes:
//!   - `GET    /libraries/{id}/removed` — list issues + series with `removed_at` set
//!   - `POST   /issues/{id}/restore`     — clear `removed_at` (file must be back)
//!   - `POST   /issues/{id}/confirm-removal` — set `removal_confirmed_at` now
//!
//! All admin-only.

use axum::{
    Json, Router,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use entity::{issue, series};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;

use crate::auth::RequireAdmin;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/libraries/{slug}/removed", get(list_removed))
        .route(
            "/series/{series_slug}/issues/{issue_slug}/restore",
            post(restore_issue),
        )
        .route(
            "/series/{series_slug}/issues/{issue_slug}/confirm-removal",
            post(confirm_issue),
        )
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RemovedListView {
    pub issues: Vec<RemovedIssueView>,
    pub series: Vec<RemovedSeriesView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RemovedIssueView {
    pub id: String,
    pub slug: String,
    pub series_id: String,
    /// URL slug of the parent series — paired with `slug` it forms the
    /// `/series/{series_slug}/issues/{slug}/(restore|confirm-removal)` path.
    pub series_slug: String,
    pub file_path: String,
    pub removed_at: String,
    pub removal_confirmed_at: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RemovedSeriesView {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub folder_path: Option<String>,
    pub removed_at: String,
    pub removal_confirmed_at: Option<String>,
}

#[utoipa::path(
    get,
    path = "/libraries/{slug}/removed",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = RemovedListView),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
pub async fn list_removed(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let lib = match crate::api::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let uuid = lib.id;

    let issues = match issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(uuid))
        .filter(issue::Column::RemovedAt.is_not_null())
        .all(&app.db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "list removed issues failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let series_rows = match series::Entity::find()
        .filter(series::Column::LibraryId.eq(uuid))
        .filter(series::Column::RemovedAt.is_not_null())
        .all(&app.db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "list removed series failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    // Batch-fetch the parent series slug for each removed issue so the
    // payload carries the data the admin UI needs to build a restore URL
    // without one round trip per row.
    let issue_series_ids: Vec<Uuid> = issues.iter().map(|i| i.series_id).collect();
    let mut series_slugs: HashMap<Uuid, String> = HashMap::new();
    if !issue_series_ids.is_empty() {
        let parents = series::Entity::find()
            .filter(series::Column::Id.is_in(issue_series_ids))
            .all(&app.db)
            .await
            .unwrap_or_default();
        for s in parents {
            series_slugs.insert(s.id, s.slug);
        }
    }

    let view = RemovedListView {
        issues: issues
            .into_iter()
            .map(|i| {
                let series_slug = series_slugs.get(&i.series_id).cloned().unwrap_or_default();
                RemovedIssueView {
                    id: i.id,
                    slug: i.slug,
                    series_id: i.series_id.to_string(),
                    series_slug,
                    file_path: i.file_path,
                    removed_at: i.removed_at.expect("filtered above").to_rfc3339(),
                    removal_confirmed_at: i.removal_confirmed_at.map(|t| t.to_rfc3339()),
                }
            })
            .collect(),
        series: series_rows
            .into_iter()
            .map(|s| RemovedSeriesView {
                id: s.id.to_string(),
                slug: s.slug,
                name: s.name,
                folder_path: s.folder_path,
                removed_at: s.removed_at.expect("filtered above").to_rfc3339(),
                removal_confirmed_at: s.removal_confirmed_at.map(|t| t.to_rfc3339()),
            })
            .collect(),
    };
    Json(view).into_response()
}

#[utoipa::path(
    post,
    path = "/series/{series_slug}/issues/{issue_slug}/restore",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    responses(
        (status = 204, description = "restored"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
        (status = 409, description = "file is still missing on disk"),
    )
)]
pub async fn restore_issue(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
) -> impl IntoResponse {
    let row = match crate::api::issues::find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let id = row.id.clone();
    if !std::path::Path::new(&row.file_path).exists() {
        return error(
            StatusCode::CONFLICT,
            "conflict.file_missing",
            "the file is still missing on disk; restore the file first",
        );
    }
    let mut am: issue::ActiveModel = row.into();
    am.removed_at = Set(None);
    am.removal_confirmed_at = Set(None);
    if let Err(e) = am.update(&app.db).await {
        tracing::error!(error = %e, issue_id = %id, "restore issue failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    post,
    path = "/series/{series_slug}/issues/{issue_slug}/confirm-removal",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    responses(
        (status = 204, description = "confirmed"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
    )
)]
pub async fn confirm_issue(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
) -> impl IntoResponse {
    let row = match crate::api::issues::find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let id = row.id.clone();
    if row.removed_at.is_none() {
        return error(
            StatusCode::CONFLICT,
            "conflict.not_removed",
            "issue is not soft-deleted",
        );
    }
    let issue_id = row.id.clone();
    let data_dir = app.cfg().data_path.clone();
    let mut am: issue::ActiveModel = row.into();
    am.removal_confirmed_at = Set(Some(chrono::Utc::now().fixed_offset()));
    if let Err(e) = am.update(&app.db).await {
        tracing::error!(error = %e, issue_id = %id, "confirm-removal failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    // M5: now that the issue is confirmed-removed, drop its on-disk thumbs.
    crate::library::thumbnails::wipe_issue_thumbs(&data_dir, &issue_id);
    StatusCode::NO_CONTENT.into_response()
}

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
