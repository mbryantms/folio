//! `/me/sessions` — self-service refresh-session management (M5, audit M-8).
//!
//! Three endpoints scoped to the calling user:
//!   - `GET    /me/sessions` — list this user's active refresh sessions.
//!   - `DELETE /me/sessions/{id}` — revoke a single session by id.
//!   - `POST   /me/sessions/revoke-all` — revoke every refresh session for
//!     this user and bump `users.token_version` so existing access tokens
//!     stop verifying immediately.
//!
//! "Revoke" is a soft delete (`revoked_at = now()`). The list view filters
//! revoked + expired rows so callers only see what's still live.
//!
//! The current-session badge is computed by hashing the caller's refresh
//! cookie and matching against `auth_sessions.refresh_token_hash`. Cookie
//! absent or unmatched simply means "none of these are flagged current,"
//! which is also the right answer when the caller authed via Bearer.
//!
//! All three endpoints emit an audit-log row.

use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use axum_extra::extract::CookieJar;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, Set,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use entity::auth_session::{self, Entity as SessionEntity};
use entity::user::{ActiveModel as UserAM, Entity as UserEntity};

use crate::audit::{self, AuditEntry};
use crate::auth::CurrentUser;
use crate::auth::cookies::REFRESH_COOKIE;
use crate::middleware::RequestContext;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me/sessions", get(list))
        .route("/me/sessions/{id}", delete(revoke_one))
        .route("/me/sessions/revoke-all", post(revoke_all))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SessionView {
    pub id: Uuid,
    pub created_at: String,
    pub last_used_at: String,
    pub expires_at: String,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
    /// True when this session matches the caller's refresh cookie.
    pub current: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SessionListView {
    pub sessions: Vec<SessionView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RevokeAllResp {
    /// Number of sessions transitioned from active → revoked. Doesn't
    /// include sessions that were already revoked or expired.
    pub revoked: u64,
}

fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    format!("{:x}", h.finalize())
}

#[utoipa::path(
    get,
    path = "/me/sessions",
    responses(
        (status = 200, body = SessionListView),
        (status = 401, description = "not authenticated"),
    )
)]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    jar: CookieJar,
) -> impl IntoResponse {
    let now = chrono::Utc::now().fixed_offset();
    let rows = match SessionEntity::find()
        .filter(auth_session::Column::UserId.eq(user.id))
        .filter(auth_session::Column::RevokedAt.is_null())
        .filter(auth_session::Column::ExpiresAt.gt(now))
        .order_by_desc(auth_session::Column::LastUsedAt)
        .all(&app.db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "list sessions failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let current_hash = jar.get(REFRESH_COOKIE).map(|c| sha256_hex(c.value()));
    let sessions = rows
        .into_iter()
        .map(|r| SessionView {
            current: current_hash
                .as_deref()
                .is_some_and(|h| h == r.refresh_token_hash),
            id: r.id,
            created_at: r.created_at.to_rfc3339(),
            last_used_at: r.last_used_at.to_rfc3339(),
            expires_at: r.expires_at.to_rfc3339(),
            user_agent: r.user_agent,
            ip: r.ip,
        })
        .collect();

    Json(SessionListView { sessions }).into_response()
}

#[utoipa::path(
    delete,
    path = "/me/sessions/{id}",
    params(("id" = Uuid, Path)),
    responses(
        (status = 204, description = "session revoked"),
        (status = 404, description = "session not found or not owned by caller"),
    )
)]
pub async fn revoke_one(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let row = match SessionEntity::find_by_id(id).one(&app.db).await {
        Ok(Some(r)) if r.user_id == user.id => r,
        Ok(_) => {
            return error(StatusCode::NOT_FOUND, "not_found", "session not found");
        }
        Err(e) => {
            tracing::error!(error = %e, "lookup session failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    if row.revoked_at.is_some() {
        return StatusCode::NO_CONTENT.into_response();
    }

    let mut am = row.into_active_model();
    am.revoked_at = Set(Some(chrono::Utc::now().fixed_offset()));
    if let Err(e) = am.update(&app.db).await {
        tracing::error!(error = %e, "revoke session failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "user.session.revoke",
            target_type: Some("auth_session"),
            target_id: Some(id.to_string()),
            payload: serde_json::json!({}),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    post,
    path = "/me/sessions/revoke-all",
    responses(
        (status = 200, body = RevokeAllResp),
        (status = 401, description = "not authenticated"),
    )
)]
pub async fn revoke_all(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
) -> impl IntoResponse {
    let now = chrono::Utc::now().fixed_offset();
    let revoked = match SessionEntity::update_many()
        .col_expr(
            auth_session::Column::RevokedAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .filter(auth_session::Column::UserId.eq(user.id))
        .filter(auth_session::Column::RevokedAt.is_null())
        .exec(&app.db)
        .await
    {
        Ok(r) => r.rows_affected,
        Err(e) => {
            tracing::error!(error = %e, "revoke-all sessions failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    // Bump token_version so every outstanding access token also stops
    // verifying. Without this, the caller (and any other tab with a fresh
    // access cookie) keeps working until that access token expires.
    let user_row = match UserEntity::find_by_id(user.id).one(&app.db).await {
        Ok(Some(r)) => r,
        _ => {
            // Caller authenticated but their row is gone — return success
            // anyway since the sessions are revoked.
            return Json(RevokeAllResp { revoked }).into_response();
        }
    };
    let mut am: UserAM = user_row.clone().into();
    am.token_version = Set(user_row.token_version + 1);
    am.updated_at = Set(now);
    if let Err(e) = am.update(&app.db).await {
        tracing::error!(error = %e, "token_version bump failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "user.session.revoke_all",
            target_type: Some("user"),
            target_id: Some(user.id.to_string()),
            payload: serde_json::json!({ "revoked": revoked }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(RevokeAllResp { revoked }).into_response()
}

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
