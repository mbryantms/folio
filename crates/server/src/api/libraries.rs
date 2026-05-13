//! `/libraries` and `/libraries/{id}/scan` (Phase 1a).
//!
//! Library access is filtered via `library_user_access` (§5.1.1). For Phase 1a,
//! the table is empty for everyone except admins, so admins see all libraries
//! and regular users see only the libraries they've been explicitly granted.
//!
//! `POST /libraries` (admin) creates a new library row pointing at a path.
//! `POST /libraries/{id}/scan` (admin) runs the Phase A scan synchronously
//! (apalis-queued background scan lands in Phase 1b).

use axum::{
    Extension, Json, Router,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use entity::{issue, library, library_user_access, scan_run, series};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, EntityTrait, ModelTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::{self, AuditEntry};
use crate::auth::{CurrentUser, RequireAdmin};
use crate::library::{ignore, thumbnails};
use crate::middleware::RequestContext;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/libraries", get(list).post(create))
        .route(
            "/libraries/{slug}",
            get(get_one).patch(update_settings).delete(delete_one),
        )
        .route("/libraries/{slug}/scan", post(scan))
        .route("/libraries/{slug}/scan-preview", get(scan_preview))
}

/// Look up a library row by its URL slug. Used by every `/libraries/{slug}`
/// handler — the slug column is UNIQUE so this is one row max. Returns
/// `Err` with the standard error envelope when not found.
pub(crate) async fn find_by_slug(
    db: &sea_orm::DatabaseConnection,
    slug: &str,
) -> Result<library::Model, axum::response::Response> {
    match library::Entity::find()
        .filter(library::Column::Slug.eq(slug))
        .one(db)
        .await
    {
        Ok(Some(row)) => Ok(row),
        Ok(None) => Err(error(
            StatusCode::NOT_FOUND,
            "not_found",
            "library not found",
        )),
        Err(e) => {
            tracing::error!(error = %e, slug, "library slug lookup failed");
            Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ))
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LibraryView {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub root_path: String,
    pub default_language: String,
    pub default_reading_direction: String,
    pub dedupe_by_content: bool,
    pub last_scan_at: Option<String>,
    /// Library Scanner v1 (Milestone 4) settings.
    pub ignore_globs: Vec<String>,
    pub report_missing_comicinfo: bool,
    pub file_watch_enabled: bool,
    pub soft_delete_days: i32,
    /// Cron expression governing the scheduled scan, or `null` if disabled.
    pub scan_schedule_cron: Option<String>,
    /// When true, the post-scan pipeline auto-enqueues page-strip thumbnails
    /// alongside the always-on cover thumbnails. See migration
    /// `m20261211_000001_generate_page_thumbs_on_scan` for rationale.
    pub generate_page_thumbs_on_scan: bool,
}

impl From<library::Model> for LibraryView {
    fn from(m: library::Model) -> Self {
        Self {
            id: m.id.to_string(),
            name: m.name,
            slug: m.slug,
            root_path: m.root_path,
            default_language: m.default_language,
            default_reading_direction: m.default_reading_direction,
            dedupe_by_content: m.dedupe_by_content,
            last_scan_at: m.last_scan_at.map(|t| t.to_rfc3339()),
            ignore_globs: m
                .ignore_globs
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            report_missing_comicinfo: m.report_missing_comicinfo,
            file_watch_enabled: m.file_watch_enabled,
            soft_delete_days: m.soft_delete_days,
            scan_schedule_cron: m.scan_schedule_cron,
            generate_page_thumbs_on_scan: m.generate_page_thumbs_on_scan,
        }
    }
}

/// Body for `PATCH /libraries/{id}` (Milestone 4). Every field is optional;
/// only the keys present in the body are updated.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateLibraryReq {
    #[serde(default)]
    pub ignore_globs: Option<Vec<String>>,
    #[serde(default)]
    pub report_missing_comicinfo: Option<bool>,
    #[serde(default)]
    pub file_watch_enabled: Option<bool>,
    #[serde(default)]
    pub soft_delete_days: Option<i32>,
    /// Cron expression. `null` clears it; an empty string is treated as null.
    /// Tri-state: omitted = leave unchanged; explicit `null` = clear.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub scan_schedule_cron: Option<Option<String>>,
    /// Admin override for the URL slug. The input is slugified
    /// (kebab-case, ASCII-folded) and rejected if it collides with another
    /// library's slug.
    #[serde(default)]
    pub slug: Option<String>,
    /// Toggle the per-library opt-in for auto-generating page-strip
    /// thumbnails on every post-scan pass. Cover thumbs are always
    /// generated regardless.
    #[serde(default)]
    pub generate_page_thumbs_on_scan: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateLibraryReq {
    pub name: String,
    pub root_path: String,
    #[serde(default = "default_lang")]
    pub default_language: String,
    #[serde(default = "default_dir")]
    pub default_reading_direction: String,
    /// Explicitly enqueue the initial scan after creating the row.
    /// Defaults to false so library creation is side-effect-light.
    #[serde(default)]
    pub scan_now: bool,
    /// Set the per-library `generate_page_thumbs_on_scan` flag at
    /// creation time. When true, the post-scan pipeline (including the
    /// initial scan triggered by `scan_now`) enqueues page-strip
    /// thumbnails alongside the always-on cover thumbnails. Defaults
    /// to false; user can flip it later from library settings.
    #[serde(default)]
    pub generate_page_thumbs_on_scan: bool,
}

fn default_lang() -> String {
    "eng".into()
}
fn default_dir() -> String {
    "ltr".into()
}

/// Response for `POST /libraries/{id}/scan` post-Milestone-2.
///
/// The scan now runs out-of-band; the response only carries the scan run id
/// and whether the trigger was coalesced into an existing in-flight scan.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ScanResp {
    pub scan_id: String,
    pub state: &'static str,
    pub coalesced: bool,
    pub kind: &'static str,
    pub library_id: String,
    pub mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coalesced_into: Option<String>,
    pub queued_followup: bool,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanMode {
    #[default]
    Normal,
    MetadataRefresh,
    ContentVerify,
}

impl ScanMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::MetadataRefresh => "metadata_refresh",
            Self::ContentVerify => "content_verify",
        }
    }

    pub fn force(self) -> bool {
        !matches!(self, Self::Normal)
    }

    pub fn reason(self) -> &'static str {
        match self {
            Self::Normal => "Uses folder and file fast paths; best for routine scans.",
            Self::MetadataRefresh => {
                "Re-parses metadata for matching files while preserving thumbnail work unless bytes changed."
            }
            Self::ContentVerify => {
                "Bypasses scanner fast paths to verify archive content; slowest option."
            }
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ScanPreviewView {
    pub mode: &'static str,
    pub dirty_folders: u64,
    pub known_issue_count: u64,
    pub thumbnail_backlog: u64,
    pub last_scan_duration_ms: Option<u64>,
    pub last_scan_state: Option<String>,
    pub watcher_status: String,
    pub reason: String,
}

// ───────── handlers ─────────

#[utoipa::path(
    get,
    path = "/libraries",
    responses((status = 200, body = Vec<LibraryView>))
)]
pub async fn list(State(app): State<AppState>, user: CurrentUser) -> impl IntoResponse {
    let q = library::Entity::find().order_by_asc(library::Column::Name);
    let rows: Vec<library::Model> = match q.all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "list libraries failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let visible = filter_visible(&app, &user, rows).await;
    Json(
        visible
            .into_iter()
            .map(LibraryView::from)
            .collect::<Vec<_>>(),
    )
    .into_response()
}

#[utoipa::path(
    get,
    path = "/libraries/{slug}",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = LibraryView),
        (status = 404, description = "not found or not accessible")
    )
)]
pub async fn get_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let row = match find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !user_can_see(&app, &user, &row).await {
        return error(StatusCode::NOT_FOUND, "not_found", "library not found");
    }
    Json(LibraryView::from(row)).into_response()
}

#[utoipa::path(
    post,
    path = "/libraries",
    request_body = CreateLibraryReq,
    responses(
        (status = 201, body = LibraryView),
        (status = 403, description = "admin only"),
        (status = 409, description = "root_path already in use")
    )
)]
pub async fn create(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Json(req): Json<CreateLibraryReq>,
) -> impl IntoResponse {
    let path = std::path::PathBuf::from(&req.root_path);
    if !path.is_absolute() {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "root_path must be absolute",
        );
    }
    let now = chrono::Utc::now().fixed_offset();
    let slug = match crate::slug::allocate_library_slug(&app.db, &req.name).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "allocate library slug failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let am = library::ActiveModel {
        id: Set(Uuid::now_v7()),
        name: Set(req.name),
        slug: Set(slug),
        root_path: Set(req.root_path),
        default_language: Set(req.default_language),
        default_reading_direction: Set(req.default_reading_direction),
        dedupe_by_content: Set(true),
        scan_schedule_cron: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_scan_at: Set(None),
        ignore_globs: Set(serde_json::json!([])),
        report_missing_comicinfo: Set(false),
        file_watch_enabled: Set(false),
        soft_delete_days: Set(30),
        thumbnails_enabled: Set(true),
        thumbnail_format: Set("webp".to_owned()),
        thumbnail_cover_quality: Set(crate::library::thumbnails::DEFAULT_COVER_QUALITY as i32),
        thumbnail_page_quality: Set(crate::library::thumbnails::DEFAULT_STRIP_QUALITY as i32),
        generate_page_thumbs_on_scan: Set(req.generate_page_thumbs_on_scan),
    };
    match am.insert(&app.db).await {
        Ok(m) => {
            if req.scan_now
                && let Err(e) = app.jobs.coalesce_scan(m.id, false).await
            {
                tracing::error!(library_id = %m.id, error = %e, "initial scan enqueue failed");
            }
            (StatusCode::CREATED, Json(LibraryView::from(m))).into_response()
        }
        Err(e) => {
            let s = e.to_string();
            if s.contains("unique") || s.contains("duplicate") {
                error(StatusCode::CONFLICT, "conflict", "root_path already used")
            } else {
                tracing::error!(error = %e, "create library failed");
                error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
            }
        }
    }
}

#[utoipa::path(
    patch,
    path = "/libraries/{slug}",
    params(("slug" = String, Path,)),
    request_body = UpdateLibraryReq,
    responses(
        (status = 200, body = LibraryView),
        (status = 400, description = "validation: invalid glob / soft_delete_days < 0"),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
pub async fn update_settings(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(slug): AxPath<String>,
    Json(req): Json<UpdateLibraryReq>,
) -> impl IntoResponse {
    let row = match find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let uuid = row.id;

    if let Some(globs) = &req.ignore_globs
        && let Err(e) = ignore::validate_globs(globs)
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation.ignore_globs",
            &e.to_string(),
        );
    }
    if let Some(days) = req.soft_delete_days
        && days < 0
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation.soft_delete_days",
            "soft_delete_days must be >= 0",
        );
    }
    if let Some(Some(cron_expr)) = &req.scan_schedule_cron
        && !cron_expr.trim().is_empty()
    {
        // Loose syntax check: ≥ 5 whitespace-separated tokens (the most common
        // shape — both 5- and 6-field cron formats satisfy this). The
        // tokio-cron-scheduler validation in Milestone 9 is the source of
        // truth, so this is an early sanity check, not a full parse.
        let token_count = cron_expr.split_whitespace().count();
        if token_count < 5 {
            return error(
                StatusCode::BAD_REQUEST,
                "validation.scan_schedule_cron",
                "cron expression must have at least 5 fields",
            );
        }
    }

    // Validate + slugify any admin-supplied slug before mutating state.
    let new_slug = if let Some(input) = req.slug.as_deref() {
        let s = crate::slug::slugify_segment(input);
        use crate::slug::SlugAllocator;
        let allocator = crate::slug::LibrarySlugAllocator {
            db: &app.db,
            excluding: Some(uuid),
        };
        match allocator.is_taken(&s).await {
            Ok(true) => {
                return error(StatusCode::CONFLICT, "conflict.slug", "slug already in use");
            }
            Ok(false) => Some(s),
            Err(e) => {
                tracing::error!(error = %e, "slug uniqueness check failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    } else {
        None
    };

    let schedule_touched = req.scan_schedule_cron.is_some();
    let mut am: library::ActiveModel = row.into();
    if let Some(s) = new_slug.clone() {
        am.slug = Set(s);
    }
    if let Some(globs) = req.ignore_globs {
        am.ignore_globs = Set(serde_json::Value::Array(
            globs.into_iter().map(serde_json::Value::String).collect(),
        ));
    }
    if let Some(b) = req.report_missing_comicinfo {
        am.report_missing_comicinfo = Set(b);
    }
    if let Some(b) = req.file_watch_enabled {
        am.file_watch_enabled = Set(b);
    }
    if let Some(d) = req.soft_delete_days {
        am.soft_delete_days = Set(d);
    }
    if let Some(b) = req.generate_page_thumbs_on_scan {
        am.generate_page_thumbs_on_scan = Set(b);
    }
    if let Some(cron_opt) = req.scan_schedule_cron {
        let normalized = cron_opt.and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        });
        am.scan_schedule_cron = Set(normalized);
    }
    am.updated_at = Set(chrono::Utc::now().fixed_offset());

    match am.update(&app.db).await {
        Ok(updated) => {
            if schedule_touched {
                crate::jobs::scheduler::reload_library_scan(&app, &updated).await;
            }
            if let Some(s) = new_slug {
                audit::record(
                    &app.db,
                    AuditEntry {
                        actor_id: user.id,
                        action: "admin.library.slug.set",
                        target_type: Some("library"),
                        target_id: Some(uuid.to_string()),
                        payload: serde_json::json!({ "slug": s }),
                        ip: ctx.ip_string(),
                        user_agent: ctx.user_agent.clone(),
                    },
                )
                .await;
            }
            Json(LibraryView::from(updated)).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, library_id = %uuid, "update library failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

/// Optional query params for the library scan endpoint. `mode` is the explicit
/// API; `force=true` remains a backwards-compatible content-verify alias.
#[derive(Debug, Default, Deserialize)]
pub struct ScanLibraryQuery {
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub mode: Option<ScanMode>,
}

#[utoipa::path(
    post,
    path = "/libraries/{slug}/scan",
    params(
        ("slug" = String, Path,),
        ("force" = Option<bool>, Query, description = "Bypass mtime fast paths. Defaults to false."),
    ),
    responses(
        (status = 202, body = ScanResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found")
    )
)]
pub async fn scan(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(slug): AxPath<String>,
    Query(q): Query<ScanLibraryQuery>,
) -> impl IntoResponse {
    let row = match find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let uuid = row.id;
    let mode = if q.force {
        ScanMode::ContentVerify
    } else {
        q.mode.unwrap_or_default()
    };
    match app.jobs.coalesce_scan(uuid, mode.force()).await {
        Ok(outcome) => (
            StatusCode::ACCEPTED,
            Json(ScanResp {
                scan_id: outcome.scan_id().to_string(),
                state: if outcome.was_coalesced() {
                    "coalesced"
                } else {
                    "queued"
                },
                coalesced: outcome.was_coalesced(),
                kind: "library",
                library_id: uuid.to_string(),
                mode: mode.as_str(),
                coalesced_into: outcome
                    .was_coalesced()
                    .then(|| outcome.scan_id().to_string()),
                queued_followup: outcome.was_coalesced(),
                reason: mode.reason().to_owned(),
                series_id: None,
                issue_id: None,
            }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "scan enqueue failed");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                &e.to_string(),
            )
        }
    }
}

#[utoipa::path(
    get,
    path = "/libraries/{slug}/scan-preview",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = ScanPreviewView),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found")
    )
)]
pub async fn scan_preview(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let row = match find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let known_issue_count = match issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(row.id))
        .filter(issue::Column::RemovedAt.is_null())
        .count(&app.db)
        .await
    {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(library_id = %row.id, error = %e, "scan preview: issue count failed");
            0
        }
    };

    let thumbnail_backlog = match issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(row.id))
        .filter(issue::Column::State.eq("active"))
        .filter(
            Condition::any()
                .add(issue::Column::ThumbnailsGeneratedAt.is_null())
                .add(issue::Column::ThumbnailVersion.lt(thumbnails::THUMBNAIL_VERSION))
                .add(issue::Column::ThumbnailsError.is_not_null()),
        )
        .count(&app.db)
        .await
    {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(library_id = %row.id, error = %e, "scan preview: thumbnail backlog failed");
            0
        }
    };

    let last_scan = scan_run::Entity::find()
        .filter(scan_run::Column::LibraryId.eq(row.id))
        .order_by_desc(scan_run::Column::StartedAt)
        .one(&app.db)
        .await
        .ok()
        .flatten();
    let last_scan_duration_ms = last_scan
        .as_ref()
        .and_then(|scan| scan.stats.get("elapsed_ms"))
        .and_then(serde_json::Value::as_u64);
    let last_scan_state = last_scan.as_ref().map(|scan| scan.state.clone());

    let series_rows = series::Entity::find()
        .filter(series::Column::LibraryId.eq(row.id))
        .all(&app.db)
        .await
        .unwrap_or_default();
    let dirty_folders = tokio::task::spawn_blocking(move || {
        series_rows
            .into_iter()
            .filter(|s| {
                let Some(folder) = s.folder_path.as_deref() else {
                    return true;
                };
                let Some(last) = s.last_scanned_at else {
                    return true;
                };
                server_dirty_folder(folder, last.to_utc())
            })
            .count() as u64
    })
    .await
    .unwrap_or(0);

    let watcher_status = if row.file_watch_enabled {
        "enabled_unverified"
    } else {
        "disabled"
    };
    let mode = ScanMode::Normal;
    Json(ScanPreviewView {
        mode: mode.as_str(),
        dirty_folders,
        known_issue_count,
        thumbnail_backlog,
        last_scan_duration_ms,
        last_scan_state,
        watcher_status: watcher_status.to_owned(),
        reason: mode.reason().to_owned(),
    })
    .into_response()
}

fn server_dirty_folder(folder: &str, last_scanned_at: chrono::DateTime<chrono::Utc>) -> bool {
    let folder = std::path::PathBuf::from(folder);
    if !folder.exists() {
        return true;
    }
    crate::library::scanner::enumerate::folder_changed_since(&folder, last_scanned_at)
}

/// Response for `DELETE /libraries/{id}` — small summary for the audit log
/// and any UI that wants to confirm what got purged.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeleteLibraryResp {
    pub deleted_library: String,
    /// Issue rows removed (cascades from libraries → series → issues).
    pub deleted_issues: u64,
    /// Series rows removed (cascade).
    pub deleted_series: u64,
    /// On-disk thumbnail directories swept. Best-effort — disk errors are
    /// logged but don't block the SQL delete.
    pub thumbs_swept: usize,
}

#[utoipa::path(
    delete,
    path = "/libraries/{slug}",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = DeleteLibraryResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
pub async fn delete_one(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let lib = match find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let uuid = lib.id;

    // ── 1. Wipe on-disk thumbnails for every issue in the library ──
    // Includes both active and removed issues — the FK cascade is about to
    // wipe the rows, so any orphan thumb file would otherwise survive
    // forever. Best-effort: failures are logged inside `wipe_issue_thumbs`.
    let issue_rows: Vec<(String,)> = match issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(uuid))
        .select_only()
        .column(issue::Column::Id)
        .into_tuple()
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(library_id = %uuid, error = %e, "delete-library: issue id query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let mut thumbs_swept = 0usize;
    for (issue_id,) in &issue_rows {
        thumbnails::wipe_issue_thumbs(&app.cfg().data_path, issue_id);
        thumbs_swept += 1;
    }

    // ── 2. Manual cleanup for tables with no FK to libraries ──
    // `library_user_access` predates the libraries table and was created
    // without an FK (see m20260101_000004_library_user_access.rs comment).
    // Without this delete the row would dangle.
    if let Err(e) = library_user_access::Entity::delete_many()
        .filter(library_user_access::Column::LibraryId.eq(uuid))
        .exec(&app.db)
        .await
    {
        tracing::warn!(library_id = %uuid, error = %e, "delete-library: library_user_access cleanup failed");
    }

    // ── 3. Capture pre-delete counts for the audit payload ──
    // After the cascading delete these are gone, so snapshot them now.
    let deleted_issues = issue_rows.len() as u64;
    let deleted_series = entity::series::Entity::find()
        .filter(entity::series::Column::LibraryId.eq(uuid))
        .count(&app.db)
        .await
        .unwrap_or(0);

    // ── 4. Delete the library row. FKs cascade into series → issues,
    // scan_runs, library_health_issues. search_doc generated columns on
    // series/issues vanish with their parent rows. ──
    if let Err(e) = lib.clone().delete(&app.db).await {
        tracing::error!(library_id = %uuid, error = %e, "delete-library: SQL delete failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    // ── 5. Best-effort Redis cleanup. A stale `scan:in_flight:<id>` key
    // would otherwise stay set and confuse the coalescer if a library
    // with the same UUID were ever re-created (unlikely; UUIDs are v7). ──
    if let Err(e) = app.jobs.purge_scan_keys(uuid).await {
        tracing::warn!(library_id = %uuid, error = %e, "delete-library: redis cleanup failed");
    }

    // ── 6. Audit log. The audit_log table has no FK to libraries (it's
    // append-only at the role level) so this row survives the delete. ──
    let lib_name = lib.name.clone();
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.library.delete",
            target_type: Some("library"),
            target_id: Some(uuid.to_string()),
            payload: serde_json::json!({
                "name": lib_name,
                "root_path": lib.root_path,
                "deleted_issues": deleted_issues,
                "deleted_series": deleted_series,
                "thumbs_swept": thumbs_swept,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(DeleteLibraryResp {
        deleted_library: uuid.to_string(),
        deleted_issues,
        deleted_series,
        thumbs_swept,
    })
    .into_response()
}

// ───────── ACL helpers ─────────

async fn filter_visible(
    app: &AppState,
    user: &CurrentUser,
    rows: Vec<library::Model>,
) -> Vec<library::Model> {
    if user.role == "admin" {
        return rows;
    }
    let lib_ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let granted: Vec<library_user_access::Model> = library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .filter(library_user_access::Column::LibraryId.is_in(lib_ids))
        .all(&app.db)
        .await
        .unwrap_or_default();
    let allowed: std::collections::HashSet<Uuid> =
        granted.into_iter().map(|g| g.library_id).collect();
    rows.into_iter()
        .filter(|r| allowed.contains(&r.id))
        .collect()
}

async fn user_can_see(app: &AppState, user: &CurrentUser, lib: &library::Model) -> bool {
    if user.role == "admin" {
        return true;
    }
    library_user_access::Entity::find_by_id((lib.id, user.id))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_some()
}

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

/// Tri-state deserialize helper: `{"foo": null}` becomes `Some(None)`
/// instead of serde's default `None`. Without it, an explicit JSON
/// `null` is indistinguishable from "field omitted" and a "clear the
/// column" branch silently no-ops.
fn deserialize_some<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}
