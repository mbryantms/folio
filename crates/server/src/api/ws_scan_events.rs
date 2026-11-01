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
    Router,
    extract::{
        Query, Request, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use entity::user::{self, Entity as UserEntity, UserRole};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::auth::extractor::{AuthRejection, admit_state};
use crate::auth::{CurrentUser, ws_ticket};
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> Router<AppState> {
    Router::new().route("/ws/scan-events", get(scan_events))
}

/// Largest client→server message accepted on `/ws/scan-events` (N-A2).
/// The socket is server→client only: clients send nothing but control
/// frames (ping / pong / close, ≤125-byte payloads by RFC 6455), and the
/// read loop in [`run`] ignores any data frame. tungstenite applies these
/// limits to *incoming* data only, so the scan-event stream the server
/// writes (a few hundred bytes per event, a few KiB for a large health
/// summary) is not affected. 64 KiB leaves room for a small JSON command
/// should one ever be added; anything larger fails the read and the
/// socket is closed. tungstenite's defaults were 64 MiB / 16 MiB.
pub const WS_MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Largest single incoming frame on `/ws/scan-events`; see
/// [`WS_MAX_MESSAGE_BYTES`].
pub const WS_MAX_FRAME_BYTES: usize = 16 * 1024;

#[derive(Debug, Deserialize)]
pub struct AuthQuery {
    /// Optional one-time ticket from `POST /auth/ws-ticket`. When absent,
    /// the handler falls back to cookie-based auth via [`CurrentUser`].
    pub ticket: Option<String>,
}

#[handler]
pub async fn scan_events(
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
    // Same account-state rule as `CurrentUser`: only `active` is admitted.
    if let Err(rejection) = admit_state(row.state) {
        return rejection.into_response();
    }
    if row.role != UserRole::Admin {
        return super::error(
            StatusCode::FORBIDDEN,
            "auth.permission_denied",
            "admin only",
        );
    }
    // Auth passed — now extract the WS upgrade. Any non-upgrade request
    // (curl probe, test oneshot post-auth) returns 400 with the standard
    // axum upgrade-headers rejection.
    let (mut parts, _body) = req.into_parts();
    match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(ws) => ws
            .max_message_size(WS_MAX_MESSAGE_BYTES)
            .max_frame_size(WS_MAX_FRAME_BYTES)
            .on_upgrade(move |socket| run(socket, app))
            .into_response(),
        Err(e) => e.into_response(),
    }
}

fn unauthorized() -> axum::response::Response {
    super::error(
        StatusCode::UNAUTHORIZED,
        "auth.required",
        "Authentication required",
    )
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
