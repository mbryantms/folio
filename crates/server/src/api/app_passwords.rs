//! `/me/app-passwords` — manage long-lived Bearer credentials (M7, audit M-14).
//!
//! Three endpoints, all `CurrentUser`-gated:
//! - `POST   /me/app-passwords` — issue a new password; returns the plaintext exactly once.
//! - `GET    /me/app-passwords` — list active passwords (no plaintext, with last_used_at).
//! - `DELETE /me/app-passwords/{id}` — soft-delete (revoked_at = now).
//!
//! Audit log: `user.app_password.create` / `user.app_password.revoke`.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use entity::app_password::{self, Entity as AppPasswordEntity};

use super::error;
use super::extractors::Validated;
use crate::audit::{self, AuditEntry};
use crate::auth::{CurrentUser, app_password as ap};
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

const MAX_ACTIVE_PER_USER: usize = 25;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(create))
        .routes(routes!(revoke))
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct CreateAppPasswordReq {
    /// Free-form label so the user can tell their tokens apart.
    /// 1-80 characters; trimmed.
    #[garde(length(max = 80), custom(label_non_empty_after_trim))]
    pub label: String,
    /// Optional scope tag — `read` (default) or `read+progress`.
    /// Tokens with `read` can browse, page-stream, and download; the
    /// progress-write surface (PUT `/opds/v1/issues/{id}/progress` and
    /// the KOReader sync shim) requires `read+progress`.
    #[serde(default)]
    #[garde(custom(valid_scope_or_default))]
    pub scope: Option<String>,
}

fn label_non_empty_after_trim(value: &str, _: &()) -> garde::Result {
    if value.trim().is_empty() {
        return Err(garde::Error::new("label cannot be empty"));
    }
    Ok(())
}

/// Allow `None` (defaults to `read` at handler time), or a non-empty
/// scope string that `ap::is_valid_scope` accepts. Whitespace-only
/// strings are treated as "not provided" — the handler does the
/// same trim+filter, so any rejection from this validator and the
/// in-handler unwrap-to-default agree on the empty case.
fn valid_scope_or_default(value: &Option<String>, _: &()) -> garde::Result {
    let Some(raw) = value else {
        return Ok(());
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if !ap::is_valid_scope(trimmed) {
        return Err(garde::Error::new("scope must be 'read' or 'read+progress'"));
    }
    Ok(())
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AppPasswordView {
    pub id: Uuid,
    pub label: String,
    pub scope: String,
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
    pub scope: String,
    pub created_at: String,
    /// The plaintext token. Shown once and never retrievable again.
    pub plaintext: String,
}

fn view(row: app_password::Model) -> AppPasswordView {
    AppPasswordView {
        id: row.id,
        label: row.label,
        scope: row.scope,
        created_at: row.created_at.to_rfc3339(),
        last_used_at: row.last_used_at.map(|t| t.to_rfc3339()),
    }
}

#[utoipa::path(
    operation_id = "app_passwords_list",    get,
    path = "/me/app-passwords",
    responses(
        (status = 200, body = AppPasswordListView),
        (status = 401, description = "not authenticated"),
    )
)]
#[handler]
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
    operation_id = "app_passwords_create",    post,
    path = "/me/app-passwords",
    request_body = CreateAppPasswordReq,
    responses(
        (status = 201, body = AppPasswordCreatedView),
        (status = 400, description = "label invalid"),
        (status = 401, description = "not authenticated"),
        (status = 409, description = "too many active passwords"),
    )
)]
#[handler]
pub async fn create(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Validated(req): Validated<CreateAppPasswordReq>,
) -> impl IntoResponse {
    // Garde already enforced length(max=80) + non-empty-after-trim
    // on the raw struct; here we just normalize for storage.
    let label = req.label.trim().to_owned();

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

    // Scope defaults to `read` for backwards compat with pre-M7 clients
    // that don't know about the field. `read+progress` requires the
    // user explicitly opt in via the UI selector.
    let scope = req
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(ap::SCOPE_READ);
    // Garde's `valid_scope_or_default` already gates this; we keep
    // the lookup here purely for the default-fallback semantics.

    let (id, plaintext) =
        match ap::issue(&app.db, user.id, &label, scope, app.secrets.pepper.as_ref()).await {
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
            payload: serde_json::json!({ "label": label, "scope": scope }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    let body = AppPasswordCreatedView {
        id: row.id,
        label: row.label,
        scope: row.scope,
        created_at: row.created_at.to_rfc3339(),
        plaintext,
    };
    (StatusCode::CREATED, Json(body)).into_response()
}

#[utoipa::path(
    operation_id = "app_passwords_revoke",    delete,
    path = "/me/app-passwords/{id}",
    params(("id" = Uuid, Path)),
    responses(
        (status = 204, description = "revoked"),
        (status = 404, description = "not found or not owned by caller"),
    )
)]
#[handler]
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
