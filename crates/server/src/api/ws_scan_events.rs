//! `GET /ws/scan-events` — live scan progress over WebSocket (spec §8.1).
//!
//! Library Scanner v1, Milestone 10.
//!
//! Subscribers receive JSON-encoded [`ScanEvent`]s as text frames. Auth is
//! admin-only and accepts either:
//!   - `?ticket=<uuid>` — one-time ticket minted by `POST /auth/ws-ticket`
//!     (the dev path: page is on :3000, API is on :8080, so the auth cookie
//!     can't ride along on the cross-origin upgrade).
//!   - cookie session — the prod path, when the page is served same-origin
//!     by the Rust binary.
//!
//! On lagged receivers (broadcast channel overflow), we send a `lagged`
//! ping frame and continue; clients that care can refresh.

use axum::extract::FromRequestParts;
use axum::{
    Json, Router,
    extract::{
        Query, Request, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use entity::user::{self, Entity as UserEntity};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::auth::extractor::AuthRejection;
use crate::auth::{CurrentUser, ws_ticket};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/ws/scan-events", get(handler))
}

#[derive(Debug, Deserialize)]
pub struct AuthQuery {
    /// Optional one-time ticket from `POST /auth/ws-ticket`. When absent,
    /// the handler falls back to cookie-based auth via [`CurrentUser`].
    pub ticket: Option<String>,
}

pub async fn handler(
    Query(q): Query<AuthQuery>,
    State(app): State<AppState>,
    cookie_user: Result<CurrentUser, AuthRejection>,
    req: Request,
) -> impl IntoResponse {
    // Auth is checked before the upgrade-header validation so a missing /
    // bad ticket consistently returns 401 — which is what every client
    // (including our own ticket flow) expects on retry. If we let the
    // `WebSocketUpgrade` extractor reject first, an unauthenticated probe
    // would see a 400 (missing upgrade headers) and clients couldn't
    // distinguish "auth failed, mint a new ticket" from "wrong protocol".
    let user_id: Uuid = match q.ticket.as_deref().filter(|s| !s.is_empty()) {
        Some(t) => match ws_ticket::consume(&app, t).await {
            Ok((id, _role)) => id,
            Err(reason) => {
                tracing::debug!(reason = %reason, "ws_scan_events: ticket consume failed");
                return unauthorized();
            }
        },
        None => match cookie_user {
            Ok(cu) => cu.id,
            Err(_) => return unauthorized(),
        },
    };

    // M3 (S-6): re-resolve the user row from the DB on upgrade. The ticket
    // is one-shot but is minted before the WS handshake; if the user is
    // disabled in the interim, the ticket payload's stale `role` would
    // otherwise still admit them. Same check the `CurrentUser` extractor
    // does on every cookie-authed request.
    let row = match UserEntity::find()
        .filter(user::Column::Id.eq(user_id))
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return unauthorized(),
        Err(e) => {
            tracing::error!(error = %e, "ws_scan_events: user lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
        }
    };
    if row.state == "disabled" {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": {"code": "auth.disabled", "message": "Account disabled"}})),
        )
            .into_response();
    }
    if row.role != "admin" {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": {"code": "auth.permission_denied", "message": "admin only"}})),
        )
            .into_response();
    }
    // Auth passed — now extract the WS upgrade. Any non-upgrade request
    // (curl probe, test oneshot post-auth) returns 400 with the standard
    // axum upgrade-headers rejection.
    let (mut parts, _body) = req.into_parts();
    match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(ws) => ws
            .on_upgrade(move |socket| run(socket, app))
            .into_response(),
        Err(e) => e.into_response(),
    }
}

fn unauthorized() -> axum::response::Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error": {"code": "auth.required", "message": "Authentication required"}})),
    )
        .into_response()
}

async fn run(mut socket: WebSocket, app: AppState) {
    let mut rx = app.events.subscribe();
    loop {
        tokio::select! {
            // Outgoing: scan events.
            evt = rx.recv() => match evt {
                Ok(e) => {
                    let payload = match serde_json::to_string(&e) {
                        Ok(s) => s,
                        Err(err) => {
                            tracing::error!(error = %err, "ws_scan_events: serialize failed");
                            continue;
                        }
                    };
                    if socket.send(Message::Text(payload.into())).await.is_err() {
                        break; // Client gone.
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    let _ = socket
                        .send(Message::Text(
                            serde_json::json!({"type": "lagged", "skipped": n}).to_string().into(),
                        ))
                        .await;
                }
                Err(RecvError::Closed) => break,
            },
            // Incoming: only watch for client close.
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(_)) => break,
                _ => {}
            },
        }
    }
}
