//! `/issues/{id}/archive/*` + `/uploads` — operator page-byte editing
//! (`archive-rewrite-1.0` M2).
//!
//! All routes are admin-only and gated on the library's
//! `allow_archive_writeback` toggle. The edit/restore endpoints enqueue
//! work or perform the swap directly; the actual byte rewrite runs in
//! [`crate::jobs::archive_edit`].
//!
//! v1 supports **CBZ only**. CBT lands in M4, CBR (via convert-to-CBZ) in
//! M6; until then non-CBZ issues get a friendly 422.

use axum::{
    Extension, Json,
    extract::{Multipart, Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::archive_rewrite::{self, mutex};
use crate::audit::{self, AuditEntry};
use crate::auth::RequireAdmin;
use crate::jobs::archive_edit::{ArchiveEditJob, BulkArchiveOp, PageOp, simulate_ops};
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(edit))
        .routes(routes!(bulk_edit))
        .routes(routes!(page_count))
        .routes(routes!(restore))
        .routes(routes!(backups))
        .routes(routes!(upload))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PageCountResponse {
    /// The archive's *actual* page count, read live from the file. Authoritative
    /// over the DB's `issue.page_count`, which can drift (stale scan, or sourced
    /// from a ComicInfo `<PageCount>`); the editor builds its tiles from this so
    /// it never shows a phantom page that isn't in the archive.
    pub page_count: usize,
}

#[utoipa::path(
    operation_id = "archive_page_count",    get,
    path = "/issues/{id}/archive/page-count",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = PageCountResponse, description = "live archive page count"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
        (status = 422, description = "writeback disabled / unsupported format / unreadable"),
    )
)]
#[handler]
pub async fn page_count(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(id): AxPath<String>,
) -> Response {
    let (row, _lib) = match preflight(&app, &id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let file_path = row.file_path.clone();
    let limits = app.cfg().archive_limits();
    match tokio::task::spawn_blocking(move || {
        archive::open(std::path::Path::new(&file_path), limits).map(|c| c.pages().len())
    })
    .await
    {
        Ok(Ok(n)) => (StatusCode::OK, Json(PageCountResponse { page_count: n })).into_response(),
        Ok(Err(e)) => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "archive.unreadable",
            &e.to_string(),
        ),
        Err(e) => {
            tracing::error!(error = %e, "archive page-count: join failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

/// Upper bound on a single bulk request. A large but bounded fan-out keeps
/// the queue (and the audit log) sane; the worker runs at concurrency 1.
const MAX_BULK_ISSUES: usize = 500;

/// Cap on a staged replacement image. Generous — a high-res page scan can
/// be a few MiB; 32 MiB leaves headroom without inviting abuse.
const MAX_UPLOAD_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct EditRequest {
    pub ops: Vec<PageOp>,
    /// When true, validate the ops and return the would-be page count
    /// without touching the archive.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EditResponse {
    /// `dry_run=true` — validation succeeded; here's what would happen.
    DryRun {
        page_count_before: usize,
        page_count_after: usize,
    },
    /// Edit job enqueued; the UI waits for the scan-completed WebSocket.
    Queued { issue_id: String },
}

/// Shared preflight: resolve the issue, enforce admin-writeback gating +
/// CBZ-only + writable mount. Returns the issue row + library on success,
/// or an error `Response`.
async fn preflight(
    app: &AppState,
    issue_id: &str,
) -> Result<(entity::issue::Model, entity::library::Model), Response> {
    let Some(row) = entity::issue::Entity::find_by_id(issue_id)
        .one(&app.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "archive edit: issue lookup failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        })?
    else {
        return Err(error(
            StatusCode::NOT_FOUND,
            "issue.not_found",
            "issue not found",
        ));
    };
    let Some(lib) = entity::library::Entity::find_by_id(row.library_id)
        .one(&app.db)
        .await
        .ok()
        .flatten()
    else {
        return Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal",
        ));
    };
    if !lib.allow_archive_writeback {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.archive_writeback_disabled",
            "archive writeback is not enabled for this library",
        ));
    }
    if !is_editable_format(&row.file_path) {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.archive_format_unsupported",
            "page editing supports CBZ, CBT, and CBR archives (CBR is converted to CBZ)",
        ));
    }
    Ok((row, lib))
}

/// CBZ/CBT rewrite in place; CBR converts to CBZ. CB7 + anything else has
/// no writer and is rejected.
fn is_editable_format(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| matches!(e.as_str(), "cbz" | "cbt" | "cbr"))
}

#[utoipa::path(
    operation_id = "archive_edit",    post,
    path = "/issues/{id}/archive/edit",
    params(("id" = String, Path,)),
    request_body = EditRequest,
    responses(
        (status = 200, body = EditResponse, description = "dry-run result"),
        (status = 202, body = EditResponse, description = "edit enqueued"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
        (status = 422, description = "writeback disabled / unsupported format / invalid ops"),
    )
)]
#[handler]
pub async fn edit(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<String>,
    Json(req): Json<EditRequest>,
) -> Response {
    let (row, lib) = match preflight(&app, &id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    // Open the archive off-thread to count pages, then validate the ops.
    // `archive::open` dispatches by extension (cbz/cbt/cbr).
    let file_path = row.file_path.clone();
    let limits = app.cfg().archive_limits();
    let page_count = match tokio::task::spawn_blocking(move || {
        archive::open(std::path::Path::new(&file_path), limits).map(|c| c.pages().len())
    })
    .await
    {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "archive.unreadable",
                &e.to_string(),
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "archive edit: page-count join failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let after = match simulate_ops(page_count, &req.ops) {
        Ok(n) => n,
        Err(e) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.page_ops",
                &e.to_string(),
            );
        }
    };

    if req.dry_run {
        return (
            StatusCode::OK,
            Json(EditResponse::DryRun {
                page_count_before: page_count,
                page_count_after: after,
            }),
        )
            .into_response();
    }

    let _ = lib; // gating already done in preflight
    use apalis::prelude::Storage;
    let mut storage = app.jobs.archive_edit_storage.clone();
    if let Err(e) = storage
        .push(ArchiveEditJob {
            issue_id: row.id.clone(),
            ops: req.ops,
            bulk_op: None,
            actor_id: Some(actor.id),
            actor_ip: ctx.ip_string(),
            actor_ua: ctx.user_agent.clone(),
        })
        .await
    {
        tracing::error!(error = %e, "archive edit: enqueue failed");
        return error(
            StatusCode::BAD_GATEWAY,
            "queue_unavailable",
            "could not enqueue edit",
        );
    }

    (
        StatusCode::ACCEPTED,
        Json(EditResponse::Queued { issue_id: row.id }),
    )
        .into_response()
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BulkEditRequest {
    /// Issue ids to apply the op to. Issues that don't exist, live in a
    /// non-writeback library, or aren't an editable format are skipped (and
    /// reported), not failed.
    pub issue_ids: Vec<String>,
    /// The single relative op applied to every eligible issue.
    pub op: BulkArchiveOp,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkSkip {
    pub issue_id: String,
    pub reason: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkEditResponse {
    /// Number of per-issue edit jobs enqueued.
    pub queued: usize,
    /// Issues that were skipped, with a reason each.
    pub skipped: Vec<BulkSkip>,
}

#[utoipa::path(
    operation_id = "archive_bulk_edit",    post,
    path = "/archive/bulk-edit",
    request_body = BulkEditRequest,
    responses(
        (status = 202, body = BulkEditResponse, description = "edit jobs enqueued"),
        (status = 403, description = "admin only"),
        (status = 422, description = "empty selection or too many issues"),
    )
)]
#[handler]
pub async fn bulk_edit(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<BulkEditRequest>,
) -> Response {
    if req.issue_ids.is_empty() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.empty_selection",
            "no issues selected",
        );
    }
    if req.issue_ids.len() > MAX_BULK_ISSUES {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.too_many_issues",
            &format!("too many issues (max {MAX_BULK_ISSUES})"),
        );
    }

    // Batch-load the issues + their libraries, then gate each by writeback +
    // editable format. We deliberately do NOT open archives here — the page
    // count is resolved per issue inside the worker (the bulk op is lowered
    // there), so the request stays cheap regardless of selection size.
    let issue_rows = match entity::issue::Entity::find()
        .filter(entity::issue::Column::Id.is_in(req.issue_ids.clone()))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "bulk archive edit: issue load failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let issue_map: HashMap<String, entity::issue::Model> =
        issue_rows.into_iter().map(|i| (i.id.clone(), i)).collect();

    let lib_ids: Vec<Uuid> = issue_map
        .values()
        .map(|i| i.library_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let lib_map: HashMap<Uuid, entity::library::Model> = match entity::library::Entity::find()
        .filter(entity::library::Column::Id.is_in(lib_ids))
        .all(&app.db)
        .await
    {
        Ok(v) => v.into_iter().map(|l| (l.id, l)).collect(),
        Err(e) => {
            tracing::error!(error = %e, "bulk archive edit: library load failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    use apalis::prelude::Storage;
    let mut storage = app.jobs.archive_edit_storage.clone();
    let mut skipped: Vec<BulkSkip> = Vec::new();
    let mut queued_ids: Vec<String> = Vec::new();

    // Preserve the caller's order so the audit + response read predictably.
    for id in &req.issue_ids {
        let Some(row) = issue_map.get(id) else {
            skipped.push(BulkSkip {
                issue_id: id.clone(),
                reason: "issue not found".to_owned(),
            });
            continue;
        };
        let Some(lib) = lib_map.get(&row.library_id) else {
            skipped.push(BulkSkip {
                issue_id: id.clone(),
                reason: "library not found".to_owned(),
            });
            continue;
        };
        if !lib.allow_archive_writeback {
            skipped.push(BulkSkip {
                issue_id: id.clone(),
                reason: "archive writeback disabled for this library".to_owned(),
            });
            continue;
        }
        if !is_editable_format(&row.file_path) {
            skipped.push(BulkSkip {
                issue_id: id.clone(),
                reason: "unsupported archive format (CBZ/CBT/CBR only)".to_owned(),
            });
            continue;
        }
        if let Err(e) = storage
            .push(ArchiveEditJob {
                issue_id: row.id.clone(),
                ops: Vec::new(),
                bulk_op: Some(req.op),
                actor_id: Some(actor.id),
                actor_ip: ctx.ip_string(),
                actor_ua: ctx.user_agent.clone(),
            })
            .await
        {
            tracing::error!(error = %e, issue_id = %row.id, "bulk archive edit: enqueue failed");
            skipped.push(BulkSkip {
                issue_id: id.clone(),
                reason: "could not enqueue".to_owned(),
            });
            continue;
        }
        queued_ids.push(row.id.clone());
    }

    // One audit row for the bulk action (each enqueued job also emits its own
    // `admin.issue.archive_edit` row when it runs — that's the drill-down).
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.issue.archive_edit.bulk",
            target_type: Some("issue"),
            target_id: None,
            payload: serde_json::json!({
                "op": req.op,
                "queued": queued_ids.len(),
                "queued_issue_ids": queued_ids,
                "skipped": skipped.len(),
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    (
        StatusCode::ACCEPTED,
        Json(BulkEditResponse {
            queued: queued_ids.len(),
            skipped,
        }),
    )
        .into_response()
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RestoreResponse {
    pub issue_id: String,
    pub status: &'static str,
}

#[utoipa::path(
    operation_id = "archive_edit_restore",    post,
    path = "/issues/{id}/archive/restore",
    params(("id" = String, Path,)),
    responses(
        (status = 202, body = RestoreResponse),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found / no backup"),
        (status = 409, description = "another rewrite is in progress"),
        (status = 422, description = "writeback disabled / unsupported format"),
    )
)]
#[handler]
pub async fn restore(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<String>,
) -> Response {
    let (row, _lib) = match preflight(&app, &id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let mut redis = app.jobs.redis.clone();
    match mutex::try_claim(&mut redis, &row.id, mutex::EDIT_TTL_SECS).await {
        Ok(true) => {}
        Ok(false) => {
            return error(
                StatusCode::CONFLICT,
                "archive.rewrite_in_progress",
                "another rewrite is in progress for this issue",
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "archive restore: mutex claim failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    }

    let target = std::path::PathBuf::from(&row.file_path);
    let restore_res =
        tokio::task::spawn_blocking(move || archive_rewrite::restore_latest_backup(&target)).await;

    // Always release the mutex regardless of outcome.
    let mut redis = app.jobs.redis.clone();
    mutex::release(&mut redis, &row.id).await;

    match restore_res {
        Ok(Ok(())) => {}
        Ok(Err(archive_rewrite::RewriteError::Io(e)))
            if e.kind() == std::io::ErrorKind::NotFound =>
        {
            return error(
                StatusCode::NOT_FOUND,
                "archive.no_backup",
                "no backup is available to restore",
            );
        }
        Ok(Err(e)) => {
            tracing::error!(error = %e, "archive restore: failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
        Err(e) => {
            tracing::error!(error = %e, "archive restore: join failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    }

    app.zip_lru.invalidate(&row.id);

    // Clear thumbnail stamps so the post-scan pipeline regenerates them
    // from the restored bytes, and stamp the rewrite.
    let am = entity::issue::ActiveModel {
        id: Set(row.id.clone()),
        last_rewrite_at: Set(Some(chrono::Utc::now().fixed_offset())),
        last_rewrite_kind: Set(Some("edit".to_owned())),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        updated_at: Set(chrono::Utc::now().fixed_offset()),
        ..Default::default()
    };
    if let Err(e) = am.update(&app.db).await {
        tracing::error!(error = %e, "archive restore: issue bookkeeping update failed");
    }

    if let Err(e) = app
        .jobs
        .coalesce_scoped_scan(
            row.library_id,
            row.series_id,
            None,
            crate::jobs::scan_series::JobKind::Issue,
            Some(row.id.clone()),
            true,
        )
        .await
    {
        tracing::error!(error = %e, "archive restore: rescan enqueue failed");
    }

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.issue.archive_restore",
            target_type: Some("issue"),
            target_id: Some(row.id.clone()),
            payload: serde_json::json!({ "issue_id": row.id, "file_path": row.file_path }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    (
        StatusCode::ACCEPTED,
        Json(RestoreResponse {
            issue_id: row.id,
            status: "restored",
        }),
    )
        .into_response()
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BackupView {
    /// Retention slot: 0 = most recent (`.bak`), 1 = `.bak.1`, …
    pub slot: i32,
    pub path: String,
    /// RFC3339 modification time, or null if unavailable.
    pub modified_at: Option<String>,
    pub size_bytes: u64,
}

#[utoipa::path(
    operation_id = "archive_edit_backups",    get,
    path = "/issues/{id}/archive/backups",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = [BackupView]),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
    )
)]
#[handler]
pub async fn backups(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(id): AxPath<String>,
) -> Response {
    let Some(row) = entity::issue::Entity::find_by_id(&id)
        .one(&app.db)
        .await
        .ok()
        .flatten()
    else {
        return error(StatusCode::NOT_FOUND, "issue.not_found", "issue not found");
    };

    let target = std::path::PathBuf::from(&row.file_path);
    let mut out = Vec::new();
    for slot in 0..=5 {
        let p = archive_rewrite::backup_slot(&target, slot);
        let Ok(meta) = std::fs::metadata(&p) else {
            continue;
        };
        let modified_at = meta
            .modified()
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
        out.push(BackupView {
            slot,
            path: p.to_string_lossy().into_owned(),
            modified_at,
            size_bytes: meta.len(),
        });
    }

    Json(out).into_response()
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UploadView {
    pub id: String,
    pub content_type: String,
}

#[utoipa::path(
    operation_id = "uploads_create",    post,
    path = "/uploads",
    request_body(content = String, description = "multipart/form-data with a `file` image field", content_type = "multipart/form-data"),
    responses(
        (status = 201, body = UploadView),
        (status = 403, description = "admin only"),
        (status = 413, description = "file too large"),
        (status = 422, description = "missing/invalid image"),
    )
)]
#[handler]
pub async fn upload(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    mut multipart: Multipart,
) -> Response {
    let mut bytes: Option<Vec<u8>> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            match field.bytes().await {
                Ok(b) => {
                    if b.len() > MAX_UPLOAD_BYTES {
                        return error(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "too_large",
                            "image exceeds 32 MiB",
                        );
                    }
                    bytes = Some(b.to_vec());
                }
                Err(e) => {
                    return error(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "validation",
                        &e.to_string(),
                    );
                }
            }
        }
    }
    let Some(bytes) = bytes else {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "missing `file` field",
        );
    };

    // Validate it's an image we can decode/re-encode later.
    let Ok(fmt) = image::guess_format(&bytes) else {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.not_an_image",
            "file is not a supported image",
        );
    };

    let uploads_dir = app.cfg().data_path.join("uploads");
    sweep_stale_uploads(&uploads_dir);
    if let Err(e) = std::fs::create_dir_all(&uploads_dir) {
        tracing::error!(error = %e, "uploads: mkdir failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    let id = Uuid::now_v7();
    let dest = uploads_dir.join(id.to_string());
    if let Err(e) = std::fs::write(&dest, &bytes) {
        tracing::error!(error = %e, "uploads: write failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    (
        StatusCode::CREATED,
        Json(UploadView {
            id: id.to_string(),
            content_type: fmt.to_mime_type().to_owned(),
        }),
    )
        .into_response()
}

/// Best-effort removal of staged uploads older than one hour — they're
/// consumed by an edit shortly after upload, so anything older is
/// abandoned. Runs opportunistically on each upload; errors are logged
/// and swallowed.
fn sweep_stale_uploads(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(3600))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        if meta.modified().is_ok_and(|m| m < cutoff) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
