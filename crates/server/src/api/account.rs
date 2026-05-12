//! `PATCH /me/account` — self-serve profile updates (M4).
//!
//! Currently supports:
//!   - `display_name` — any non-empty trimmed string
//!   - `email` — local users only; OIDC users get a 403 because the
//!     issuer owns their email
//!   - `current_password` + `new_password` — local users only; argon2id rehash
//!
//! The endpoint is CSRF-protected (cookie auth, unsafe verb). All mutations
//! emit an audit-log row keyed on the user themself as the actor and target.

use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::{patch, post},
};
use axum_extra::extract::CookieJar;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;

use entity::user::{self, ActiveModel as UserAM, Entity as UserEntity};

use crate::api::form_or_json::{FormOrJson, ResponseFormat, redirect_with_error};
use crate::audit::{self, AuditEntry};
use crate::auth::CurrentUser;
use crate::auth::cookies::{csrf_cookie, new_csrf_token};
use crate::auth::local::{MeResp, me_resp_from_row};
use crate::auth::password;
use crate::middleware::RequestContext;
use crate::state::AppState;

// Access TTL flows from config; the CSRF cookie max-age must match the access
// cookie's so JS doesn't think the form is signed-in after the access cookie
// expires. Read at handler time via `app.cfg.access_ttl()`.

pub fn routes() -> Router<AppState> {
    // PATCH is the JSON contract (XHR happy path); POST is the
    // form-fallback alias so an HTML `<form method="POST">` can target
    // the same handler. Both wire through the same `update` function;
    // the CSRF middleware accepts the hidden `csrf_token` form field as
    // an alternative to the `X-CSRF-Token` header (see `auth/csrf.rs`).
    Router::new()
        .route("/me/account", patch(update))
        .route("/me/account", post(update))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AccountReq {
    #[serde(default)]
    pub display_name: Option<String>,
    /// New email. Local users only — OIDC users cannot self-edit (the issuer
    /// owns this field).
    #[serde(default)]
    pub email: Option<String>,
    /// Required when changing the password. Verified before any write.
    #[serde(default)]
    pub current_password: Option<String>,
    /// New password. Must be ≥ 12 chars per the local-auth policy.
    #[serde(default)]
    pub new_password: Option<String>,
    /// Optional second password field. The web form ships two `<input>`s
    /// for confirmation; submitting them as `new_password` + `confirm_password`
    /// lets the no-JS fallback validate parity server-side. Ignored when
    /// absent or equal.
    #[serde(default)]
    pub confirm_password: Option<String>,
}

#[utoipa::path(
    patch,
    path = "/me/account",
    request_body = AccountReq,
    responses(
        (status = 200, body = MeResp),
        (status = 400, description = "validation error"),
        (status = 401, description = "current_password incorrect or session invalid"),
        (status = 403, description = "OIDC users cannot edit email/password"),
        (status = 409, description = "email already in use"),
    )
)]
pub async fn update(
    State(app): State<AppState>,
    user: CurrentUser,
    jar: CookieJar,
    Extension(ctx): Extension<RequestContext>,
    FormOrJson { data: req, format }: FormOrJson<AccountReq>,
) -> impl IntoResponse {
    // Form-fallback failures bounce back to /settings/account with the
    // banner. JSON path keeps the existing envelope.
    let settings_target = "/settings/account";
    let fail = |status: StatusCode, code: &str, msg: &str| -> axum::response::Response {
        match format {
            ResponseFormat::Json => error(status, code, msg),
            ResponseFormat::Form => {
                Redirect::to(&redirect_with_error(settings_target, code, msg, None)).into_response()
            }
        }
    };

    let row = match UserEntity::find_by_id(user.id).one(&app.db).await {
        Ok(Some(r)) => r,
        _ => return fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };

    // Determine the auth mode for the user — OIDC users have a
    // `oidc:<issuer>|<sub>` external_id and a NULL password_hash.
    let is_local = row.external_id.starts_with("local:");

    if let Some(name) = req.display_name.as_deref()
        && name.trim().is_empty()
    {
        return fail(
            StatusCode::BAD_REQUEST,
            "validation.display_name",
            "display_name cannot be empty",
        );
    }

    if let Some(email) = req.email.as_ref() {
        if !is_local {
            return fail(
                StatusCode::FORBIDDEN,
                "auth.email_managed_by_issuer",
                "email is managed by your identity provider",
            );
        }
        let lower = email.trim().to_lowercase();
        if !lower.contains('@') || lower.len() > 254 {
            return fail(StatusCode::BAD_REQUEST, "validation.email", "invalid email");
        }
        if Some(&lower) != row.email.as_ref()
            && let Ok(Some(other)) = UserEntity::find()
                .filter(user::Column::Email.eq(lower.clone()))
                .one(&app.db)
                .await
            && other.id != row.id
        {
            return fail(StatusCode::CONFLICT, "conflict", "email already in use");
        }
    }

    let password_change = req.new_password.is_some() || req.current_password.is_some();
    if password_change {
        if !is_local {
            return fail(
                StatusCode::FORBIDDEN,
                "auth.password_managed_by_issuer",
                "password is managed by your identity provider",
            );
        }
        let Some(new_pw) = req.new_password.as_deref() else {
            return fail(
                StatusCode::BAD_REQUEST,
                "validation.new_password",
                "new_password is required when current_password is provided",
            );
        };
        let Some(current_pw) = req.current_password.as_deref() else {
            return fail(
                StatusCode::BAD_REQUEST,
                "validation.current_password",
                "current_password is required to change the password",
            );
        };
        if new_pw.len() < 12 {
            return fail(
                StatusCode::BAD_REQUEST,
                "validation.new_password",
                "new password must be at least 12 characters",
            );
        }
        // Optional confirm-password parity (form fallback only — JS validates
        // before submit). If the field is present and doesn't match, reject.
        if let Some(confirm) = req.confirm_password.as_deref()
            && confirm != new_pw
        {
            return fail(
                StatusCode::BAD_REQUEST,
                "validation.confirm_password",
                "passwords do not match",
            );
        }
        let Some(stored) = row.password_hash.as_ref() else {
            return fail(
                StatusCode::FORBIDDEN,
                "auth.password_managed_by_issuer",
                "this account has no local password",
            );
        };
        let ok = password::verify(stored, current_pw, app.secrets.pepper.as_ref()).unwrap_or(false);
        if !ok {
            return fail(
                StatusCode::UNAUTHORIZED,
                "auth.invalid",
                "current password is incorrect",
            );
        }
    }

    let mut am: UserAM = row.clone().into();
    let mut changed = serde_json::Map::new();
    let mut bump_token_version = false;

    if let Some(name) = req.display_name {
        let trimmed = name.trim().to_owned();
        if trimmed != row.display_name {
            changed.insert("display_name".into(), serde_json::json!(trimmed));
            am.display_name = Set(trimmed);
        }
    }
    if let Some(email) = req.email {
        let lower = email.trim().to_lowercase();
        if Some(&lower) != row.email.as_ref() {
            changed.insert("email".into(), serde_json::json!(lower));
            am.email = Set(Some(lower));
            // Email change requires re-verification when SMTP is configured.
            // For now we don't have the verification flow, so treat the new
            // email as unverified to keep the door open for a follow-up.
            am.email_verified = Set(false);
        }
    }
    if let Some(new_pw) = req.new_password {
        let hashed = match password::hash(&new_pw, app.secrets.pepper.as_ref()) {
            Ok(h) => h,
            Err(e) => {
                tracing::error!(error = %e, "argon2 rehash failed");
                return fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
        changed.insert("password".into(), serde_json::json!("changed"));
        am.password_hash = Set(Some(hashed));
        bump_token_version = true;
    }

    if changed.is_empty() {
        let csrf = new_csrf_token();
        let body = me_resp_from_row(&row, csrf.clone());
        let jar = jar.add(csrf_cookie(csrf, app.cfg.access_ttl()));
        return match format {
            ResponseFormat::Json => (StatusCode::OK, jar, Json(body)).into_response(),
            ResponseFormat::Form => {
                (jar, Redirect::to("/settings/account?ok=1")).into_response()
            }
        };
    }

    am.updated_at = Set(chrono::Utc::now().fixed_offset());
    if bump_token_version {
        am.token_version = Set(row.token_version + 1);
    }
    let updated = match am.update(&app.db).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "account update failed");
            return fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "user.account.update",
            target_type: Some("user"),
            target_id: Some(user.id.to_string()),
            payload: serde_json::Value::Object(changed),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    let csrf = new_csrf_token();
    let body = me_resp_from_row(&updated, csrf.clone());
    let jar = jar.add(csrf_cookie(csrf, app.cfg.access_ttl()));
    match format {
        ResponseFormat::Json => (StatusCode::OK, jar, Json(body)).into_response(),
        ResponseFormat::Form => (jar, Redirect::to("/settings/account?ok=1")).into_response(),
    }
}

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
