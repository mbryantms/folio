//! Thumbnail-pipeline admin endpoints.
//!
//! Read-side:
//!   - `GET /admin/libraries/{slug}/thumbnails-status`
//!     Counts of generated / missing / errored cover thumbnails, page-strip
//!     readiness from disk, plus in-flight queue depth (whole-server, not
//!     per-library — apalis queues aren't filterable).
//!   - `GET /admin/libraries/{slug}/thumbnails-settings`
//!     Per-library `enabled`, `format`, and encoder quality. Used by the
//!     library settings tab to populate the settings card.
//!
//! Write-side:
//!   - `PATCH /admin/libraries/{slug}/thumbnails-settings`
//!     Updates `enabled`, `format`, and/or quality. Format/quality changes do
//!     not auto-regenerate; the admin runs force-recreate when ready.
//!     Audit: `admin.thumbnails.settings.update`.
//!   - `POST /admin/libraries/{slug}/thumbnails/generate-missing`
//!     Enqueue a cover `ThumbsJob` for every issue currently missing thumbs (or
//!     stamped at an older `thumbnail_version`). Does NOT wipe disk.
//!     Audit: `admin.thumbnails.generate_missing`.
//!   - `POST /admin/libraries/{slug}/thumbnails/generate-page-map`
//!     Enqueue lazy reader page-map strip thumbnails for every active issue.
//!     Does NOT wipe disk; existing strip files are skipped by the worker.
//!     Audit: `admin.thumbnails.generate_page_map`.
//!   - `POST /admin/libraries/{slug}/thumbnails/force-recreate`
//!     Wipes on-disk thumbs for every active issue, clears DB stamps, and
//!     enqueues fresh jobs. The only path that picks up a format change.
//!     Audit: `admin.thumbnails.force_recreate`.
//!   - `DELETE /admin/libraries/{slug}/thumbnails`
//!     Wipes on-disk thumbs and clears DB stamps. No re-enqueue.
//!     Audit: `admin.thumbnails.delete_all`.
//!   - `POST /admin/issues/{id}/regenerate-thumbnails`
//!     Single-issue force-recreate. Audit: `admin.thumbnails.regenerate.issue`.
//!
//! All endpoints require `role == "admin"`.

use axum::{
    Extension, Json, Router,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use entity::{issue, library};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, QueryFilter,
    QuerySelect, Set, Statement,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use crate::audit::{self, AuditEntry};
use crate::auth::RequireAdmin;
use crate::jobs::post_scan;
use crate::library::thumbnails::{self, THUMBNAIL_VERSION, ThumbFormat};
use crate::middleware::RequestContext;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/admin/libraries/{slug}/thumbnails-status",
            get(library_status),
        )
        .route(
            "/admin/libraries/{slug}/thumbnails-settings",
            get(get_settings).patch(update_settings),
        )
        .route(
            "/admin/libraries/{slug}/thumbnails/generate-missing",
            post(generate_missing),
        )
        .route(
            "/admin/libraries/{slug}/thumbnails/generate-page-map",
            post(generate_page_map),
        )
        .route(
            "/admin/libraries/{slug}/thumbnails/force-recreate",
            post(force_recreate),
        )
        .route("/admin/libraries/{slug}/thumbnails", delete(delete_all))
        // Series-scope thumbnail regen — admins can rebuild one book without
        // touching the rest of the library.
        .route(
            "/admin/series/{series_slug}/thumbnails/regenerate-cover",
            post(regenerate_series_cover),
        )
        .route(
            "/admin/series/{series_slug}/thumbnails/generate-page-map",
            post(generate_series_page_map),
        )
        .route(
            "/admin/series/{series_slug}/thumbnails/force-recreate-page-map",
            post(force_recreate_series_page_map),
        )
        // Issue-scope thumbnail regen. The cover route replaces the legacy
        // `/regenerate-thumbnails` endpoint, which conflated cover regen with
        // a full strip-dir wipe — no UI consumer existed, so this is a clean
        // rename + tightened semantics.
        .route(
            "/admin/series/{series_slug}/issues/{issue_slug}/thumbnails/regenerate-cover",
            post(regenerate_issue_cover),
        )
        .route(
            "/admin/series/{series_slug}/issues/{issue_slug}/thumbnails/generate-page-map",
            post(generate_issue_page_map),
        )
        .route(
            "/admin/series/{series_slug}/issues/{issue_slug}/thumbnails/force-recreate-page-map",
            post(force_recreate_issue_page_map),
        )
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ThumbnailsStatusView {
    /// Total active issues in the library.
    pub total: u64,
    /// Issues whose cover thumbnail is stamped done at the current
    /// `THUMBNAIL_VERSION`.
    pub generated: u64,
    /// Issues with `thumbnails_generated_at IS NULL` or
    /// `thumbnail_version < CURRENT` — cover work that the post-scan worker
    /// still needs to do.
    pub missing: u64,
    /// Issues whose last gen attempt set `thumbnails_error`.
    pub errored: u64,
    pub cover_generated: u64,
    pub cover_missing: u64,
    pub cover_queued: u64,
    pub cover_running: u64,
    pub cover_failed: u64,
    /// Total page-strip thumbnails needed for active issues with known page
    /// counts.
    pub page_total: u64,
    /// Existing page-strip thumbnails found on disk, across all known
    /// thumbnail formats.
    pub page_generated: u64,
    /// Page-strip thumbnails still missing from disk.
    pub page_missing: u64,
    pub page_map_generated: u64,
    pub page_map_missing: u64,
    pub page_map_queued: u64,
    pub page_map_running: u64,
    pub page_map_failed: u64,
    /// Server-wide queue depth of `post_scan_thumbs` jobs (not filtered by
    /// library — apalis-redis doesn't expose per-payload counts).
    pub in_flight: i64,
    /// Code-side current version; clients can detect bumps.
    pub current_version: i32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ThumbnailsSettingsView {
    pub enabled: bool,
    /// One of `webp` | `jpeg` | `png`.
    pub format: String,
    /// Cover thumbnail encoder quality, 0..=100.
    pub cover_quality: i32,
    /// Reader page thumbnail encoder quality, 0..=100.
    pub page_quality: i32,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateThumbnailsSettingsReq {
    #[serde(default)]
    pub enabled: Option<bool>,
    /// One of `webp` | `jpeg` | `png`. Validated case-insensitively.
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub cover_quality: Option<i32>,
    #[serde(default)]
    pub page_quality: Option<i32>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RegenerateResp {
    pub enqueued: usize,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeleteAllResp {
    pub deleted: usize,
}

#[derive(Debug, FromQueryResult)]
struct ThumbStatusCounts {
    total: i64,
    generated: i64,
    errored: i64,
}

#[derive(Debug, FromQueryResult)]
struct PageThumbIssueRow {
    id: String,
    page_count: Option<i32>,
}

#[utoipa::path(
    get,
    path = "/admin/libraries/{slug}/thumbnails-status",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = ThumbnailsStatusView),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
pub async fn library_status(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let lib = match crate::api::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let lib_id = lib.id;

    let counts = thumb_status_counts(&app, lib_id)
        .await
        .unwrap_or(ThumbStatusCounts {
            total: 0,
            generated: 0,
            errored: 0,
        });
    let total = counts.total.max(0) as u64;
    let generated = counts.generated.max(0) as u64;
    let errored = counts.errored.max(0) as u64;
    let missing = total.saturating_sub(generated);
    let (page_total, page_generated) = match page_thumb_status_counts(&app, lib_id).await {
        Ok(counts) => counts,
        Err(e) => {
            tracing::warn!(library_id = %lib_id, error = %e, "page thumbnail status probe failed");
            (0, 0)
        }
    };
    let page_missing = page_total.saturating_sub(page_generated);
    let (cover_queued, page_map_queued) = thumbnail_queued_counts(&app, lib_id).await;

    // Whole-queue depth via apalis. Same call admin_queue makes; cheap.
    let in_flight = match queue_depth(&app).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, "thumbs queue depth probe failed");
            0
        }
    };

    Json(ThumbnailsStatusView {
        total,
        generated,
        missing,
        errored,
        cover_generated: generated,
        cover_missing: missing,
        cover_queued,
        cover_running: 0,
        cover_failed: errored,
        page_total,
        page_generated,
        page_missing,
        page_map_generated: page_generated,
        page_map_missing: page_missing,
        page_map_queued,
        page_map_running: 0,
        page_map_failed: errored,
        in_flight,
        current_version: THUMBNAIL_VERSION,
    })
    .into_response()
}

async fn thumbnail_queued_counts(app: &AppState, lib_id: Uuid) -> (u64, u64) {
    let ids: Vec<String> = match issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .filter(issue::Column::State.eq("active"))
        .select_only()
        .column(issue::Column::Id)
        .into_tuple()
        .all(&app.db)
        .await
    {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(library_id = %lib_id, error = %e, "thumbnail queued status query failed");
            return (0, 0);
        }
    };
    let keys = app.thumb_job_keys().await;
    let mut cover = 0u64;
    let mut page_map = 0u64;
    for id in ids {
        if keys.contains(&format!("{id}:Cover")) || keys.contains(&format!("{id}:CoverAndStrip")) {
            cover += 1;
        }
        if keys.contains(&format!("{id}:Strip")) || keys.contains(&format!("{id}:CoverAndStrip")) {
            page_map += 1;
        }
    }
    (cover, page_map)
}

#[utoipa::path(
    get,
    path = "/admin/libraries/{slug}/thumbnails-settings",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = ThumbnailsSettingsView),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
pub async fn get_settings(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let lib = match crate::api::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    Json(ThumbnailsSettingsView {
        enabled: lib.thumbnails_enabled,
        format: lib.thumbnail_format,
        cover_quality: lib.thumbnail_cover_quality,
        page_quality: lib.thumbnail_page_quality,
    })
    .into_response()
}

#[utoipa::path(
    patch,
    path = "/admin/libraries/{slug}/thumbnails-settings",
    params(("slug" = String, Path,)),
    request_body = UpdateThumbnailsSettingsReq,
    responses(
        (status = 200, body = ThumbnailsSettingsView),
        (status = 400, description = "invalid format value"),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
pub async fn update_settings(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(slug): AxPath<String>,
    Json(req): Json<UpdateThumbnailsSettingsReq>,
) -> impl IntoResponse {
    let row = match crate::api::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let lib_id = row.id;

    // Validate the format up-front so a bad value short-circuits before we
    // touch the DB. Empty string is treated as "not provided".
    let normalized_format = match req.format.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => match ThumbFormat::parse(&s.to_ascii_lowercase()) {
            Some(f) => Some(f.as_str().to_owned()),
            None => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "validation.thumbnail_format",
                    "format must be one of: webp, jpeg, png",
                );
            }
        },
        _ => None,
    };
    let validate_quality =
        |value: Option<i32>, field: &str| -> Result<Option<i32>, axum::response::Response> {
            match value {
                Some(v) if !(0..=100).contains(&v) => Err(error(
                    StatusCode::BAD_REQUEST,
                    "validation.thumbnail_quality",
                    &format!("{field} must be between 0 and 100"),
                )),
                Some(v) => Ok(Some(v)),
                None => Ok(None),
            }
        };
    let cover_quality = match validate_quality(req.cover_quality, "cover_quality") {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let page_quality = match validate_quality(req.page_quality, "page_quality") {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let prev_enabled = row.thumbnails_enabled;
    let prev_format = row.thumbnail_format.clone();
    let prev_cover_quality = row.thumbnail_cover_quality;
    let prev_page_quality = row.thumbnail_page_quality;

    let mut am: library::ActiveModel = row.into();
    if let Some(b) = req.enabled {
        am.thumbnails_enabled = Set(b);
    }
    if let Some(f) = normalized_format.clone() {
        am.thumbnail_format = Set(f);
    }
    if let Some(q) = cover_quality {
        am.thumbnail_cover_quality = Set(q);
    }
    if let Some(q) = page_quality {
        am.thumbnail_page_quality = Set(q);
    }
    am.updated_at = Set(chrono::Utc::now().fixed_offset());

    let updated = match am.update(&app.db).await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(library_id = %lib_id, error = %e, "update thumb settings failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.thumbnails.settings.update",
            target_type: Some("library"),
            target_id: Some(lib_id.to_string()),
            payload: serde_json::json!({
                "prev": {
                    "enabled": prev_enabled,
                    "format": prev_format,
                    "cover_quality": prev_cover_quality,
                    "page_quality": prev_page_quality,
                },
                "next": {
                    "enabled": updated.thumbnails_enabled,
                    "format": updated.thumbnail_format,
                    "cover_quality": updated.thumbnail_cover_quality,
                    "page_quality": updated.thumbnail_page_quality,
                },
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(ThumbnailsSettingsView {
        enabled: updated.thumbnails_enabled,
        format: updated.thumbnail_format,
        cover_quality: updated.thumbnail_cover_quality,
        page_quality: updated.thumbnail_page_quality,
    })
    .into_response()
}

#[utoipa::path(
    post,
    path = "/admin/libraries/{slug}/thumbnails/generate-missing",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = RegenerateResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
        (status = 409, description = "thumbnails disabled"),
    )
)]
pub async fn generate_missing(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let lib = match crate::api::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let lib_id = lib.id;
    if !lib.thumbnails_enabled {
        return error(
            StatusCode::CONFLICT,
            "thumbnails.disabled",
            "thumbnails are disabled for this library",
        );
    }

    let enqueued = post_scan::enqueue_pending_for_library(&app, lib_id).await;

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.thumbnails.generate_missing",
            target_type: Some("library"),
            target_id: Some(lib_id.to_string()),
            payload: serde_json::json!({ "enqueued": enqueued }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(RegenerateResp { enqueued }).into_response()
}

#[utoipa::path(
    post,
    path = "/admin/libraries/{slug}/thumbnails/generate-page-map",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = RegenerateResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
        (status = 409, description = "thumbnails disabled"),
    )
)]
pub async fn generate_page_map(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let lib = match crate::api::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let lib_id = lib.id;
    if !lib.thumbnails_enabled {
        return error(
            StatusCode::CONFLICT,
            "thumbnails.disabled",
            "thumbnails are disabled for this library",
        );
    }

    let enqueued = post_scan::enqueue_strips_for_library(&app, lib_id).await;

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.thumbnails.generate_page_map",
            target_type: Some("library"),
            target_id: Some(lib_id.to_string()),
            payload: serde_json::json!({ "enqueued": enqueued }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(RegenerateResp { enqueued }).into_response()
}

#[utoipa::path(
    post,
    path = "/admin/libraries/{slug}/thumbnails/force-recreate",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = RegenerateResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
        (status = 409, description = "thumbnails disabled"),
    )
)]
pub async fn force_recreate(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let lib = match crate::api::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let lib_id = lib.id;
    if !lib.thumbnails_enabled {
        return error(
            StatusCode::CONFLICT,
            "thumbnails.disabled",
            "thumbnails are disabled for this library",
        );
    }

    let issue_ids: Vec<String> = match issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .filter(issue::Column::State.eq("active"))
        .select_only()
        .column(issue::Column::Id)
        .into_tuple::<String>()
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(library_id = %lib_id, error = %e, "force-recreate: query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let wiped = wipe_issue_thumb_ids(app.cfg.data_path.clone(), issue_ids).await;
    if let Err(e) = clear_thumb_state_for_library(&app, lib_id, true).await {
        tracing::warn!(library_id = %lib_id, error = %e, "force-recreate: bulk clear failed");
    }

    let enqueued = post_scan::enqueue_pending_for_library(&app, lib_id).await;

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.thumbnails.force_recreate",
            target_type: Some("library"),
            target_id: Some(lib_id.to_string()),
            payload: serde_json::json!({ "enqueued": enqueued, "wiped": wiped }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(RegenerateResp { enqueued }).into_response()
}

#[utoipa::path(
    delete,
    path = "/admin/libraries/{slug}/thumbnails",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = DeleteAllResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
pub async fn delete_all(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let lib = match crate::api::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let lib_id = lib.id;

    // Sweep every issue in the library — active *and* removed — so the
    // delete-all guarantees a clean slate. Removed issues normally get
    // their thumbs swept on confirm-removal, but a partial wipe could
    // still leave files behind; this is the place the admin asks for a
    // full reset.
    let issue_ids: Vec<String> = match issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .select_only()
        .column(issue::Column::Id)
        .into_tuple::<String>()
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(library_id = %lib_id, error = %e, "delete-all: query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let deleted = wipe_issue_thumb_ids(app.cfg.data_path.clone(), issue_ids).await;
    if let Err(e) = clear_thumb_state_for_library(&app, lib_id, false).await {
        tracing::warn!(library_id = %lib_id, error = %e, "delete-all: bulk clear failed");
    }

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.thumbnails.delete_all",
            target_type: Some("library"),
            target_id: Some(lib_id.to_string()),
            payload: serde_json::json!({ "deleted": deleted }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(DeleteAllResp { deleted }).into_response()
}

// ───────── series-scope thumbnail regen ─────────

/// Wipe every cover file in a series and re-enqueue cover jobs for each
/// active issue. Strip subtree is preserved; strips refill lazily through
/// the inline-fallback path or by a separate `force-recreate-page-map`
/// call. Audit: `admin.thumbnails.regenerate.series_cover`.
#[utoipa::path(
    post,
    path = "/admin/series/{series_slug}/thumbnails/regenerate-cover",
    params(("series_slug" = String, Path,)),
    responses(
        (status = 200, body = RegenerateResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found"),
        (status = 409, description = "thumbnails disabled"),
    )
)]
pub async fn regenerate_series_cover(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(series_slug): AxPath<String>,
) -> impl IntoResponse {
    let series_row = match crate::api::series::find_by_slug(&app.db, &series_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let lib = match library::Entity::find_by_id(series_row.library_id)
        .one(&app.db)
        .await
    {
        Ok(Some(l)) => l,
        Ok(None) => {
            return error(StatusCode::NOT_FOUND, "not_found", "library not found");
        }
        Err(e) => {
            tracing::error!(library_id = %series_row.library_id, error = %e, "regenerate-series-cover: library lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if !lib.thumbnails_enabled {
        return error(
            StatusCode::CONFLICT,
            "thumbnails.disabled",
            "thumbnails are disabled for this library",
        );
    }

    let issue_ids: Vec<String> = match issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(series_row.library_id))
        .filter(issue::Column::SeriesId.eq(series_row.id))
        .filter(issue::Column::State.eq("active"))
        .select_only()
        .column(issue::Column::Id)
        .into_tuple::<String>()
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(series_id = %series_row.id, error = %e, "regenerate-series-cover: query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    parallel_wipe_issue_files(
        app.cfg.data_path.clone(),
        issue_ids.clone(),
        WipeScope::Cover,
    )
    .await;
    if let Err(e) = clear_thumb_state_for_series(&app, series_row.id).await {
        tracing::warn!(series_id = %series_row.id, error = %e, "regenerate-series-cover: stamp clear failed");
    }

    let mut enqueued = 0usize;
    for id in issue_ids {
        if post_scan::enqueue_thumb_job(&app, post_scan::ThumbsJob::cover(id)).await {
            enqueued += 1;
        }
    }

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.thumbnails.regenerate.series_cover",
            target_type: Some("series"),
            target_id: Some(series_row.id.to_string()),
            payload: serde_json::json!({ "enqueued": enqueued }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(RegenerateResp { enqueued }).into_response()
}

/// Fill-only enqueue: walk every active issue in the series and push a
/// `Strip` job. The worker skips pages whose strip files already exist, so
/// repeated calls are cheap and idempotent. Audit:
/// `admin.thumbnails.generate_page_map.series`.
#[utoipa::path(
    post,
    path = "/admin/series/{series_slug}/thumbnails/generate-page-map",
    params(("series_slug" = String, Path,)),
    responses(
        (status = 200, body = RegenerateResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found"),
        (status = 409, description = "thumbnails disabled"),
    )
)]
pub async fn generate_series_page_map(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(series_slug): AxPath<String>,
) -> impl IntoResponse {
    let series_row = match crate::api::series::find_by_slug(&app.db, &series_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let lib = match library::Entity::find_by_id(series_row.library_id)
        .one(&app.db)
        .await
    {
        Ok(Some(l)) => l,
        Ok(None) => {
            return error(StatusCode::NOT_FOUND, "not_found", "library not found");
        }
        Err(e) => {
            tracing::error!(library_id = %series_row.library_id, error = %e, "generate-series-page-map: library lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if !lib.thumbnails_enabled {
        return error(
            StatusCode::CONFLICT,
            "thumbnails.disabled",
            "thumbnails are disabled for this library",
        );
    }

    let enqueued =
        post_scan::enqueue_strips_for_series(&app, series_row.library_id, series_row.id).await;

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.thumbnails.generate_page_map.series",
            target_type: Some("series"),
            target_id: Some(series_row.id.to_string()),
            payload: serde_json::json!({ "enqueued": enqueued }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(RegenerateResp { enqueued }).into_response()
}

/// Destructive: delete every strip subtree in the series, then enqueue a
/// `Strip` job per active issue so each page is re-encoded from the
/// archive. Cover files are preserved. Audit:
/// `admin.thumbnails.force_recreate.series_page_map`.
#[utoipa::path(
    post,
    path = "/admin/series/{series_slug}/thumbnails/force-recreate-page-map",
    params(("series_slug" = String, Path,)),
    responses(
        (status = 200, body = RegenerateResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found"),
        (status = 409, description = "thumbnails disabled"),
    )
)]
pub async fn force_recreate_series_page_map(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(series_slug): AxPath<String>,
) -> impl IntoResponse {
    let series_row = match crate::api::series::find_by_slug(&app.db, &series_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let lib = match library::Entity::find_by_id(series_row.library_id)
        .one(&app.db)
        .await
    {
        Ok(Some(l)) => l,
        Ok(None) => {
            return error(StatusCode::NOT_FOUND, "not_found", "library not found");
        }
        Err(e) => {
            tracing::error!(library_id = %series_row.library_id, error = %e, "force-recreate-series-page-map: library lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if !lib.thumbnails_enabled {
        return error(
            StatusCode::CONFLICT,
            "thumbnails.disabled",
            "thumbnails are disabled for this library",
        );
    }

    let issue_ids: Vec<String> = match issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(series_row.library_id))
        .filter(issue::Column::SeriesId.eq(series_row.id))
        .filter(issue::Column::State.eq("active"))
        .select_only()
        .column(issue::Column::Id)
        .into_tuple::<String>()
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(series_id = %series_row.id, error = %e, "force-recreate-series-page-map: query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    parallel_wipe_issue_files(
        app.cfg.data_path.clone(),
        issue_ids.clone(),
        WipeScope::Strips,
    )
    .await;

    let mut enqueued = 0usize;
    for id in issue_ids {
        if post_scan::enqueue_thumb_job(&app, post_scan::ThumbsJob::strip(id)).await {
            enqueued += 1;
        }
    }

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.thumbnails.force_recreate.series_page_map",
            target_type: Some("series"),
            target_id: Some(series_row.id.to_string()),
            payload: serde_json::json!({ "enqueued": enqueued }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(RegenerateResp { enqueued }).into_response()
}

// ───────── issue-scope thumbnail regen ─────────

/// Wipe one issue's cover thumbnail (every known extension), clear its
/// stamp columns, and enqueue a fresh `Cover` job. Strip subtree is
/// preserved. Audit: `admin.thumbnails.regenerate.issue_cover`.
#[utoipa::path(
    post,
    path = "/admin/series/{series_slug}/issues/{issue_slug}/thumbnails/regenerate-cover",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    responses(
        (status = 200, body = RegenerateResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
        (status = 409, description = "thumbnails disabled"),
    )
)]
pub async fn regenerate_issue_cover(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
) -> impl IntoResponse {
    let row = match crate::api::issues::find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Some(resp) =
        check_thumbnails_enabled(&app, row.library_id, "regenerate-issue-cover").await
    {
        return resp;
    }

    thumbnails::wipe_issue_cover(&app.cfg.data_path, &row.id);
    clear_thumb_state(&app, &row).await;

    let enqueued =
        if post_scan::enqueue_thumb_job(&app, post_scan::ThumbsJob::cover(row.id.clone())).await {
            1
        } else {
            0
        };

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.thumbnails.regenerate.issue_cover",
            target_type: Some("issue"),
            target_id: Some(row.id.clone()),
            payload: serde_json::json!({ "enqueued": enqueued }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(RegenerateResp { enqueued }).into_response()
}

/// Fill-only `Strip` enqueue for a single issue. Worker skips pages whose
/// strip files already exist, so calling this when nothing's missing is a
/// no-op. Audit: `admin.thumbnails.generate_page_map.issue`.
#[utoipa::path(
    post,
    path = "/admin/series/{series_slug}/issues/{issue_slug}/thumbnails/generate-page-map",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    responses(
        (status = 200, body = RegenerateResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
        (status = 409, description = "thumbnails disabled"),
    )
)]
pub async fn generate_issue_page_map(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
) -> impl IntoResponse {
    let row = match crate::api::issues::find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Some(resp) =
        check_thumbnails_enabled(&app, row.library_id, "generate-issue-page-map").await
    {
        return resp;
    }

    let enqueued =
        if post_scan::enqueue_thumb_job(&app, post_scan::ThumbsJob::strip(row.id.clone())).await {
            1
        } else {
            0
        };

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.thumbnails.generate_page_map.issue",
            target_type: Some("issue"),
            target_id: Some(row.id.clone()),
            payload: serde_json::json!({ "enqueued": enqueued }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(RegenerateResp { enqueued }).into_response()
}

/// Destructive: wipe one issue's strip subtree, then enqueue a fresh
/// `Strip` job. Cover file is preserved. Audit:
/// `admin.thumbnails.force_recreate.issue_page_map`.
#[utoipa::path(
    post,
    path = "/admin/series/{series_slug}/issues/{issue_slug}/thumbnails/force-recreate-page-map",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    responses(
        (status = 200, body = RegenerateResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
        (status = 409, description = "thumbnails disabled"),
    )
)]
pub async fn force_recreate_issue_page_map(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
) -> impl IntoResponse {
    let row = match crate::api::issues::find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Some(resp) =
        check_thumbnails_enabled(&app, row.library_id, "force-recreate-issue-page-map").await
    {
        return resp;
    }

    thumbnails::wipe_issue_strips(&app.cfg.data_path, &row.id);

    let enqueued =
        if post_scan::enqueue_thumb_job(&app, post_scan::ThumbsJob::strip(row.id.clone())).await {
            1
        } else {
            0
        };

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.thumbnails.force_recreate.issue_page_map",
            target_type: Some("issue"),
            target_id: Some(row.id.clone()),
            payload: serde_json::json!({ "enqueued": enqueued }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(RegenerateResp { enqueued }).into_response()
}

// ───────── helpers ─────────

async fn clear_thumb_state(app: &AppState, row: &issue::Model) {
    let mut am: issue::ActiveModel = row.clone().into();
    am.thumbnails_generated_at = Set(None);
    am.thumbnail_version = Set(0);
    am.thumbnails_error = Set(None);
    am.updated_at = Set(chrono::Utc::now().fixed_offset());
    if let Err(e) = am.update(&app.db).await {
        tracing::warn!(issue_id = %row.id, error = %e, "clear thumb state failed");
    }
}

async fn clear_thumb_state_for_library(
    app: &AppState,
    lib_id: Uuid,
    active_only: bool,
) -> Result<u64, sea_orm::DbErr> {
    let now = chrono::Utc::now().fixed_offset();
    let mut update = issue::Entity::update_many()
        .set(issue::ActiveModel {
            thumbnails_generated_at: Set(None),
            thumbnail_version: Set(0),
            thumbnails_error: Set(None),
            updated_at: Set(now),
            ..Default::default()
        })
        .filter(issue::Column::LibraryId.eq(lib_id));
    if active_only {
        update = update.filter(issue::Column::State.eq("active"));
    }
    update.exec(&app.db).await.map(|res| res.rows_affected)
}

async fn thumb_status_counts(
    app: &AppState,
    lib_id: Uuid,
) -> Result<ThumbStatusCounts, sea_orm::DbErr> {
    let stmt = Statement::from_sql_and_values(
        app.db.get_database_backend(),
        r#"
        SELECT
            COUNT(*)::BIGINT AS total,
            COUNT(*) FILTER (
                WHERE thumbnails_generated_at IS NOT NULL
                  AND thumbnail_version >= $2
            )::BIGINT AS generated,
            COUNT(*) FILTER (WHERE thumbnails_error IS NOT NULL)::BIGINT AS errored
        FROM issues
        WHERE library_id = $1
          AND state = 'active'
        "#,
        [lib_id.into(), THUMBNAIL_VERSION.into()],
    );
    Ok(ThumbStatusCounts::find_by_statement(stmt)
        .one(&app.db)
        .await?
        .unwrap_or(ThumbStatusCounts {
            total: 0,
            generated: 0,
            errored: 0,
        }))
}

async fn page_thumb_status_counts(
    app: &AppState,
    lib_id: Uuid,
) -> Result<(u64, u64), sea_orm::DbErr> {
    let rows = issue::Entity::find()
        .select_only()
        .column(issue::Column::Id)
        .column(issue::Column::PageCount)
        .filter(issue::Column::LibraryId.eq(lib_id))
        .filter(issue::Column::State.eq("active"))
        .into_model::<PageThumbIssueRow>()
        .all(&app.db)
        .await?;

    let data_dir = app.cfg.data_path.clone();
    let counts = tokio::task::spawn_blocking(move || {
        let mut total = 0u64;
        let mut generated = 0u64;
        for row in rows {
            let page_total = row.page_count.unwrap_or(0).max(0) as u64;
            if page_total == 0 {
                continue;
            }
            total += page_total;
            match thumbnails::count_existing_strips(&data_dir, &row.id) {
                Ok(existing) => generated += (existing as u64).min(page_total),
                Err(e) => {
                    tracing::warn!(
                        issue_id = %row.id,
                        error = %e,
                        "page thumbnail count failed"
                    );
                }
            }
        }
        (total, generated)
    })
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, "page thumbnail count worker failed");
        (0, 0)
    });

    Ok(counts)
}

async fn wipe_issue_thumb_ids(data_dir: PathBuf, issue_ids: Vec<String>) -> usize {
    parallel_wipe_issue_files(data_dir, issue_ids, WipeScope::All).await
}

/// Which on-disk thumbnail files a parallel wipe should target. `Cover`
/// preserves the strip subtree (used by per-issue / per-series cover
/// regen); `Strips` preserves the cover (used by force-recreate page-map);
/// `All` is the historic library-wide behavior.
#[derive(Debug, Clone, Copy)]
enum WipeScope {
    Cover,
    Strips,
    All,
}

async fn parallel_wipe_issue_files(
    data_dir: PathBuf,
    issue_ids: Vec<String>,
    scope: WipeScope,
) -> usize {
    if issue_ids.is_empty() {
        return 0;
    }
    let total = issue_ids.len();
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8)
        .min(total);
    let chunk_size = total.div_ceil(workers);
    let mut handles = Vec::with_capacity(workers);
    for chunk in issue_ids.chunks(chunk_size) {
        let data_dir = data_dir.clone();
        let chunk = chunk.to_owned();
        handles.push(tokio::task::spawn_blocking(move || {
            for id in &chunk {
                match scope {
                    WipeScope::Cover => thumbnails::wipe_issue_cover(&data_dir, id),
                    WipeScope::Strips => thumbnails::wipe_issue_strips(&data_dir, id),
                    WipeScope::All => thumbnails::wipe_issue_thumbs(&data_dir, id),
                }
            }
            chunk.len()
        }));
    }
    let mut wiped = 0usize;
    for handle in handles {
        match handle.await {
            Ok(n) => wiped += n,
            Err(e) => tracing::warn!(error = %e, "thumbnail wipe worker failed"),
        }
    }
    wiped
}

/// Bulk-clear `thumbnails_generated_at` / `thumbnail_version` /
/// `thumbnails_error` for every active issue in a series. Mirrors
/// `clear_thumb_state_for_library` scoped to one series.
async fn clear_thumb_state_for_series(
    app: &AppState,
    series_id: Uuid,
) -> Result<u64, sea_orm::DbErr> {
    let now = chrono::Utc::now().fixed_offset();
    issue::Entity::update_many()
        .set(issue::ActiveModel {
            thumbnails_generated_at: Set(None),
            thumbnail_version: Set(0),
            thumbnails_error: Set(None),
            updated_at: Set(now),
            ..Default::default()
        })
        .filter(issue::Column::SeriesId.eq(series_id))
        .filter(issue::Column::State.eq("active"))
        .exec(&app.db)
        .await
        .map(|res| res.rows_affected)
}

/// Issue-scope helper: load the library, return a `409 thumbnails.disabled`
/// response when the toggle is off, otherwise `None`. Avoids repeating the
/// same look-up + branch in every issue/series handler. Returns 404 if the
/// library row was deleted between the issue load and this call.
async fn check_thumbnails_enabled(
    app: &AppState,
    library_id: Uuid,
    op: &'static str,
) -> Option<axum::response::Response> {
    match library::Entity::find_by_id(library_id).one(&app.db).await {
        Ok(Some(l)) if !l.thumbnails_enabled => Some(error(
            StatusCode::CONFLICT,
            "thumbnails.disabled",
            "thumbnails are disabled for this library",
        )),
        Ok(Some(_)) => None,
        Ok(None) => Some(error(
            StatusCode::NOT_FOUND,
            "not_found",
            "library not found",
        )),
        Err(e) => {
            tracing::error!(library_id = %library_id, error = %e, op, "thumbnails-enabled probe failed");
            Some(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ))
        }
    }
}

async fn queue_depth(app: &AppState) -> Result<i64, anyhow::Error> {
    use apalis::prelude::Storage;
    let mut storage = app.jobs.post_scan_thumbs_storage.clone();
    Ok(storage.len().await?)
}

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
