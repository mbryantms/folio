//! `/me/app-passwords` — manage long-lived Bearer credentials (M7, audit M-14).
//!
//! Three endpoints, all `CurrentUser`-gated:
//! - `POST   /me/app-passwords` — issue a new password; returns the plaintext exactly once.
//! - `GET    /me/app-passwords` — list active passwords (no plaintext, with last_used_at).
//! - `DELETE /me/app-passwords/{id}` — soft-delete (revoked_at = now).
//!
//! Audit log: `user.app_password.create` / `user.app_password.revoke`.

use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get},
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use entity::app_password::{self, Entity as AppPasswordEntity};

use crate::audit::{self, AuditEntry};
use crate::auth::{CurrentUser, app_password as ap};
use crate::middleware::RequestContext;
use crate::state::AppState;

const MAX_LABEL_LEN: usize = 80;
const MAX_ACTIVE_PER_USER: usize = 25;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me/app-passwords", get(list).post(create))
        .route("/me/app-passwords/{id}", delete(revoke))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateAppPasswordReq {
    /// Free-form label so the user can tell their tokens apart.
    /// 1-80 characters; trimmed.
    pub label: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AppPasswordView {
    pub id: Uuid,
    pub label: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AppPasswordListView {
    pub items: Vec<AppPasswordView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AppPasswordCreatedView {
    pub id: Uuid,
    pub label: String,
    pub created_at: String,
    /// The plaintext token. Shown once and never retrievable again.
    pub plaintext: String,
}

fn view(row: app_password::Model) -> AppPasswordView {
    AppPasswordView {
        id: row.id,
        label: row.label,
        created_at: row.created_at.to_rfc3339(),
        last_used_at: row.last_used_at.map(|t| t.to_rfc3339()),
    }
}

#[utoipa::path(
    get,
    path = "/me/app-passwords",
    responses(
        (status = 200, body = AppPasswordListView),
        (status = 401, description = "not authenticated"),
    )
)]
pub async fn list(State(app): State<AppState>, user: CurrentUser) -> impl IntoResponse {
    match AppPasswordEntity::find()
        .filter(app_password::Column::UserId.eq(user.id))
        .filter(app_password::Column::RevokedAt.is_null())
        .order_by_desc(app_password::Column::CreatedAt)
        .all(&app.db)
        .await
    {
        Ok(rows) => Json(AppPasswordListView {
            items: rows.into_iter().map(view).collect(),
        })
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "list app passwords failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

#[utoipa::path(
    post,
    path = "/me/app-passwords",
    request_body = CreateAppPasswordReq,
    responses(
        (status = 201, body = AppPasswordCreatedView),
        (status = 400, description = "label invalid"),
        (status = 401, description = "not authenticated"),
        (status = 409, description = "too many active passwords"),
    )
)]
pub async fn create(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<CreateAppPasswordReq>,
) -> impl IntoResponse {
    let label = req.label.trim().to_owned();
    if label.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "validation.label",
            "label cannot be empty",
        );
    }
    if label.len() > MAX_LABEL_LEN {
        return error(
            StatusCode::BAD_REQUEST,
            "validation.label",
            "label must be 80 characters or fewer",
        );
    }

    // Per-user soft cap. Bearer auth has to argon2-scan every active
    // row; cap the working set so a stray script can't push it into the
    // hundreds.
    let active_count = match AppPasswordEntity::find()
        .filter(app_password::Column::UserId.eq(user.id))
        .filter(app_password::Column::RevokedAt.is_null())
        .count(&app.db)
        .await
    {
        Ok(c) => c as usize,
        Err(e) => {
            tracing::error!(error = %e, "app password count failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if active_count >= MAX_ACTIVE_PER_USER {
        return error(
            StatusCode::CONFLICT,
            "auth.too_many_app_passwords",
            "revoke an existing app password before issuing a new one",
        );
    }

    let (id, plaintext) =
        match ap::issue(&app.db, user.id, &label, app.secrets.pepper.as_ref()).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "issue app password failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };

    // Load the inserted row for the response timestamps.
    let row = match AppPasswordEntity::find_by_id(id).one(&app.db).await {
        Ok(Some(r)) => r,
        _ => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "user.app_password.create",
            target_type: Some("app_password"),
            target_id: Some(id.to_string()),
            payload: serde_json::json!({ "label": label }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    let body = AppPasswordCreatedView {
        id: row.id,
        label: row.label,
        created_at: row.created_at.to_rfc3339(),
        plaintext,
    };
    (StatusCode::CREATED, Json(body)).into_response()
}

#[utoipa::path(
    delete,
    path = "/me/app-passwords/{id}",
    params(("id" = Uuid, Path)),
    responses(
        (status = 204, description = "revoked"),
        (status = 404, description = "not found or not owned by caller"),
    )
)]
pub async fn revoke(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let row = match AppPasswordEntity::find_by_id(id).one(&app.db).await {
        Ok(Some(r)) if r.user_id == user.id => r,
        Ok(_) => {
            return error(StatusCode::NOT_FOUND, "not_found", "app password not found");
        }
        Err(e) => {
            tracing::error!(error = %e, "app password lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if row.revoked_at.is_some() {
        return StatusCode::NO_CONTENT.into_response();
    }
    let mut am = row.into_active_model();
    am.revoked_at = Set(Some(chrono::Utc::now().fixed_offset()));
    if let Err(e) = am.update(&app.db).await {
        tracing::error!(error = %e, "revoke app password failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "user.app_password.revoke",
            target_type: Some("app_password"),
            target_id: Some(id.to_string()),
            payload: serde_json::json!({}),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    StatusCode::NO_CONTENT.into_response()
}

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
