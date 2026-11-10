//! `PATCH /me/account` — self-serve profile updates (M4).
//!
//! Currently supports:
//!   - `display_name` — any non-empty trimmed string
//!   - `email` — local users only; OIDC users get a 403 because the
//!     issuer owns their email. A change needs `current_password` (when the
//!     account has one; wrong passwords count toward the failed-auth
//!     lockout). With mail delivery configured the address does NOT change
//!     here: an `email_change_requests` row is stored, a single-use link
//!     goes to the new address and a notice to the old one, and the
//!     response carries `pending_email`. The link
//!     (`GET /auth/local/confirm-email-change`) applies it. Without mail
//!     delivery there is no way to confirm, so the change applies at once
//!     (unverified) and the other sessions are signed out.
//!   - `current_password` + `new_password` — local users only; argon2id rehash
//!
//! The endpoint is CSRF-protected (cookie auth, unsafe verb). All mutations
//! emit an audit-log row keyed on the user themself as the actor and target.

use axum::{
    Extension, Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Redirect},
};
use axum_extra::extract::CookieJar;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set, TransactionTrait};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use entity::email_change_request::{
    self, ActiveModel as EmailChangeAM, Entity as EmailChangeEntity,
};
use entity::user::{self, ActiveModel as UserAM, Entity as UserEntity};

use super::{error, respond_with_field_errors};
use crate::api::form_or_json::{FormOrJson, ResponseFormat, redirect_with_error};
use crate::audit::{self, AuditEntry};
use crate::auth::cookies::{csrf_cookie, new_csrf_token, sha256_hex};
use crate::auth::email_token;
use crate::auth::local::{
    EMAIL_CHANGE_TTL, MeResp, has_local_password, me_resp_from_row, with_pending_email,
};
use crate::auth::{CurrentUser, failed_auth, password};
use crate::email::templates;
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;
use shared::error::{ApiErrorCode, FieldError};

// Access TTL flows from config; the CSRF cookie max-age must match the access
// cookie's so JS doesn't think the form is signed-in after the access cookie
// expires. Read at handler time via `app.cfg().access_ttl()`.

pub fn routes() -> OpenApiRouter<AppState> {
    // PATCH is the JSON contract (XHR happy path); POST is the
    // form-fallback alias so an HTML `<form method="POST">` can target
    // the same handler. The utoipa::path annotation on `update` only
    // declares PATCH; the POST alias is added explicitly for the
    // form-fallback path (no spec entry needed — the JSON contract
    // is the public one).
    OpenApiRouter::new()
        .routes(routes!(update))
        .route("/me/account", axum::routing::post(update))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AccountReq {
    #[serde(default)]
    pub display_name: Option<String>,
    /// New email. Local users only — OIDC users cannot self-edit (the issuer
    /// owns this field).
    #[serde(default)]
    pub email: Option<String>,
    /// Required when changing the password, and when changing the email
    /// of an account that has a password. Verified before any write.
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
    operation_id = "account_update",    patch,
    path = "/me/account",
    request_body = AccountReq,
    responses(
        (status = 200, body = MeResp, description = "updated. After an email change with mail delivery configured, `email` is unchanged and `pending_email` names the address the confirmation link went to"),
        (status = 400, description = "validation error"),
        (status = 401, description = "current_password incorrect (password change) or session invalid"),
        (status = 403, description = "OIDC users cannot edit email/password, or current_password incorrect for an email change"),
        (status = 409, description = "email already in use"),
        (status = 422, description = "validation error; a missing current_password for an email change carries a field error"),
        (status = 429, description = "locked out after failed password attempts"),
    )
)]
#[handler]
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
    // Same, with the message bound to `current_password` for the form.
    let fail_password = |status: StatusCode, code: ApiErrorCode, msg: &str| match format {
        ResponseFormat::Json => respond_with_field_errors(
            status,
            code,
            msg,
            vec![FieldError {
                field: "current_password".to_owned(),
                message: msg.to_owned(),
            }],
        ),
        ResponseFormat::Form => Redirect::to(&redirect_with_error(
            settings_target,
            code.as_str(),
            msg,
            None,
        ))
        .into_response(),
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
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.display_name",
            "display_name cannot be empty",
        );
    }

    // The normalized new address when the request changes it.
    let mut new_email: Option<String> = None;
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
            return fail(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.email",
                "invalid email",
            );
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
        if Some(&lower) != row.email.as_ref() {
            new_email = Some(lower);
        }
    }

    // An email change re-authenticates with the password when the account
    // has one (an attacker holding a session for a moment must not be able
    // to point the account at their own address and reset the password).
    // Wrong passwords count against both failed-auth axes, as `DELETE /me`
    // does, and answer 403 (401 reads as "session expired" to the web
    // client).
    let mut reauthed = false;
    if new_email.is_some() && has_local_password(&row) {
        let stored = row.password_hash.as_deref().unwrap_or_default();
        let Some(given) = req.current_password.as_deref().filter(|p| !p.is_empty()) else {
            return fail_password(
                StatusCode::UNPROCESSABLE_ENTITY,
                ApiErrorCode::Validation,
                "enter your current password to change your email",
            );
        };
        let email_key = row
            .email
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(str::to_lowercase);
        if let Some(ip) = ctx.client_ip
            && let Ok(Some(retry)) = failed_auth::check_lockout_for(&app, ip).await
        {
            return failed_auth::lockout_response(retry);
        }
        if let Some(key) = email_key.as_deref()
            && let Ok(Some(retry)) = failed_auth::check_lockout_for_email(&app, key).await
        {
            return failed_auth::lockout_response(retry);
        }
        let ok =
            password::verify_rotating(stored, given, app.secrets.peppers()).is_ok_and(|v| v.ok());
        if !ok {
            failed_auth::record_failure_for(&app, &ctx).await;
            if let Some(key) = email_key.as_deref() {
                failed_auth::record_failure_for_email(&app, key).await;
            }
            tracing::info!(reason = "bad_password", "email change rejected");
            return fail_password(
                StatusCode::FORBIDDEN,
                ApiErrorCode::AuthInvalid,
                "current password is incorrect",
            );
        }
        reauthed = true;
    }

    // `current_password` alone (no email change) still means "change the
    // password" so the missing-`new_password` error below keeps firing.
    let password_change =
        req.new_password.is_some() || (req.current_password.is_some() && new_email.is_none());
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
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.new_password",
                "new_password is required when current_password is provided",
            );
        };
        let Some(current_pw) = req.current_password.as_deref() else {
            return fail(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.current_password",
                "current_password is required to change the password",
            );
        };
        if new_pw.len() < 12 {
            return fail(
                StatusCode::UNPROCESSABLE_ENTITY,
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
                StatusCode::UNPROCESSABLE_ENTITY,
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
        // Rotation-aware (L-1): a hash still under the previous pepper must
        // verify here too; the password change below rewrites it anyway.
        // Already checked above when the same request changes the email.
        let ok = reauthed
            || password::verify_rotating(stored, current_pw, app.secrets.peppers())
                .is_ok_and(|v| v.ok());
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
    // Mail delivery configured: same signal register and `/auth/config`
    // use (`smtp_host` set; `email::build` installs the logging no-op
    // sender exactly when it is not).
    let mail_configured = app
        .cfg()
        .smtp_host
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty());
    // The address a confirmation link goes to (mail configured).
    let mut pending: Option<String> = None;
    let mut immediate_email_change = false;
    if let Some(lower) = new_email {
        if mail_configured {
            pending = Some(lower);
        } else {
            // No way to confirm: apply now, unverified, and sign the other
            // sessions out as a password change does.
            tracing::warn!(
                user_id = %user.id,
                "email changed without confirmation: mail delivery is not configured"
            );
            changed.insert("email".into(), serde_json::json!(lower));
            changed.insert("email_confirmed".into(), serde_json::json!(false));
            am.email = Set(Some(lower));
            am.email_verified = Set(false);
            bump_token_version = true;
            immediate_email_change = true;
        }
    }
    if let Some(new_pw) = req.new_password {
        let hashed = match password::hash(
            &new_pw,
            app.secrets.pepper.as_ref(),
            app.cfg().password_hash_cost,
        ) {
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

    if changed.is_empty() && pending.is_none() {
        let csrf = new_csrf_token();
        let body = with_pending_email(&app.db, me_resp_from_row(&row, csrf.clone())).await;
        let jar = jar.add(csrf_cookie(csrf, app.cfg().access_ttl()));
        return match format {
            ResponseFormat::Json => (StatusCode::OK, jar, Json(body)).into_response(),
            ResponseFormat::Form => (jar, Redirect::to("/settings/account?ok=1")).into_response(),
        };
    }

    let updated = if changed.is_empty() {
        row.clone()
    } else {
        am.updated_at = Set(chrono::Utc::now().fixed_offset());
        if bump_token_version {
            am.token_version = Set(row.token_version + 1);
        }
        match am.update(&app.db).await {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "account update failed");
                return fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    };

    if immediate_email_change {
        // A request left over from when mail was configured is moot now.
        let _ = EmailChangeEntity::delete_many()
            .filter(email_change_request::Column::UserId.eq(user.id))
            .exec(&app.db)
            .await;
    }
    if let Some(new_addr) = pending.as_deref() {
        if let Err(e) = store_and_send_email_change(&app, &row, new_addr).await {
            tracing::error!(error = %e, user_id = %user.id, "email-change request failed");
            return fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
        changed.insert("email_change".into(), serde_json::json!("requested"));
    }

    // AUTH-1: a password change must not leave stolen refresh sessions alive.
    // Revoke every OTHER session for this user; keep the caller's current
    // session (identified by its refresh cookie) so the active browser stays
    // signed in — its access token, invalidated by the token_version bump
    // above, transparently re-mints via /auth/refresh.
    if bump_token_version {
        let keep = jar
            .get(crate::auth::cookies::REFRESH_COOKIE)
            .map(|c| crate::auth::cookies::sha256_hex(c.value()));
        crate::auth::local::revoke_sessions_for_user(&app.db, user.id, keep.as_deref()).await;
    }

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
    let body = with_pending_email(&app.db, me_resp_from_row(&updated, csrf.clone())).await;
    let jar = jar.add(csrf_cookie(csrf, app.cfg().access_ttl()));
    let form_target = if pending.is_some() {
        "/settings/account?email_change=sent"
    } else {
        "/settings/account?ok=1"
    };
    match format {
        ResponseFormat::Json => (StatusCode::OK, jar, Json(body)).into_response(),
        ResponseFormat::Form => (jar, Redirect::to(form_target)).into_response(),
    }
}

/// Replace the user's pending email change with one for `new_addr`, then
/// mail the confirmation link to `new_addr` and a notice to the current
/// address. Send failures are logged, not returned: the request is stored
/// and a new PATCH replaces it.
async fn store_and_send_email_change(
    app: &AppState,
    row: &user::Model,
    new_addr: &str,
) -> Result<(), sea_orm::DbErr> {
    let token_id = uuid::Uuid::now_v7();
    let tok = email_token::issue_email_change(
        row.id,
        token_id,
        EMAIL_CHANGE_TTL,
        app.secrets.email_token_key.as_ref(),
    );
    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::seconds(EMAIL_CHANGE_TTL.as_secs() as i64);
    let txn = app.db.begin().await?;
    // One pending request per user (unique `user_id`): the new one replaces
    // the old, whose link stops working.
    EmailChangeEntity::delete_many()
        .filter(email_change_request::Column::UserId.eq(row.id))
        .exec(&txn)
        .await?;
    EmailChangeAM {
        token_id: Set(token_id),
        user_id: Set(row.id),
        new_email: Set(new_addr.to_owned()),
        token_hash: Set(sha256_hex(&tok)),
        expires_at: Set(expires_at.fixed_offset()),
        consumed_at: Set(None),
        created_at: Set(now.fixed_offset()),
    }
    .insert(&txn)
    .await?;
    txn.commit().await?;

    let public_url = app.cfg().public_url.clone();
    let confirm = templates::email_change_confirm(&public_url, new_addr, &tok);
    if let Err(e) = app.send_email(confirm).await {
        tracing::warn!(error = %e, user_id = %row.id, "email-change confirmation send failed");
    }
    if let Some(old) = row.email.as_deref().filter(|e| !e.trim().is_empty()) {
        let notice = templates::email_change_requested(&public_url, old, new_addr);
        if let Err(e) = app.send_email(notice).await {
            tracing::warn!(error = %e, user_id = %row.id, "email-change notice send failed");
        }
    }
    tracing::info!(user_id = %row.id, "email-change confirmation sent");
    Ok(())
}
