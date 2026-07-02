//! Reconciliation surface: list pending removals, restore, confirm-removal.
//!
//! Library Scanner v1, Milestone 7 (spec §4.7).
//!
//! Routes:
//!   - `GET    /libraries/{id}/removed` — list issues + series with `removed_at` set
//!   - `POST   /issues/{id}/restore`     — clear `removed_at` (file must be back)
//!   - `POST   /issues/{id}/confirm-removal` — set `removal_confirmed_at` now
//!   - `POST   /series/{slug}/restore`   — clear `removed_at` on a series +
//!     its on-disk issues (folder must be back)
//!
//! All admin-only.

use axum::{
    Extension, Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, FixedOffset};
use entity::{issue, series};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, EntityTrait, FromQueryResult, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use shared::pagination::{decode_cursor, encode_cursor};
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
        .routes(routes!(list_removed))
        .routes(routes!(restore_issue))
        .routes(routes!(confirm_issue))
        .routes(routes!(restore_series))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RemovedListView {
    pub issues: Vec<RemovedIssueView>,
    /// Complete on the first page (cursor absent); empty on subsequent
    /// pages. Removed-series counts are small (one row per folder), so
    /// they don't paginate — only the per-file issue list does.
    pub series: Vec<RemovedSeriesView>,
    /// Cursor for the next page of `issues`; `None` on the last page.
    pub next_cursor: Option<String>,
    /// Total removed-issue count. First page only, mirroring the
    /// `CursorPage::total` convention.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_issues: Option<u64>,
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

/// Lean projection for the removed-issue listing — `issue::Model` carries
/// `comic_info_raw`, which is large and useless here (audit UX-11).
#[derive(FromQueryResult)]
struct RemovedIssueRow {
    id: String,
    slug: String,
    series_id: Uuid,
    file_path: String,
    removed_at: DateTime<FixedOffset>,
    removal_confirmed_at: Option<DateTime<FixedOffset>>,
}

#[derive(FromQueryResult)]
struct RemovedSeriesRow {
    id: Uuid,
    slug: String,
    name: String,
    folder_path: Option<String>,
    removed_at: DateTime<FixedOffset>,
    removal_confirmed_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Deserialize)]
pub struct RemovedListQuery {
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[utoipa::path(
    operation_id = "reconcile_list_removed",    get,
    path = "/libraries/{slug}/removed",
    params(
        ("slug" = String, Path,),
        ("limit" = Option<u64>, Query,),
        ("cursor" = Option<String>, Query,),
    ),
    responses(
        (status = 200, body = RemovedListView),
        (status = 400, description = "invalid cursor"),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
#[handler]
pub async fn list_removed(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(slug): AxPath<String>,
    Query(q): Query<RemovedListQuery>,
) -> impl IntoResponse {
    let lib = match crate::api::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let uuid = lib.id;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);

    let cursor: Option<(DateTime<FixedOffset>, String)> = match q.cursor.as_deref() {
        None => None,
        Some(c) => match decode_cursor::<(DateTime<FixedOffset>, String)>(c) {
            Ok(parsed) => Some(parsed),
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "validation.cursor",
                    "invalid cursor",
                );
            }
        },
    };
    let first_page = cursor.is_none();

    // Keyset-paginated issues (audit UX-11): a bulk removal can strand
    // thousands of rows, so the per-file list pages on
    // (removed_at DESC, id DESC) with an over-fetch of one to detect a
    // further page.
    let mut sel = issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(uuid))
        .filter(issue::Column::RemovedAt.is_not_null())
        .select_only()
        .column(issue::Column::Id)
        .column(issue::Column::Slug)
        .column(issue::Column::SeriesId)
        .column(issue::Column::FilePath)
        .column(issue::Column::RemovedAt)
        .column(issue::Column::RemovalConfirmedAt);
    if let Some((c_at, c_id)) = cursor {
        sel = sel.filter(
            Condition::any().add(issue::Column::RemovedAt.lt(c_at)).add(
                Condition::all()
                    .add(issue::Column::RemovedAt.eq(c_at))
                    .add(issue::Column::Id.lt(c_id)),
            ),
        );
    }
    let rows = match sel
        .order_by_desc(issue::Column::RemovedAt)
        .order_by_desc(issue::Column::Id)
        .limit(limit + 1)
        .into_model::<RemovedIssueRow>()
        .all(&app.db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "list removed issues failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let next_cursor = if rows.len() as u64 > limit {
        rows.get((limit - 1) as usize)
            .and_then(|r| encode_cursor(&(r.removed_at, r.id.clone())).ok())
    } else {
        None
    };
    let issues: Vec<RemovedIssueRow> = rows.into_iter().take(limit as usize).collect();

    let total_issues = if first_page {
        match issue::Entity::find()
            .filter(issue::Column::LibraryId.eq(uuid))
            .filter(issue::Column::RemovedAt.is_not_null())
            .count(&app.db)
            .await
        {
            Ok(n) => Some(n),
            Err(e) => {
                tracing::error!(error = %e, "count removed issues failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    } else {
        None
    };

    // Removed series ride on the first page only — one row per folder,
    // small by construction.
    let series_rows: Vec<RemovedSeriesRow> = if first_page {
        match series::Entity::find()
            .filter(series::Column::LibraryId.eq(uuid))
            .filter(series::Column::RemovedAt.is_not_null())
            .select_only()
            .column(series::Column::Id)
            .column(series::Column::Slug)
            .column(series::Column::Name)
            .column(series::Column::FolderPath)
            .column(series::Column::RemovedAt)
            .column(series::Column::RemovalConfirmedAt)
            .order_by_desc(series::Column::RemovedAt)
            .into_model::<RemovedSeriesRow>()
            .all(&app.db)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "list removed series failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    } else {
        Vec::new()
    };

    // Batch-fetch the parent series slug for each removed issue so the
    // payload carries the data the admin UI needs to build a restore URL
    // without one round trip per row.
    let issue_series_ids: Vec<Uuid> = issues.iter().map(|i| i.series_id).collect();
    let mut series_slugs: HashMap<Uuid, String> = HashMap::new();
    if !issue_series_ids.is_empty() {
        let parents: Vec<(Uuid, String)> = series::Entity::find()
            .filter(series::Column::Id.is_in(issue_series_ids))
            .select_only()
            .column(series::Column::Id)
            .column(series::Column::Slug)
            .into_tuple()
            .all(&app.db)
            .await
            .unwrap_or_default();
        for (id, slug) in parents {
            series_slugs.insert(id, slug);
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
                    removed_at: i.removed_at.to_rfc3339(),
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
                removed_at: s.removed_at.to_rfc3339(),
                removal_confirmed_at: s.removal_confirmed_at.map(|t| t.to_rfc3339()),
            })
            .collect(),
        next_cursor,
        total_issues,
    };
    Json(view).into_response()
}

#[utoipa::path(
    operation_id = "reconcile_restore_issue",    post,
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
#[handler]
pub async fn restore_issue(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
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

    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.issue.restore",
        target = ("issue", id.clone()),
        payload = serde_json::json!({"series_slug": series_slug, "issue_slug": issue_slug}),
    );

    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    operation_id = "reconcile_confirm_issue",    post,
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
#[handler]
pub async fn confirm_issue(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
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

    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.issue.confirm_removal",
        target = ("issue", id),
        payload = serde_json::json!({"series_slug": series_slug, "issue_slug": issue_slug}),
    );

    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    operation_id = "reconcile_restore_series",    post,
    path = "/series/{slug}/restore",
    params(("slug" = String, Path,)),
    responses(
        (status = 204, description = "restored"),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found"),
        (status = 409, description = "series is not removed, or its folder is still missing on disk"),
    )
)]
#[handler]
pub async fn restore_series(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let row = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if row.removed_at.is_none() {
        return error(
            StatusCode::CONFLICT,
            "conflict.not_removed",
            "series is not soft-deleted",
        );
    }
    if let Some(folder) = row.folder_path.as_deref()
        && !std::path::Path::new(folder).exists()
    {
        return error(
            StatusCode::CONFLICT,
            "conflict.folder_missing",
            "the series folder is still missing on disk; restore the folder first",
        );
    }

    // Restore the child issues whose files are actually back — the same
    // rule the scanner's reconcile pass applies (`(removed, present) →
    // restore`). Issues whose files are still missing stay soft-deleted;
    // a later scan (or per-issue restore) picks them up.
    let removed_issues: Vec<(String, String)> = match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(row.id))
        .filter(issue::Column::RemovedAt.is_not_null())
        .select_only()
        .column(issue::Column::Id)
        .column(issue::Column::FilePath)
        .into_tuple()
        .all(&app.db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, series_id = %row.id, "restore series: issue list failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    // Off-thread: one stat per removed file; a bulk-removed series can
    // hold hundreds.
    let restorable: Vec<String> = match tokio::task::spawn_blocking(move || {
        removed_issues
            .into_iter()
            .filter(|(_, path)| std::path::Path::new(path).exists())
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
    })
    .await
    {
        Ok(ids) => ids,
        Err(e) => {
            tracing::error!(error = %e, "restore series: stat join failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let restored_issue_count = restorable.len();
    for chunk in restorable.chunks(500) {
        if let Err(e) = issue::Entity::update_many()
            .col_expr(
                issue::Column::RemovedAt,
                Expr::value(Option::<DateTime<FixedOffset>>::None),
            )
            .col_expr(
                issue::Column::RemovalConfirmedAt,
                Expr::value(Option::<DateTime<FixedOffset>>::None),
            )
            .filter(issue::Column::Id.is_in(chunk.to_vec()))
            .exec(&app.db)
            .await
        {
            tracing::error!(error = %e, series_id = %row.id, "restore series: issue restore failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    }

    let series_id = row.id;
    let mut am: series::ActiveModel = row.into();
    am.removed_at = Set(None);
    am.removal_confirmed_at = Set(None);
    if let Err(e) = am.update(&app.db).await {
        tracing::error!(error = %e, series_id = %series_id, "restore series failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.series.restore",
        target = ("series", series_id.to_string()),
        payload = serde_json::json!({
            "series_slug": slug,
            "restored_issues": restored_issue_count,
        }),
    );

    StatusCode::NO_CONTENT.into_response()
}
