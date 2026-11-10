//! `PATCH /me/account` — self-serve profile updates (M4).
//!
//! Currently supports:
//!   - `display_name` — any non-empty trimmed string
//!   - `email` — local users only; OIDC users get a 403 because the
//!     issuer owns their email. A change needs `current_password` (when the
//!     account has one; wrong passwords count toward the failed-auth
//!     lockout); only past that check does a taken address answer 409.
//!     At most [`EMAIL_CHANGE_REQUESTS_PER_HOUR`] requests per hour (429).
//!     Submitting the current address cancels a pending change. With mail delivery configured the address does NOT change
//!     here: an `email_change_requests` row is stored, a single-use link
//!     goes to the new address and a notice to the old one, and the
//!     response carries `pending_email`. The link
//!     (`GET /auth/local/confirm-email-change`) applies it. Without mail
//!     delivery there is no way to confirm, so the change applies at once
//!     (unverified) and the other sessions are signed out.
//!   - `current_password` + `new_password` — local users only; argon2id
//!     rehash. The password is checked with the same lockout and failure
//!     counting as the email branch; a wrong one answers 403 `auth.invalid`.
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
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QuerySelect, Set, TransactionTrait,
};
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
use crate::auth::failed_auth::PasswordCheck;
use crate::auth::local::{
    EMAIL_CHANGE_TTL, MeResp, has_local_password, me_resp_from_row, request_is_live,
    with_pending_email,
};
use crate::auth::{CurrentUser, failed_auth, password};
use crate::config::AuthMode;
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

/// At most this many email-change requests per user per hour (each sends
/// two emails). Counted on the `email_change_requests` row.
pub const EMAIL_CHANGE_REQUESTS_PER_HOUR: i32 = 5;
const EMAIL_CHANGE_WINDOW: chrono::Duration = chrono::Duration::hours(1);

#[utoipa::path(
    operation_id = "account_update",    patch,
    path = "/me/account",
    request_body = AccountReq,
    responses(
        (status = 200, body = MeResp, description = "updated. After an email change with mail delivery configured, `email` is unchanged and `pending_email` names the address the confirmation link went to. Submitting the current address cancels a pending change"),
        (status = 400, description = "validation error"),
        (status = 401, description = "session invalid"),
        (status = 403, description = "OIDC users cannot edit email/password, or current_password incorrect (code `auth.invalid`, field error on `current_password`)"),
        (status = 409, description = "email already in use (checked after the password)"),
        (status = 422, description = "validation error; a missing current_password for an email change carries a field error"),
        (status = 429, description = "locked out after failed password attempts (`auth.locked_out`), or more than 5 email-change requests in an hour (`rate_limited`); both carry Retry-After"),
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
    // Lockout and the hourly email-change limit: 429 + Retry-After (JSON),
    // the settings banner (form).
    let fail_retry = |resp: axum::response::Response, code: &str, msg: &str| match format {
        ResponseFormat::Json => resp,
        ResponseFormat::Form => {
            Redirect::to(&redirect_with_error(settings_target, code, msg, None)).into_response()
        }
    };
    let locked_msg = "too many failed sign-in attempts; try again later";
    let wrong_msg = "current password is incorrect";

    let row = match UserEntity::find_by_id(user.id).one(&app.db).await {
        Ok(Some(r)) => r,
        _ => return fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };

    // Determine the auth mode for the user — OIDC users have a
    // `oidc:<issuer>|<sub>` external_id and a NULL password_hash.
    let is_local = row.external_id.starts_with("local:");
    // The no-JS form posts every field, so an empty value means "not given".
    let current_pw = req.current_password.as_deref().filter(|p| !p.is_empty());
    let new_pw = req.new_password.as_deref().filter(|p| !p.is_empty());

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
    // The request names the current address: cancel a pending change.
    let mut cancel_pending = false;
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
        let current = row.email.as_deref().map(|e| e.trim().to_lowercase());
        if current.as_deref() == Some(lower.as_str()) {
            cancel_pending = true;
        } else {
            // The confirm link only works while local auth is on (OIDC-only
            // mode refuses it), so refuse the change up front, before any
            // mail goes out.
            if !matches!(app.cfg().auth_mode, AuthMode::Local | AuthMode::Both) {
                return fail(
                    StatusCode::FORBIDDEN,
                    "auth.email_managed_by_issuer",
                    "email is managed by your identity provider",
                );
            }
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
        let Some(given) = current_pw else {
            return fail_password(
                StatusCode::UNPROCESSABLE_ENTITY,
                ApiErrorCode::Validation,
                "enter your current password to change your email",
            );
        };
        match failed_auth::verify_password_counted(&app, &ctx, &row, given).await {
            PasswordCheck::Ok => reauthed = true,
            PasswordCheck::LockedOut(retry) => {
                return fail_retry(
                    failed_auth::lockout_response(retry),
                    "auth.locked_out",
                    locked_msg,
                );
            }
            PasswordCheck::Internal => {
                return fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
            PasswordCheck::Wrong => {
                tracing::info!(reason = "bad_password", "email change rejected");
                return fail_password(StatusCode::FORBIDDEN, ApiErrorCode::AuthInvalid, wrong_msg);
            }
        }
    }

    // Only `new_password` makes a password change. A lone
    // `current_password` (the profile form always carries the field, and a
    // password manager may fill it) is ignored when the email is unchanged.
    let password_change = new_pw.is_some();
    if let Some(new_pw) = new_pw {
        if !is_local {
            return fail(
                StatusCode::FORBIDDEN,
                "auth.password_managed_by_issuer",
                "password is managed by your identity provider",
            );
        }
        let Some(current_pw) = current_pw else {
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
        if !has_local_password(&row) {
            return fail(
                StatusCode::FORBIDDEN,
                "auth.password_managed_by_issuer",
                "this account has no local password",
            );
        }
        // Same lockout gate and failure counting as the email branch
        // (already checked when the same request changes the email).
        // Lockout check → verify → count a failure, as `DELETE /me` does.
        if !reauthed {
            match failed_auth::verify_password_counted(&app, &ctx, &row, current_pw).await {
                PasswordCheck::Ok => {}
                PasswordCheck::LockedOut(retry) => {
                    return fail_retry(
                        failed_auth::lockout_response(retry),
                        "auth.locked_out",
                        locked_msg,
                    );
                }
                PasswordCheck::Internal => {
                    return fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
                }
                PasswordCheck::Wrong => {
                    tracing::info!(reason = "bad_password", "password change rejected");
                    return fail_password(
                        StatusCode::FORBIDDEN,
                        ApiErrorCode::AuthInvalid,
                        wrong_msg,
                    );
                }
            }
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
    let pending = new_email.as_ref().filter(|_| mail_configured).cloned();

    // Hash outside the transaction (argon2 is slow; the row lock is not
    // held for it).
    let new_hash = match new_pw.filter(|_| password_change) {
        Some(new_pw) => match password::hash_blocking(
            new_pw,
            app.secrets.pepper.as_ref(),
            app.cfg().password_hash_cost,
        )
        .await
        {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!(error = %e, "argon2 rehash failed");
                return fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        },
        None => None,
    };

    // All-or-nothing from here: the user row, the email-change request,
    // the session revocation and the audit row commit together, and the
    // hourly limit is checked before any of them is written. The user row
    // is locked first; it serializes concurrent PATCHes of one account
    // (and the confirm link, which locks in the same order).
    let internal = || fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "account update: begin failed");
            return internal();
        }
    };
    let locked = match UserEntity::find_by_id(user.id)
        .lock_exclusive()
        .one(&txn)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return internal(),
        Err(e) => {
            tracing::error!(error = %e, "account update: user lock failed");
            return internal();
        }
    };
    // The password was verified against `row`, read before the lock. If a
    // password change or reset, or sign-out-everywhere, committed since,
    // this request rides on credentials that are no longer valid: refuse it
    // as a wrong password and write nothing. (Requests that checked no
    // password, a display-name edit or a cancel, do not depend on them.)
    if (reauthed || password_change)
        && credentials_changed(credential_stamp(&row), credential_stamp(&locked))
    {
        tracing::info!(user_id = %user.id, "account update refused: credentials changed mid-request");
        return fail_password(StatusCode::FORBIDDEN, ApiErrorCode::AuthInvalid, wrong_msg);
    }
    // Only now, past the password and under the lock, say whether the
    // address is in use (the same answer registration gives). Compared as
    // `lower(email)`, like the unique index, which stays the backstop.
    if let Some(lower) = new_email.as_deref() {
        match email_in_use_by_other(&txn, lower, row.id).await {
            Ok(false) => {}
            Ok(true) => return fail(StatusCode::CONFLICT, "conflict", "email already in use"),
            Err(e) => {
                tracing::error!(error = %e, "email-change address lookup failed");
                return internal();
            }
        }
    }
    let now = chrono::Utc::now().fixed_offset();
    let prev = if pending.is_some() || cancel_pending {
        match EmailChangeEntity::find()
            .filter(email_change_request::Column::UserId.eq(user.id))
            .one(&txn)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "email-change request lookup failed");
                return internal();
            }
        }
    } else {
        None
    };
    let mut window = None;
    if pending.is_some() {
        match next_window(prev.as_ref(), now) {
            Ok(w) => window = Some(w),
            // Dropping `txn` rolls back; nothing was written.
            Err(retry) => return fail_retry(rate_limited(retry), "rate_limited", RATE_LIMITED_MSG),
        }
    }

    let mut am: UserAM = locked.clone().into();
    // Audit payload; `row_changed` tracks writes to the user row itself.
    let mut changed = serde_json::Map::new();
    let mut row_changed = false;
    let mut bump_token_version = false;

    if let Some(name) = req.display_name.as_deref() {
        let trimmed = name.trim().to_owned();
        if trimmed != locked.display_name {
            changed.insert("display_name".into(), serde_json::json!(trimmed));
            am.display_name = Set(trimmed);
            row_changed = true;
        }
    }
    let immediate_email_change = new_email.is_some() && !mail_configured;
    if let Some(lower) = new_email.as_ref().filter(|_| immediate_email_change) {
        // No way to confirm: apply now, unverified, and sign the other
        // sessions out as a password change does.
        tracing::warn!(
            user_id = %user.id,
            "email changed without confirmation: mail delivery is not configured"
        );
        changed.insert("email".into(), serde_json::json!(lower));
        changed.insert("previous_email".into(), serde_json::json!(locked.email));
        changed.insert("email_verified".into(), serde_json::json!(false));
        am.email = Set(Some(lower.clone()));
        am.email_verified = Set(false);
        bump_token_version = true;
        row_changed = true;
    }
    if let Some(hashed) = new_hash {
        changed.insert("password".into(), serde_json::json!("changed"));
        am.password_hash = Set(Some(hashed));
        bump_token_version = true;
        row_changed = true;
    }
    // Submitting the current address while a change is pending cancels it.
    let cancelled = prev
        .as_ref()
        .filter(|p| cancel_pending && request_is_live(p, locked.token_version, now))
        .map(|p| p.new_email.clone());

    if changed.is_empty() && pending.is_none() && cancelled.is_none() {
        drop(txn);
        let csrf = new_csrf_token();
        let body = with_pending_email(&app.db, me_resp_from_row(&locked, csrf.clone())).await;
        let jar = jar.add(csrf_cookie(csrf, app.cfg().access_ttl()));
        return match format {
            ResponseFormat::Json => (StatusCode::OK, jar, Json(body)).into_response(),
            ResponseFormat::Form => (jar, Redirect::to("/settings/account?ok=1")).into_response(),
        };
    }

    let updated = if row_changed {
        am.updated_at = Set(now);
        if bump_token_version {
            am.token_version = Set(locked.token_version + 1);
        }
        match am.update(&txn).await {
            Ok(m) => m,
            Err(e) if is_unique_violation(&e) => {
                // The unique index on lower(email) closes the race with the
                // check above.
                return fail(StatusCode::CONFLICT, "conflict", "email already in use");
            }
            Err(e) => {
                tracing::error!(error = %e, "account update failed");
                return internal();
            }
        }
    } else {
        locked.clone()
    };

    if immediate_email_change {
        // A request left over from when mail was configured is moot now.
        if let Err(e) = EmailChangeEntity::delete_many()
            .filter(email_change_request::Column::UserId.eq(user.id))
            .exec(&txn)
            .await
        {
            tracing::error!(error = %e, user_id = %user.id, "email-change request delete failed");
            return internal();
        }
    }
    if let Some(addr) = cancelled.as_deref() {
        // The row stays (expired) so the hourly count survives the cancel.
        if let Err(e) = EmailChangeEntity::update_many()
            .col_expr(
                email_change_request::Column::ExpiresAt,
                sea_orm::sea_query::Expr::value(now),
            )
            .filter(email_change_request::Column::UserId.eq(user.id))
            .exec(&txn)
            .await
        {
            tracing::error!(error = %e, user_id = %user.id, "email-change cancel failed");
            return internal();
        }
        changed.insert("email_change".into(), serde_json::json!("cancelled"));
        changed.insert("pending_email".into(), serde_json::json!(addr));
    }
    // The link to mail once the transaction commits.
    let mut link: Option<(String, String)> = None;
    if let (Some(new_addr), Some(window)) = (pending.as_deref(), window) {
        // `updated` carries the token_version a password change in this
        // same request produced, so that change does not void the link.
        match write_email_change_request(
            &app,
            &txn,
            &updated,
            new_addr,
            prev.is_some(),
            window,
            now,
        )
        .await
        {
            Ok(tok) => link = Some((new_addr.to_owned(), tok)),
            Err(e) => {
                tracing::error!(error = %e, user_id = %user.id, "email-change request write failed");
                return internal();
            }
        }
        changed.insert("email_change".into(), serde_json::json!("requested"));
        changed.insert("pending_email".into(), serde_json::json!(new_addr));
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
        crate::auth::local::revoke_sessions_for_user(&txn, user.id, keep.as_deref()).await;
    }

    if let Err(e) = audit::record_in(
        &txn,
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
    .await
    {
        tracing::error!(error = %e, user_id = %user.id, "account update audit write failed");
        return internal();
    }
    if let Err(e) = txn.commit().await {
        tracing::error!(error = %e, user_id = %user.id, "account update commit failed");
        return internal();
    }

    if let Some((new_addr, tok)) = link.as_ref() {
        send_email_change_mail(&app, &updated, new_addr, tok).await;
    }

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

const RATE_LIMITED_MSG: &str = "too many email-change requests; try again later";

/// 429 `rate_limited` with `Retry-After` and `retry_after_seconds`: the
/// rate-limit middleware's own envelope.
fn rate_limited(retry_after_seconds: u64) -> axum::response::Response {
    crate::middleware::rate_limit::rate_limited_response(RATE_LIMITED_MSG, retry_after_seconds)
}

/// The parts of a user row the password check depends on.
fn credential_stamp(row: &user::Model) -> (i64, Option<&str>) {
    (row.token_version, row.password_hash.as_deref())
}

/// True when the account's credentials changed between the unlocked read
/// the password was verified against and the locked read the writes use.
fn credentials_changed(before: (i64, Option<&str>), locked: (i64, Option<&str>)) -> bool {
    before != locked
}

/// True when another account holds `lower` (compared as `lower(email)`,
/// the unique index's expression).
pub(crate) async fn email_in_use_by_other<C: sea_orm::ConnectionTrait>(
    db: &C,
    lower: &str,
    user_id: uuid::Uuid,
) -> Result<bool, sea_orm::DbErr> {
    use sea_orm::sea_query::{Expr, ExprTrait, Func};
    Ok(UserEntity::find()
        .filter(Expr::expr(Func::lower(Expr::col(user::Column::Email))).eq(lower))
        .filter(user::Column::Id.ne(user_id))
        .one(db)
        .await?
        .is_some())
}

/// A unique-index violation (the email index, on the user row).
pub(crate) fn is_unique_violation(e: &sea_orm::DbErr) -> bool {
    matches!(
        e.sql_err(),
        Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
    )
}

/// The hourly window a new request falls in, as `(window start, count
/// including this request)`, carried over from the previous row while its
/// window is open; or the seconds until another request is allowed.
fn next_window(
    prev: Option<&email_change_request::Model>,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> Result<(chrono::DateTime<chrono::FixedOffset>, i32), u64> {
    match prev {
        Some(p) if p.window_started_at + EMAIL_CHANGE_WINDOW > now => {
            if p.request_count >= EMAIL_CHANGE_REQUESTS_PER_HOUR {
                let left = (p.window_started_at + EMAIL_CHANGE_WINDOW - now).num_seconds();
                Err(left.max(1) as u64)
            } else {
                Ok((p.window_started_at, p.request_count + 1))
            }
        }
        _ => Ok((now, 1)),
    }
}

/// Point the user's single `email_change_requests` row at `new_addr`
/// with a fresh token, inside the caller's transaction (which holds the
/// user row lock). The row is updated in place when it exists, so its
/// hourly window and count are never lost; otherwise it is inserted.
/// Returns the token for the link.
async fn write_email_change_request(
    app: &AppState,
    txn: &sea_orm::DatabaseTransaction,
    row: &user::Model,
    new_addr: &str,
    exists: bool,
    (window_started_at, request_count): (chrono::DateTime<chrono::FixedOffset>, i32),
    now: chrono::DateTime<chrono::FixedOffset>,
) -> Result<String, sea_orm::DbErr> {
    use sea_orm::sea_query::Expr;
    let token_id = uuid::Uuid::now_v7();
    let tok = email_token::issue_email_change(
        row.id,
        token_id,
        EMAIL_CHANGE_TTL,
        app.secrets.email_token_key.as_ref(),
    );
    let expires_at = now + chrono::Duration::seconds(EMAIL_CHANGE_TTL.as_secs() as i64);
    if exists {
        // A new token id voids the previous link.
        EmailChangeEntity::update_many()
            .col_expr(email_change_request::Column::TokenId, Expr::value(token_id))
            .col_expr(
                email_change_request::Column::NewEmail,
                Expr::value(new_addr.to_owned()),
            )
            .col_expr(
                email_change_request::Column::TokenHash,
                Expr::value(sha256_hex(&tok)),
            )
            .col_expr(
                email_change_request::Column::ExpiresAt,
                Expr::value(expires_at),
            )
            .col_expr(
                email_change_request::Column::ConsumedAt,
                Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
            )
            .col_expr(email_change_request::Column::CreatedAt, Expr::value(now))
            .col_expr(
                email_change_request::Column::TokenVersion,
                Expr::value(row.token_version),
            )
            .col_expr(
                email_change_request::Column::WindowStartedAt,
                Expr::value(window_started_at),
            )
            .col_expr(
                email_change_request::Column::RequestCount,
                Expr::value(request_count),
            )
            .filter(email_change_request::Column::UserId.eq(row.id))
            .exec(txn)
            .await?;
    } else {
        EmailChangeAM {
            token_id: Set(token_id),
            user_id: Set(row.id),
            new_email: Set(new_addr.to_owned()),
            token_hash: Set(sha256_hex(&tok)),
            expires_at: Set(expires_at),
            consumed_at: Set(None),
            created_at: Set(now),
            token_version: Set(row.token_version),
            window_started_at: Set(window_started_at),
            request_count: Set(request_count),
        }
        .insert(txn)
        .await?;
    }
    Ok(tok)
}

/// Mail the confirmation link to `new_addr` and a notice to the account's
/// current address. Runs after the request committed; send failures are
/// logged, not returned (a new PATCH sends a new link).
async fn send_email_change_mail(app: &AppState, row: &user::Model, new_addr: &str, tok: &str) {
    let public_url = app.cfg().public_url.clone();
    let confirm = templates::email_change_confirm(&public_url, new_addr, tok);
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
}

#[cfg(test)]
mod tests {
    use super::credentials_changed;

    #[test]
    fn credentials_changed_refuses_a_bumped_token_version_or_a_new_hash() {
        let before = (3, Some("hash-a"));
        assert!(!credentials_changed(before, (3, Some("hash-a"))));
        // Password reset, password change or sign-out-everywhere.
        assert!(credentials_changed(before, (4, Some("hash-a"))));
        // A hash written without a bump still counts.
        assert!(credentials_changed(before, (3, Some("hash-b"))));
        assert!(credentials_changed(before, (3, None)));
    }
}
