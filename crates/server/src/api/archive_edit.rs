//! `/issues/{id}/archive/*` + `/uploads` — operator page-byte editing
//! (`archive-rewrite-1.0` M2).
//!
//! All routes are admin-only and gated on the library's
//! `allow_archive_writeback` toggle. The edit/restore endpoints enqueue
//! work or perform the swap directly; the actual byte rewrite runs in
//! [`crate::jobs::archive_edit`].
//!
//! Supported containers: **CBZ**, **CBT** (rewritten in place as tar) and
//! **CBR** (converted to a sibling `.cbz`; the original is kept as
//! `.cbr.bak`) — see `EditFormat` in [`crate::jobs::archive_edit`]. Any
//! other extension gets a friendly 422.

use axum::{
    Extension, Json,
    extract::{DefaultBodyLimit, Multipart, Path as AxPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::FutureExt;
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
use crate::library::event_log::{self, Action, Category, NewEvent, Severity};
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
}

/// `POST /uploads`, kept out of [`routes`] because `app::build_openapi_router`
/// mounts it in the long-body group: a 32 MiB body over a slow phone link
/// outlasts `app::JSON_ROUTE_TIMEOUT`, so it runs under
/// `app::UPLOAD_ROUTE_TIMEOUT` (plus the per-frame
/// `app::UPLOAD_BODY_IDLE_TIMEOUT`) and skips compression instead.
///
/// The staged-image upload is the one body in this module bigger than the
/// 2 MiB default (`app::DEFAULT_BODY_LIMIT`), so it raises the limit on its
/// own sub-router. The handler still caps the `file` field at
/// `MAX_UPLOAD_BYTES`; the slack covers the multipart framing around it.
pub fn upload_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(upload))
        .layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT))
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
    match live_page_count(&app, &row).await {
        Ok(Ok(n)) => (StatusCode::OK, Json(PageCountResponse { page_count: n })).into_response(),
        Ok(Err(e)) => error(StatusCode::UNPROCESSABLE_ENTITY, "archive.unreadable", &e),
        Err(e) => {
            tracing::error!(error = %e, "archive page-count: server fault");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

/// The archive's live page count, read through a handle from the
/// no-follow walk (`library::no_follow`). A refused file (a link, not a
/// regular file, outside the root) answers like an unreadable archive.
/// The outer `Err` is a server fault (a failed library lookup or a join
/// failure), answered 500.
async fn live_page_count(
    app: &AppState,
    row: &entity::issue::Model,
) -> Result<Result<usize, String>, String> {
    let source = match crate::library::no_follow::open_issue_archive(app, row).await {
        Ok(s) => s,
        Err(r @ crate::library::no_follow::Refusal::Lookup(_)) => return Err(r.to_string()),
        Err(r) => return Ok(Err(format!("archive not opened: {r}"))),
    };
    let limits = app.cfg().archive_limits();
    tokio::task::spawn_blocking(move || {
        archive::open_file(source, limits)
            .map(|c| c.pages().len())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("join failed: {e}"))
}

/// Upper bound on a single bulk request. A large but bounded fan-out keeps
/// the queue (and the audit log) sane; the worker runs at concurrency 1.
const MAX_BULK_ISSUES: usize = 500;

/// Cap on a staged replacement image. Generous — a high-res page scan can
/// be a few MiB; 32 MiB leaves headroom without inviting abuse.
const MAX_UPLOAD_BYTES: usize = 32 * 1024 * 1024;

/// Request-body limit on `POST /uploads`: the field cap plus 64 KiB for
/// the multipart boundary and part headers.
const UPLOAD_BODY_LIMIT: usize = MAX_UPLOAD_BYTES + 64 * 1024;

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
    preflight_checked(app, issue_id)
        .await
        .map_err(RestoreFailure::into_response)
}

/// [`preflight`] with the refusal as data, so the restore task can run the
/// same gates again once it holds the lock.
async fn preflight_checked(
    app: &AppState,
    issue_id: &str,
) -> Result<(entity::issue::Model, entity::library::Model), RestoreFailure> {
    let internal =
        || RestoreFailure::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    let Some(row) = entity::issue::Entity::find_by_id(issue_id)
        .one(&app.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "archive edit: issue lookup failed");
            internal()
        })?
    else {
        return Err(RestoreFailure::new(
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
        return Err(internal());
    };
    if !lib.allow_archive_writeback {
        return Err(RestoreFailure::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.archive_writeback_disabled",
            "archive writeback is not enabled for this library",
        ));
    }
    if !is_editable_format(&row.file_path) {
        return Err(RestoreFailure::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.archive_format_unsupported",
            "page editing supports CBZ, CBT, and CBR archives (CBR is converted to CBZ)",
        ));
    }
    Ok((row, lib))
}

/// CBZ/CBT rewrite in place; CBR converts to CBZ. CB7 + anything else is
/// rejected (a CB7 only becomes an issue row after scan-time conversion to
/// CBZ, so the page editor never needs a CB7 path).
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
    // `archive::open_file` dispatches by container (cbz/cbt/cbr).
    let page_count = match live_page_count(&app, &row).await {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => {
            return error(StatusCode::UNPROCESSABLE_ENTITY, "archive.unreadable", &e);
        }
        Err(e) => {
            tracing::error!(error = %e, "archive edit: page-count server fault");
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
    use apalis::prelude::TaskSink;
    let mut storage = app.jobs.archive_edit_storage.clone();
    if let Err(e) = storage
        .push(ArchiveEditJob {
            issue_id: row.id.clone(),
            ops: req.ops,
            bulk_op: None,
            actor_id: Some(actor.id),
            actor_ip: ctx.ip_string(),
            actor_ua: ctx.user_agent.clone(),
            attempt: 0,
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

    use apalis::prelude::TaskSink;
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
                attempt: 0,
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
    /// `restored`, or `in_progress` when the restore was still running at
    /// the response deadline ([`RESTORE_RESPONSE_DEADLINE`]).
    pub status: &'static str,
    /// True when the restore was still running when this response was
    /// sent: it finishes on its own (audit row, rescan, lock release), and
    /// a failure is recorded as a library event. `kept_as`, `warning` and
    /// `rebuilt_from` are then not known yet.
    pub in_progress: bool,
    /// Where the archive the restore replaced is kept: `<file>.bak`, the
    /// newest backup (restore swaps the archive and its `.bak`, so
    /// restoring again undoes it). Null when there was no archive at the
    /// issue's path to keep. When the replaced archive could not take the
    /// `.bak` name (the last step of the fallback swap failed), the name
    /// it is kept under instead, `<file>.bak.<hex>.tmp`; `warning` says so.
    pub kept_as: Option<String>,
    /// Set when the restore happened but something about keeping the
    /// replaced archive did not go as planned: says where it is and why.
    pub warning: Option<String>,
    /// Set when the restore rebuilt the archive from a pre-conversion
    /// backup (`?backup=X.cbr.bak`): the backup's file name. The archive
    /// is then a CBZ rebuilt from that file, not its bytes.
    pub rebuilt_from: Option<String>,
    /// What the restore did (or, `in_progress`, that it is still running),
    /// in one sentence for the admin.
    pub message: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RestoreQuery {
    /// Which backup to restore: its file name exactly as the backups
    /// listing returns it in `name`. Omitted, or the issue's own
    /// `<file>.bak`: the newest backup is swapped in (unchanged behaviour).
    /// A pre-conversion backup (`pre_conversion: true`) is rebuilt as a
    /// CBZ into the issue's file. Anything else the listing does not offer
    /// for this issue is refused (422 `archive.backup_not_restorable`).
    pub backup: Option<String>,
}

/// The refusal for a `?backup=` the listing does not offer.
const BACKUP_NOT_RESTORABLE: &str = "restore takes the newest backup (.bak) or a pre-conversion \
     backup the backups listing offers for this issue";

/// How long the restore handler waits for the restore before answering
/// `in_progress`; comfortably below the `/api` route timeout
/// (`app::JSON_ROUTE_TIMEOUT`, 60 s), which would otherwise answer 408 for
/// a restore that goes on to happen. The restore itself runs in its own
/// task and is never cut short by either.
pub const RESTORE_RESPONSE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(45);

#[utoipa::path(
    operation_id = "archive_edit_restore",    post,
    path = "/issues/{id}/archive/restore",
    params(("id" = String, Path,), RestoreQuery),
    responses(
        (status = 202, body = RestoreResponse, description = "restored, or (`in_progress: true`) still running at the response deadline and finishing on its own"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found / no backup"),
        (status = 409, description = "another rewrite is in progress / the issue's archive changed while the restore was starting (`archive.issue_changed`)"),
        (status = 422, description = "writeback disabled / unsupported format / the named backup is not restorable / the pre-conversion backup could not be rebuilt"),
    )
)]
#[handler]
pub async fn restore(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<String>,
    Query(q): Query<RestoreQuery>,
) -> Response {
    let (row, lib) = match preflight(&app, &id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let target = std::path::PathBuf::from(&row.file_path);
    let own_bak = archive_rewrite::backup_slot(&target, 0);
    let own_bak_name = own_bak
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    let named = q.backup.filter(|b| Some(b) != own_bak_name.as_ref());
    let request = RestoreRequest {
        row,
        actor_id: actor.id,
        actor_ip: ctx.ip_string(),
        actor_ua: ctx.user_agent.clone(),
        rebuilt_from: None,
    };
    let Some(named) = named else {
        return restore_issue(
            &app,
            request,
            RESTORE_RESPONSE_DEADLINE,
            archive_rewrite::restore_latest_backup,
        )
        .await;
    };
    // Only a backup the listing offers for THIS issue, matched by its bare
    // file name; the name never becomes a path of its own. Own older slots
    // are listed but not restorable here.
    let listed = listed_backups(&target).await;
    let Some(found) = listed
        .into_iter()
        .find(|b| b.pre_conversion && b.name == named)
    else {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "archive.backup_not_restorable",
            BACKUP_NOT_RESTORABLE,
        );
    };
    let limits = app.cfg().archive_limits();
    let retain = lib.archive_backup_retain_count;
    let source = std::ffi::OsString::from(found.name.clone());
    restore_issue(
        &app,
        RestoreRequest {
            rebuilt_from: Some(found.name),
            ..request
        },
        RESTORE_RESPONSE_DEADLINE,
        move |root, target| {
            archive_rewrite::restore_rebuilt(
                root,
                target,
                &source,
                retain,
                true,
                |io| crate::library::scanner::cbr_convert::rebuild_cbz_into(io, limits),
                |_| Ok(()),
            )
        },
    )
    .await
}

/// Who asked for a restore and of what. `rebuilt_from` names the
/// pre-conversion backup a rebuild-restore reads (`None`: the ordinary
/// restore); a rebuild also takes an archive-work permit.
pub struct RestoreRequest {
    pub row: entity::issue::Model,
    pub actor_id: Uuid,
    pub actor_ip: Option<String>,
    pub actor_ua: Option<String>,
    pub rebuilt_from: Option<String>,
}

/// A restore that did not happen: the response it would have been.
#[derive(Debug)]
pub struct RestoreFailure {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl RestoreFailure {
    fn into_response(self) -> Response {
        error(self.status, self.code, &self.message)
    }

    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

/// A restore that happened.
#[derive(Debug)]
pub struct RestoreDone {
    kept_as: Option<String>,
    warning: Option<String>,
    rebuilt_from: Option<String>,
    message: String,
}

/// The restore after [`preflight`], in a task of its own so nothing that
/// must follow the swap depends on the client still waiting: the task
/// claims the per-issue rewrite lock, keeps it alive with a heartbeat (as
/// the rewrite job and the page editor do) and holds it through the swap,
/// the cached-reader drop, the row stamp, the rescan enqueue and the audit
/// row, then releases it. The handler waits up to `deadline` for the
/// outcome; past it (or when the request is dropped, e.g. by the route
/// timeout or a client that went away) it answers `in_progress` and the
/// task finishes alone, logging a failure at ERROR and recording it as a
/// library event, since no response is left to carry it.
///
/// `swap` is [`archive_rewrite::restore_latest_backup`] or a
/// [`archive_rewrite::restore_rebuilt`] closure in production; tests pass
/// fault-injecting or slow variants. Public for the integration tests.
pub async fn restore_issue<F>(
    app: &AppState,
    request: RestoreRequest,
    deadline: std::time::Duration,
    swap: F,
) -> Response
where
    F: FnOnce(
            &crate::library::no_follow::LibraryRoot,
            &std::path::Path,
        ) -> Result<archive_rewrite::RestoreOutcome, archive_rewrite::RewriteError>
        + Send
        + 'static,
{
    let issue_id = request.row.id.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let task_app = app.clone();
    tokio::spawn(async move {
        let library_id = request.row.library_id;
        let slug = request.row.slug.clone();
        let issue_id = request.row.id.clone();
        let result = run_restore(&task_app, request, swap).await;
        // Nobody is waiting any more: the handler answered `in_progress`
        // or its request was dropped.
        if let Err(unsent) = tx.send(result) {
            match unsent {
                Ok(done) => tracing::info!(
                    issue_id = %issue_id,
                    message = %done.message,
                    "archive restore: finished after the response was sent",
                ),
                Err(failure) => {
                    tracing::error!(
                        issue_id = %issue_id,
                        code = failure.code,
                        error = %failure.message,
                        "archive restore: failed after the response was sent",
                    );
                    event_log::record(
                        &task_app.db,
                        NewEvent::new(
                            library_id,
                            Category::Archive,
                            Action::Errored,
                            Severity::Error,
                            format!(
                                "Archive restore of {slug} failed after the request had \
                                 returned: {}",
                                failure.message
                            ),
                        )
                        .entity("issue", issue_id.clone(), Some(slug))
                        .detail(serde_json::json!({
                            "code": failure.code,
                            "status": failure.status.as_u16(),
                        })),
                    )
                    .await;
                }
            }
        }
    });
    match tokio::time::timeout(deadline, rx).await {
        Ok(Ok(Ok(done))) => (
            StatusCode::ACCEPTED,
            Json(RestoreResponse {
                issue_id,
                status: "restored",
                in_progress: false,
                kept_as: done.kept_as,
                warning: done.warning,
                rebuilt_from: done.rebuilt_from,
                message: done.message,
            }),
        )
            .into_response(),
        Ok(Ok(Err(failure))) => failure.into_response(),
        Ok(Err(_)) => {
            tracing::error!(issue_id = %issue_id, "archive restore: the restore task ended without an outcome");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
        Err(_) => (
            StatusCode::ACCEPTED,
            Json(RestoreResponse {
                issue_id,
                status: "in_progress",
                in_progress: true,
                kept_as: None,
                warning: None,
                rebuilt_from: None,
                message: "The restore is still running; the issue is rescanned when it is \
                          done. A failure is recorded in the library's events."
                    .to_owned(),
            }),
        )
            .into_response(),
    }
}

/// The reason a backup could not be rebuilt, for the response and the
/// log: the error kind and a short fixed reason. The reader's own text can
/// carry entry names from an untrusted archive, so only our own validation
/// messages pass through, bounded and without control characters.
fn rebuild_failure_reason(e: &archive_rewrite::RewriteError) -> String {
    use archive::ArchiveError as A;
    use archive_rewrite::RewriteError as R;
    match e {
        R::ArchiveErr(A::Encrypted) => "archive error: the backup is encrypted".to_owned(),
        R::ArchiveErr(A::CapExceeded(_)) => {
            "archive error: the backup is over an archive size limit".to_owned()
        }
        R::ArchiveErr(A::UnsafeEntry(_)) => {
            "archive error: the backup has an entry with an unsafe path".to_owned()
        }
        R::ArchiveErr(A::Io(_)) => "archive error: the backup could not be read".to_owned(),
        R::ArchiveErr(_) => "archive error: the backup is damaged or unreadable".to_owned(),
        R::ValidationFailed(m) => format!("validation failed: {}", bounded_text(m, 160)),
        _ => "the rebuild failed".to_owned(),
    }
}

/// `s` with control characters replaced by spaces, cut to at most `max`
/// characters (an ellipsis marks the cut).
fn bounded_text(s: &str, max: usize) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

/// The body of [`restore_issue`]'s task: lock, swap, then everything that
/// must follow the swap, then unlock.
async fn run_restore<F>(
    app: &AppState,
    request: RestoreRequest,
    swap: F,
) -> Result<RestoreDone, RestoreFailure>
where
    F: FnOnce(
            &crate::library::no_follow::LibraryRoot,
            &std::path::Path,
        ) -> Result<archive_rewrite::RestoreOutcome, archive_rewrite::RewriteError>
        + Send
        + 'static,
{
    let RestoreRequest {
        row,
        actor_id,
        actor_ip,
        actor_ua,
        rebuilt_from,
    } = request;
    let mut redis = app.jobs.redis.clone();
    let token = match mutex::try_claim(&mut redis, &row.id, mutex::EDIT_TTL_SECS).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return Err(RestoreFailure::new(
                StatusCode::CONFLICT,
                "archive.rewrite_in_progress",
                "another rewrite is in progress for this issue",
            ));
        }
        Err(e) => {
            tracing::error!(error = %e, "archive restore: mutex claim failed");
            return Err(RestoreFailure::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    // Held until the very end, through the bookkeeping after the swap.
    let heartbeat = mutex::Heartbeat::start(
        app.jobs.redis.clone(),
        row.id.clone(),
        token.clone(),
        mutex::EDIT_TTL_SECS,
    );
    // A panic in the work after the claim still releases the lock and
    // reaches the handler (or, once it is gone, the library events) as a
    // failure.
    let result = std::panic::AssertUnwindSafe(restore_locked(
        app,
        row.clone(),
        actor_id,
        actor_ip,
        actor_ua,
        rebuilt_from,
        swap,
    ))
    .catch_unwind()
    .await
    .unwrap_or_else(|_| {
        tracing::error!(issue_id = %row.id, "archive restore: the restore task panicked");
        Err(RestoreFailure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the restore failed unexpectedly; see the server log",
        ))
    });
    drop(heartbeat);
    let mut redis = app.jobs.redis.clone();
    mutex::release(&mut redis, &row.id, &token).await;
    result
}

/// [`run_restore`] with the lock held.
async fn restore_locked<F>(
    app: &AppState,
    row: entity::issue::Model,
    actor_id: Uuid,
    actor_ip: Option<String>,
    actor_ua: Option<String>,
    rebuilt_from: Option<String>,
    swap: F,
) -> Result<RestoreDone, RestoreFailure>
where
    F: FnOnce(
            &crate::library::no_follow::LibraryRoot,
            &std::path::Path,
        ) -> Result<archive_rewrite::RestoreOutcome, archive_rewrite::RewriteError>
        + Send
        + 'static,
{
    // The handler's preflight and backup check ran before the lock was
    // claimed: run them again on fresh rows now that nothing else can
    // rewrite the issue. A page edit may have converted `X.cbr` to `X.cbz`
    // in between; restoring against the stale path would rename the
    // conversion's `X.cbr.bak` back to `X.cbr` beside the `.cbz`.
    let (fresh, _lib) = preflight_checked(app, &row.id).await?;
    if fresh.file_path != row.file_path {
        return Err(RestoreFailure::new(
            StatusCode::CONFLICT,
            "archive.issue_changed",
            "the issue's archive changed while the restore was starting (it was renamed or \
             converted); nothing was restored. Try again.",
        ));
    }
    if let Some(name) = &rebuilt_from {
        let still_listed = listed_backups(std::path::Path::new(&fresh.file_path))
            .await
            .iter()
            .any(|b| b.pre_conversion && &b.name == name);
        if !still_listed {
            return Err(RestoreFailure::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "archive.backup_not_restorable",
                BACKUP_NOT_RESTORABLE,
            ));
        }
    }

    // Relative to the folder handle from the no-follow walk; a link at the
    // backup or the archive (or on the folder path) is refused. The library
    // lookup comes first, so no database connection is held while waiting
    // for the archive-work permit below.
    let target = std::path::PathBuf::from(&row.file_path);
    let restore_res = match crate::library::no_follow::library_root(app, row.library_id).await {
        Ok(Some(root)) => {
            // A rebuild reads and writes a whole archive: it takes an
            // archive-work permit like the scan-time conversion.
            let permit = if rebuilt_from.is_some() {
                match app.archive_work_semaphore.clone().acquire_owned().await {
                    Ok(p) => Some(p),
                    Err(e) => {
                        tracing::error!(error = %e, "archive restore: archive work semaphore closed");
                        return Err(RestoreFailure::new(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "internal",
                            "internal",
                        ));
                    }
                }
            } else {
                None
            };
            let res = tokio::task::spawn_blocking(move || swap(&root, &target)).await;
            drop(permit);
            res
        }
        Ok(None) => Ok(Err(archive_rewrite::RewriteError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "library not found",
        )))),
        // A failed lookup is a server fault (answered 500 below), never
        // "no backup".
        Err(e) => Ok(Err(archive_rewrite::RewriteError::Io(
            std::io::Error::other(format!("library lookup failed: {e}")),
        ))),
    };

    let restored = match restore_res {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(archive_rewrite::RewriteError::Io(e)))
            if e.kind() == std::io::ErrorKind::NotFound =>
        {
            return Err(RestoreFailure::new(
                StatusCode::NOT_FOUND,
                "archive.no_backup",
                "no backup is available to restore",
            ));
        }
        Ok(Err(e @ archive_rewrite::RewriteError::Refused(_))) => {
            // The backup or the archive is a symbolic link, is not a
            // regular file, or a folder on the way is a link: nothing was
            // restored. Not "no backup": there is something there, Folio
            // will not use it.
            tracing::warn!(issue_id = %row.id, error = %e, "archive restore: refused");
            return Err(RestoreFailure::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "archive.restore_refused",
                "the backup or the archive is a symbolic link or not a regular file; \
                 nothing was restored",
            ));
        }
        Ok(Err(
            e @ (archive_rewrite::RewriteError::ArchiveErr(_)
            | archive_rewrite::RewriteError::ValidationFailed(_)),
        )) => {
            // Only the rebuild of a pre-conversion backup reads an archive:
            // a corrupt or unreadable backup, or one over a limit. Nothing
            // was swapped.
            let reason = rebuild_failure_reason(&e);
            tracing::warn!(
                issue_id = %row.id,
                error = %bounded_text(&e.to_string(), 300),
                "archive restore: the backup could not be rebuilt",
            );
            return Err(RestoreFailure::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "archive.restore_conversion_failed",
                format!(
                    "the pre-conversion backup could not be rebuilt as a CBZ; nothing was \
                     changed ({reason})"
                ),
            ));
        }
        Ok(Err(archive_rewrite::RewriteError::Io(e)))
            if e.kind() == std::io::ErrorKind::InvalidInput =>
        {
            return Err(RestoreFailure::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "archive.backup_not_restorable",
                "the named backup is not a pre-conversion backup of this issue's archive",
            ));
        }
        Ok(Err(e)) => {
            tracing::error!(error = %e, "archive restore: failed");
            return Err(RestoreFailure::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
        Err(e) => {
            tracing::error!(error = %e, "archive restore: join failed");
            return Err(RestoreFailure::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    let kept_as = restored
        .kept
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());
    let warning = restored.warning;
    let kept_label = restored.kept.as_deref().map(archive_rewrite::file_label);
    let message = match (&rebuilt_from, &kept_label) {
        (Some(from), kept) => format!(
            "The archive was rebuilt as a CBZ from the pre-conversion backup {from} (the \
             original bytes stay available in that backup){}.",
            kept.as_ref()
                .map(|k| format!("; the replaced archive is kept as {k}"))
                .unwrap_or_default()
        ),
        (None, Some(k)) => format!("The backup was restored; the replaced archive is kept as {k}."),
        (None, None) => "The backup was restored.".to_owned(),
    };

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
            actor_id,
            action: "admin.issue.archive_restore",
            target_type: Some("issue"),
            target_id: Some(row.id.clone()),
            payload: serde_json::json!({
                "issue_id": row.id,
                "file_path": row.file_path,
                "kept_as": kept_as,
                "warning": warning,
                "rebuilt_from": rebuilt_from,
                "message": message,
            }),
            ip: actor_ip,
            user_agent: actor_ua,
        },
    )
    .await;

    Ok(RestoreDone {
        kept_as,
        warning,
        rebuilt_from,
        message,
    })
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BackupView {
    /// Retention slot: 0 = most recent (`.bak`), 1 = `.bak.1`, …
    pub slot: i32,
    pub path: String,
    /// The backup's file name: what `POST …/archive/restore?backup=` takes.
    pub name: String,
    /// RFC3339 modification time of the backup file, or null if
    /// unavailable. For a hard-linked backup (the usual kind) this is the
    /// ORIGINAL archive's modification time, not when the backup was made:
    /// show `made_at` as the backup's date.
    pub modified_at: Option<String>,
    /// RFC3339 time the backup was made (the file's inode change time,
    /// `archive_rewrite::backup_made_at`, the clock the retention sweep
    /// ages backups by), or null when that cannot be known (no change time,
    /// or one earlier than the modification time); the sweep keeps such a
    /// backup. A later rotation, restore or permission change moves it
    /// forward, never back.
    pub made_at: Option<String>,
    pub size_bytes: u64,
    /// The backup of the archive this issue's file was converted from
    /// (`X.cbr.bak` / `X.cb7.bak[.n]` beside the issue's `X.cbz`), not of
    /// the current file. Listed so it can be found; restore does not use
    /// it as is: restoring it (`?backup=<name>`) rebuilds it as a CBZ into
    /// the issue's file, and the backup itself stays untouched.
    pub pre_conversion: bool,
}

/// The backups the listing offers for the archive at `target`: its own
/// `<file>.bak[.n]` by slot, then the backups of the archive it was
/// converted from (`X.cbr.bak[.n]` / `X.cb7.bak[.n]`), sorted by slot and
/// name, never in directory order. Regular files only (no link followed).
/// Restore resolves a named backup against exactly this set.
async fn listed_backups(target: &std::path::Path) -> Vec<BackupView> {
    let mut out = Vec::new();
    for slot in 0..=archive_rewrite::MAX_BACKUP_SLOT {
        let p = archive_rewrite::backup_slot(target, slot);
        // Listed without following links: only a regular file is a backup.
        let Ok(meta) = tokio::fs::symlink_metadata(&p).await else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        out.push(backup_view(slot, &p, &meta, false));
    }

    // A converted archive's original kept its old name (`X.cbr.bak` beside
    // `X.cbz`): look for those in the folder too. Read-only; sorted, so the
    // answer never depends on the directory's listing order.
    if let (Some(dir), Some(own)) = (target.parent(), target.file_name().and_then(|n| n.to_str()))
        && let Ok(mut entries) = tokio::fs::read_dir(dir).await
    {
        let mut found = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(slot) = archive_rewrite::pre_conversion_backup_slot(own, name) else {
                continue;
            };
            let p = entry.path();
            let Ok(meta) = tokio::fs::symlink_metadata(&p).await else {
                continue;
            };
            if meta.is_file() {
                found.push((name.to_owned(), slot, p, meta));
            }
        }
        found.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
        out.extend(
            found
                .iter()
                .map(|(_, slot, p, meta)| backup_view(*slot, p, meta, true)),
        );
    }
    out
}

fn backup_view(
    slot: i32,
    path: &std::path::Path,
    meta: &std::fs::Metadata,
    pre_conversion: bool,
) -> BackupView {
    let rfc3339 = |t: std::time::SystemTime| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339();
    BackupView {
        slot,
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: path.to_string_lossy().into_owned(),
        modified_at: meta.modified().ok().map(rfc3339),
        made_at: archive_rewrite::backup_made_at(meta).map(rfc3339),
        size_bytes: meta.len(),
        pre_conversion,
    }
}

#[utoipa::path(
    operation_id = "archive_edit_backups",    get,
    path = "/issues/{id}/archive/backups",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = [BackupView], description = "the issue file's own backups by slot, then the pre-conversion backups"),
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
    let out = listed_backups(&target).await;
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
    // N-A1: every filesystem touch below goes through `tokio::fs` or
    // `spawn_blocking` so a 32 MiB upload never stalls a runtime worker.
    let uploads_dir = app.cfg().data_path.join("uploads");
    {
        // Best-effort and only touches files older than an hour, so it runs
        // detached instead of delaying this upload.
        let dir = uploads_dir.clone();
        drop(tokio::task::spawn_blocking(move || {
            sweep_stale_uploads(&dir)
        }));
    }
    if let Err(e) = tokio::fs::create_dir_all(&uploads_dir).await {
        tracing::error!(error = %e, "uploads: mkdir failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    let id = Uuid::now_v7();
    let dest = uploads_dir.join(id.to_string());
    // Stream into a `.part` sibling and rename only once the bytes are
    // complete and sniffed as an image: the edit job never sees a
    // half-written or non-image staged file under the final name. An
    // abandoned `.part` (crash mid-upload) is swept like any stale upload.
    let part = uploads_dir.join(format!("{id}.part"));
    // Backstop for a cancelled request (client gone, `UPLOAD_ROUTE_TIMEOUT`):
    // the future is dropped mid-stream and the explicit cleanup below never
    // runs, so the guard removes the `.part` (best effort; anything it
    // misses, such as a create still in flight on the blocking pool, is
    // left to the hourly sweep).
    let mut guard = PartFileGuard(Some(part.clone()));
    let fmt = match stream_upload_field(&mut multipart, &part).await {
        Ok(fmt) => fmt,
        Err(resp) => {
            let _ = tokio::fs::remove_file(&part).await;
            guard.disarm();
            return resp;
        }
    };
    if let Err(e) = tokio::fs::rename(&part, &dest).await {
        tracing::error!(error = %e, "uploads: rename failed");
        let _ = tokio::fs::remove_file(&part).await;
        guard.disarm();
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    guard.disarm();

    (
        StatusCode::CREATED,
        Json(UploadView {
            id: id.to_string(),
            content_type: fmt.to_mime_type().to_owned(),
        }),
    )
        .into_response()
}

/// Removes a staged `.part` file when dropped while still armed; see
/// [`upload`]. The removal is spawned (a `Drop` can't await) onto the
/// current runtime.
struct PartFileGuard(Option<std::path::PathBuf>);

impl PartFileGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for PartFileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take()
            && let Ok(rt) = tokio::runtime::Handle::try_current()
        {
            rt.spawn(async move {
                let _ = tokio::fs::remove_file(&path).await;
            });
        }
    }
}

/// Write-buffer size for streaming a staged upload to disk.
const UPLOAD_WRITE_BUFFER: usize = 1024 * 1024;

/// Bytes kept in memory from the head of the upload for
/// [`image::guess_format`], which only inspects leading magic numbers.
const SNIFF_PREFIX_BYTES: usize = 64;

/// Stream the multipart `file` field chunk by chunk into `part`, never
/// holding more than one chunk (plus the sniff prefix) in memory. Returns
/// the sniffed image format, or the error response to send. The caller
/// removes `part` on error.
///
/// Error mapping matches the pre-streaming handler: over the cap → 413
/// `too_large`; a malformed multipart stream → 422 `validation`; no `file`
/// field → 422 `validation`; not an image → 422 `validation.not_an_image`.
/// A body that trips the route's `DefaultBodyLimit` surfaces from the
/// multipart reader with status 413 and maps to `too_large` as well; one
/// that stalls past `app::UPLOAD_BODY_IDLE_TIMEOUT` maps to 408
/// `request_timeout`.
async fn stream_upload_field(
    multipart: &mut Multipart,
    part: &std::path::Path,
) -> Result<image::ImageFormat, Response> {
    use tokio::io::AsyncWriteExt;

    let multipart_error = |e: axum::extract::multipart::MultipartError| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            too_large()
        } else if is_body_idle_timeout(&e) {
            error(
                StatusCode::REQUEST_TIMEOUT,
                "request_timeout",
                "upload stalled: no data received in time",
            )
        } else {
            error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                &e.to_string(),
            )
        }
    };
    let write_failed = |e: std::io::Error| {
        tracing::error!(error = %e, "uploads: write failed");
        error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
    };

    loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(e) => return Err(multipart_error(e)),
        };
        if field.name() != Some("file") {
            continue;
        }
        // Buffered so a 32 MiB upload is a few dozen blocking-pool writes,
        // not one per multipart chunk.
        let mut file = tokio::io::BufWriter::with_capacity(
            UPLOAD_WRITE_BUFFER,
            tokio::fs::File::create(part).await.map_err(write_failed)?,
        );
        let mut written: usize = 0;
        let mut prefix: Vec<u8> = Vec::with_capacity(SNIFF_PREFIX_BYTES);
        while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
            written = written.saturating_add(chunk.len());
            if written > MAX_UPLOAD_BYTES {
                return Err(too_large());
            }
            if prefix.len() < SNIFF_PREFIX_BYTES {
                let take = (SNIFF_PREFIX_BYTES - prefix.len()).min(chunk.len());
                prefix.extend_from_slice(&chunk[..take]);
            }
            file.write_all(&chunk).await.map_err(write_failed)?;
        }
        file.flush().await.map_err(write_failed)?;
        // Validate it's an image we can decode/re-encode later.
        return image::guess_format(&prefix).map_err(|_| {
            error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.not_an_image",
                "file is not a supported image",
            )
        });
    }
    Err(error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation",
        "missing `file` field",
    ))
}

/// Whether a multipart read failed because the long-body group's
/// `TimeoutBody` (`app::UPLOAD_BODY_IDLE_TIMEOUT`) gave up on a stalled
/// client. The error is buried a few `source()` levels down (multer →
/// axum body error → tower-http), so walk the chain.
fn is_body_idle_timeout(e: &axum::extract::multipart::MultipartError) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if err.is::<tower_http::timeout::TimeoutError>() {
            return true;
        }
        cur = err.source();
    }
    false
}

fn too_large() -> Response {
    super::respond(
        StatusCode::PAYLOAD_TOO_LARGE,
        shared::error::ApiErrorCode::TooLarge,
        "image exceeds 32 MiB",
    )
}

/// Best-effort removal of staged uploads older than one hour — they're
/// consumed by an edit shortly after upload, so anything older is
/// abandoned. Runs opportunistically on each upload, on the blocking
/// pool (it walks the directory with `std::fs`); errors are logged and
/// swallowed.
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
