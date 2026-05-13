//! `/admin/email/*` — operational probes for the transactional email
//! pipeline (M2 of the runtime-config-admin plan).
//!
//! - `GET /admin/email/status` — reads `AppState::email_status`: whether
//!   SMTP is wired, when the last send happened, and whether it succeeded.
//! - `POST /admin/email/test` — sends a no-op verification email to the
//!   calling admin's address (the same template the recovery flow uses)
//!   so an operator can confirm the relay end-to-end after saving credentials.
//!
//! Both routes are gated by [`RequireAdmin`]. The test-send endpoint
//! audit-logs as `admin.email.test`.

use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;

use crate::audit::{self, AuditEntry};
use crate::auth::RequireAdmin;
use crate::email::Email;
use crate::middleware::RequestContext;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/email/status", get(status))
        .route("/admin/email/test", post(test_send))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EmailStatusView {
    /// `true` when the live sender is a real transport (Lettre or Mock),
    /// `false` when SMTP is unset and the Noop fallback is installed.
    pub configured: bool,
    pub last_send_at: Option<String>,
    pub last_send_ok: Option<bool>,
    pub last_error: Option<String>,
    pub last_duration_ms: Option<u64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TestEmailResp {
    pub delivered: bool,
    pub duration_ms: u64,
    pub to: String,
}

#[utoipa::path(
    get,
    path = "/admin/email/status",
    responses(
        (status = 200, body = EmailStatusView),
        (status = 403, description = "admin only"),
    )
)]
pub async fn status(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let s = app.email_status.read().await.clone();
    Json(EmailStatusView {
        configured: s.configured,
        last_send_at: s.last_send_at.map(|t| t.to_rfc3339()),
        last_send_ok: s.last_send_ok,
        last_error: s.last_error,
        last_duration_ms: s.last_duration_ms,
    })
    .into_response()
}

#[utoipa::path(
    post,
    path = "/admin/email/test",
    responses(
        (status = 200, body = TestEmailResp),
        (status = 400, description = "no email on caller's account"),
        (status = 403, description = "admin only"),
        (status = 502, description = "transport error"),
    )
)]
pub async fn test_send(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
) -> Response {
    let Some(to) = actor.email.clone() else {
        return error(
            StatusCode::BAD_REQUEST,
            "email.no_recipient",
            "your admin account has no email address; set one before testing",
        );
    };

    // A purpose-built test template — short, distinct from the real
    // recovery emails so an admin testing the relay doesn't get confused
    // with a stale verify-email link from a previous run.
    let public_url = app.cfg().public_url.trim_end_matches('/').to_owned();
    let when = chrono::Utc::now().to_rfc3339();
    let email = Email {
        to: to.clone(),
        subject: "Folio SMTP test email".to_owned(),
        body_text: format!(
            "This is a test email from Folio ({public_url}) sent at {when}.\n\n\
             If you received this, your SMTP configuration is working.\n\n\
             Triggered by admin {} via /admin/email/test.",
            actor.display_name,
        ),
        body_html: Some(format!(
            "<p>This is a test email from Folio (<a href=\"{public_url}\">{public_url}</a>) \
             sent at {when}.</p>\
             <p>If you received this, your SMTP configuration is working.</p>\
             <p style=\"color:#555;font-size:13px;\">Triggered by admin {} via \
             <code>/admin/email/test</code>.</p>",
            actor.display_name,
        )),
    };

    let started = std::time::Instant::now();
    let send_result = app.send_email(email).await;
    let duration_ms = started.elapsed().as_millis() as u64;

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.email.test",
            target_type: Some("email"),
            target_id: Some(to.clone()),
            payload: serde_json::json!({
                "delivered": send_result.is_ok(),
                "duration_ms": duration_ms,
                "error": send_result.as_ref().err().map(ToString::to_string),
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    match send_result {
        Ok(()) => Json(TestEmailResp {
            delivered: true,
            duration_ms,
            to,
        })
        .into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "admin email test send failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": {
                        "code": "email.send_failed",
                        "message": e.to_string(),
                    }
                })),
            )
                .into_response()
        }
    }
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
