//! First-import lazy-hash progress (roadmap WP-3.2).
//!
//!   - `GET  /libraries/{slug}/hash-backfill` — pending / total counts for
//!     the library settings page's progress bar.
//!   - `POST /libraries/{slug}/hash-backfill` — (re)enqueue the drain, e.g.
//!     after a restart abandoned it. Scans also enqueue it automatically.
//!
//! See [`crate::jobs::hash_backfill`] for the job itself.

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
use crate::audit::{self, AuditEntry};
use crate::auth::RequireAdmin;
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(hash_backfill_status, hash_backfill_start))
}

/// Content-hash backfill progress for one library.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct HashBackfillView {
    /// Live issues whose full-file BLAKE3 is still pending.
    pub pending: u64,
    /// Live issues in the library.
    pub total: u64,
    /// `total - pending`.
    pub hashed: u64,
    /// The library's `trust_fingerprint_on_first_import` opt-in.
    pub enabled: bool,
    /// Whether new files are currently ingested without hashing — the
    /// opt-in is on and the library has never completed a full scan.
    pub first_import_active: bool,
}

/// Response for `POST /libraries/{slug}/hash-backfill`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct HashBackfillStartResp {
    /// `false` when there was nothing pending (no job pushed).
    pub enqueued: bool,
    pub pending: u64,
}

async fn view(
    app: &AppState,
    lib: &entity::library::Model,
) -> Result<HashBackfillView, axum::response::Response> {
    match crate::jobs::hash_backfill::progress(&app.db, lib.id).await {
        Ok((pending, total)) => Ok(HashBackfillView {
            pending,
            total,
            hashed: total.saturating_sub(pending),
            enabled: lib.trust_fingerprint_on_first_import,
            first_import_active: crate::library::scanner::process::lazy_hash_eligible(lib),
        }),
        Err(e) => {
            tracing::error!(library_id = %lib.id, error = %e, "hash backfill progress query failed");
            Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ))
        }
    }
}

#[utoipa::path(
    operation_id = "libraries_hash_backfill_status",
    get,
    path = "/libraries/{slug}/hash-backfill",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = HashBackfillView),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
#[handler]
pub async fn hash_backfill_status(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let lib = match super::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match view(&app, &lib).await {
        Ok(v) => Json(v).into_response(),
        Err(resp) => resp,
    }
}

#[utoipa::path(
    operation_id = "libraries_hash_backfill_start",
    post,
    path = "/libraries/{slug}/hash-backfill",
    params(("slug" = String, Path,)),
    responses(
        (status = 202, body = HashBackfillStartResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
#[handler]
pub async fn hash_backfill_start(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let lib = match super::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let enqueued = crate::jobs::hash_backfill::enqueue_if_pending(&app, lib.id).await;
    let pending = match view(&app, &lib).await {
        Ok(v) => v.pending,
        Err(resp) => return resp,
    };
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.library.hash_backfill.start",
            target_type: Some("library"),
            target_id: Some(lib.id.to_string()),
            payload: serde_json::json!({ "enqueued": enqueued, "pending": pending }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    (
        StatusCode::ACCEPTED,
        Json(HashBackfillStartResp { enqueued, pending }),
    )
        .into_response()
}
