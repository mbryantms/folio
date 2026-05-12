//! WebSocket auth tickets (§9.6).
//!
//! Cookie-authed clients call POST /auth/ws-ticket and receive a one-time
//! UUID with a 30-second TTL. The WS upgrade handler accepts the ticket via
//! `?ticket=` query param as an alternative to the cookie, since browser
//! dev mode (Next on :3000, API on :8080) can't share the auth cookie
//! across origins on the WS handshake. In production the cookie path still
//! works because everything is served from the same origin.
//!
//! Storage: Redis `ws_ticket:{uuid}` → JSON `{ user_id, role }`, EX=30.
//! Consumption: GETDEL ensures one-time use.
//!
//! Spec §9.6 also mentions per-IP bucket rate limiting (30/s). The general
//! `auth/csrf` + global rate-limit middleware already gates this endpoint;
//! a dedicated bucket on top is a follow-up.

use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
use serde::Serialize;
use uuid::Uuid;

use crate::auth::CurrentUser;
use crate::middleware::rate_limit;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/auth/ws-ticket",
        post(mint).route_layer(rate_limit::WS_TICKET.build()),
    )
}

const TICKET_TTL_SECS: u64 = 30;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WsTicketResp {
    pub ticket: String,
    pub expires_in: u64,
}

fn redis_key(ticket: &str) -> String {
    format!("ws_ticket:{ticket}")
}

#[utoipa::path(
    post,
    path = "/auth/ws-ticket",
    responses(
        (status = 200, body = WsTicketResp),
        (status = 401, description = "auth required"),
        (status = 500, description = "redis error"),
    )
)]
pub async fn mint(
    axum::extract::State(app): axum::extract::State<AppState>,
    user: CurrentUser,
) -> impl IntoResponse {
    let ticket = Uuid::now_v7().to_string();
    let payload = serde_json::json!({
        "user_id": user.id.to_string(),
        "role": user.role,
    })
    .to_string();
    let mut conn = app.jobs.redis.clone();
    let res: Result<(), redis::RedisError> = redis::cmd("SET")
        .arg(redis_key(&ticket))
        .arg(&payload)
        .arg("EX")
        .arg(TICKET_TTL_SECS)
        .query_async(&mut conn)
        .await;
    if let Err(e) = res {
        tracing::error!(error = %e, "ws_ticket: redis SET failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": {"code": "internal", "message": "internal"}})),
        )
            .into_response();
    }
    Json(WsTicketResp {
        ticket,
        expires_in: TICKET_TTL_SECS,
    })
    .into_response()
}

/// Atomically consume a ticket — GETDEL returns the value AND deletes the
/// key in a single round-trip, so a leaked ticket cannot be replayed.
pub async fn consume(app: &AppState, ticket: &str) -> Result<(Uuid, String), &'static str> {
    let mut conn = app.jobs.redis.clone();
    let raw: Option<String> = redis::cmd("GETDEL")
        .arg(redis_key(ticket))
        .query_async(&mut conn)
        .await
        .map_err(|_| "redis_error")?;
    let raw = raw.ok_or("ticket_not_found")?;
    let v: serde_json::Value = serde_json::from_str(&raw).map_err(|_| "ticket_corrupt")?;
    let id = v["user_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or("ticket_corrupt")?;
    let role = v["role"].as_str().ok_or("ticket_corrupt")?.to_string();
    Ok((id, role))
}
