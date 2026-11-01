//! Library-wide sidecar re-emit (audit 2026-11 A7, decision D-6).
//!
//!   - `POST /libraries/{slug}/sidecar-reemit` — start a run that rewrites
//!     the ComicInfo / MetronInfo sidecars of every active issue from the
//!     database with the current composer. Writeback libraries only.
//!   - `GET  /libraries/{slug}/sidecar-reemit` — the library's latest run
//!     (live or finished in the last week), for the settings card.
//!
//! The job itself is [`crate::jobs::sidecar_reemit`]; progress for every
//! run also shows on `/admin/background-work`.

use axum::{
    Extension, Json,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::error;
use crate::auth::RequireAdmin;
use crate::jobs::sidecar_reemit::{self, RunSnapshot, StartError};
use crate::metadata::manual_writeback::Actor;
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(sidecar_reemit_status, sidecar_reemit_start))
}

/// One library-wide sidecar re-emit run.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SidecarReemitRunView {
    pub run_id: String,
    pub library_id: String,
    /// `running` (queueing rewrites) | `draining` (all queued, waiting for
    /// the last rewrites) | `completed` | `aborted`.
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// Active, non-removed issues when the run started.
    pub total: i64,
    /// Rewrites queued so far.
    pub enqueued: i64,
    /// Archives the sidecar path refused (CBR / CB7 without conversion,
    /// unsupported formats): skipped, never queued.
    pub skipped_refused: i64,
    /// Issues removed between the walk and the compose.
    pub skipped_gone: i64,
    /// Archives whose sidecars were rewritten.
    pub rewritten: i64,
    /// Rewrites that gave up after their retries.
    pub failed: i64,
    /// Rewrites queued or running right now.
    pub outstanding: i64,
    /// Live, but no tick has run for 15 minutes (a dead-lettered tick or a
    /// cleared queue): nothing will advance it, and starting a new run
    /// supersedes it.
    pub stalled: bool,
    /// Why an aborted run stopped.
    pub note: Option<String>,
}

impl From<RunSnapshot> for SidecarReemitRunView {
    fn from(r: RunSnapshot) -> Self {
        Self {
            run_id: r.run_id.to_string(),
            library_id: r.library_id.to_string(),
            status: r.status,
            started_at: r.started_at,
            finished_at: r.finished_at,
            total: r.total,
            enqueued: r.enqueued,
            skipped_refused: r.refused,
            skipped_gone: r.gone,
            rewritten: r.rewritten,
            failed: r.failed,
            outstanding: r.outstanding,
            stalled: r.stalled,
            note: r.note,
        }
    }
}

/// `GET /libraries/{slug}/sidecar-reemit`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SidecarReemitStatusView {
    /// The library's newest run still on record; `None` when there is none.
    pub run: Option<SidecarReemitRunView>,
}

#[utoipa::path(
    operation_id = "libraries_sidecar_reemit_status",
    get,
    path = "/libraries/{slug}/sidecar-reemit",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = SidecarReemitStatusView),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
#[handler]
pub async fn sidecar_reemit_status(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let lib = match super::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match sidecar_reemit::latest_for_library(&app, lib.id).await {
        Ok(run) => Json(SidecarReemitStatusView {
            run: run.map(Into::into),
        })
        .into_response(),
        Err(e) => {
            tracing::error!(library_id = %lib.id, error = %e, "sidecar re-emit status read failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

#[utoipa::path(
    operation_id = "libraries_sidecar_reemit_start",
    post,
    path = "/libraries/{slug}/sidecar-reemit",
    params(("slug" = String, Path,)),
    responses(
        (status = 202, body = SidecarReemitRunView),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
        (status = 409, description = "writeback is off on this library, or a run is already in progress"),
    )
)]
#[handler]
pub async fn sidecar_reemit_start(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let lib = match super::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let job_actor = Actor {
        id: Some(actor.id),
        ip: ctx.ip_string(),
        user_agent: ctx.user_agent.clone(),
    };
    let run = match sidecar_reemit::start(&app, &lib, &job_actor).await {
        Ok(run) => run,
        Err(StartError::NotWriteback) => {
            return error(
                StatusCode::CONFLICT,
                "writeback_disabled",
                "this library does not have both archive writeback and metadata writeback on",
            );
        }
        Err(StartError::InProgress(run_id)) => {
            return error(
                StatusCode::CONFLICT,
                "reemit_in_progress",
                &format!("a sidecar re-emit is already running for this library (run {run_id})"),
            );
        }
        Err(StartError::Other(e)) => {
            tracing::error!(library_id = %lib.id, error = %e, "sidecar re-emit start failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    crate::record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.library.sidecars.reemit",
        target = ("library", lib.id.to_string()),
        payload = serde_json::json!({
            "run_id": run.run_id,
            "total": run.total,
        }),
    );
    (StatusCode::ACCEPTED, Json(SidecarReemitRunView::from(run))).into_response()
}
