//! `DELETE /me` — self-service account deletion (spec §17.10, product
//! audit 2026-11 §5 item 21). The counterpart of `GET /me/export`: the web
//! dialog points at the export before it asks for confirmation.
//!
//! **Re-authentication.** A live session is not enough on its own — a
//! borrowed or stolen browser must not be able to erase an account in one
//! click:
//!
//! - Accounts with a local password (`password_editable` on `/auth/me`:
//!   `local:` external id + a stored hash) must send `password`. A wrong
//!   password is `403 auth.invalid` (not 401, which the web client reads
//!   as "session expired" and answers with a silent refresh-and-retry) and
//!   counts against both failed-auth lockout axes, so this endpoint is no
//!   better a password oracle than the sign-in form.
//! - Accounts that sign in through an identity provider have no password
//!   Folio can check. A true re-auth would be a fresh OIDC round trip
//!   (`prompt=login` / `max_age=0`) minting a short-lived re-auth token —
//!   a flow of its own. Until that exists they type their email (or, when
//!   the issuer gave none, their display name) as `confirm`; with the
//!   session cookie and the CSRF token that proves intent, not identity.
//! - App-password callers (OPDS / KOReader tokens) are refused outright:
//!   a reading-app credential must never be able to delete the account.
//!
//! **Last admin.** An instance must keep an active admin, so an admin whose
//! deletion would leave none gets `409 conflict`. The check runs inside the
//! deletion transaction with every admin row locked (`FOR UPDATE`, id
//! order), so two admins deleting themselves at once can't both pass it.
//!
//! **What goes.** One transaction:
//!
//! 1. [`UNCONSTRAINED_USER_DELETES`] — `user_id` columns with no FK to
//!    `users` (`progress_records`, `library_user_access`) are deleted
//!    explicitly.
//! 2. `DELETE FROM users`. Every other personal table references
//!    `users(id)` `ON DELETE CASCADE` (markers, reading sessions, saved
//!    views → collection entries, CBL lists → entries, ratings, notes,
//!    pages, pins, sidebar, rail dismissals, view groups, log widgets,
//!    page overrides, per-series reader prefs, app passwords, auth sessions
//!    — the refresh tokens — and password-reset uses). Attribution columns
//!    on shared rows (`metadata_run.triggered_by`, `app_setting.updated_by`,
//!    relationship `created_by` / `reviewed_by`, duplicate decisions) are
//!    `ON DELETE SET NULL`: the shared row stays, the pointer clears.
//! 3. The `account.self_delete` audit row, written through
//!    [`crate::audit::record_in`] on the same transaction.
//!
//! **What stays.** History keeps the id it recorded, as the hard-purge
//! sweep does for issues ([`crate::jobs::hard_purge`]): `audit_log` is
//! append-only (never UPDATE / DELETE) and `actor_id` has no FK, so the
//! user's audit rows survive with the actor detached — `/admin/audit`
//! shows the bare id with no label. [`KEPT_USER_REFERENCES`] lists every
//! such column; `tests/account_self_delete.rs` fails when a new user-id
//! column appears that is in neither list.
//!
//! The new audit row carries the SHA-256 of the lower-cased email, never
//! the email: an operator who knows an address can confirm that account
//! deleted itself without the deletion adding the address to the log.
//! Older rows are kept as written, and some name an address in their
//! payload (`user.account.update` after an email change,
//! `admin.user.create`) — append-only history is not rewritten here.
//!
//! **Afterwards.** The session, CSRF and refresh cookies are cleared in the
//! 204. Any copy of the old cookies is dead too: the access token resolves
//! to no user row (401) and the refresh token's session row cascaded away.
//! Background jobs an admin started keep running; the one job-side insert
//! that names the user under an FK (`metadata_run.triggered_by`) falls back
//! to `NULL` when the user is gone (`orchestrator::start_run`).

use axum::{
    Extension,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    Statement, TransactionTrait,
};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use entity::user::{self, UserRole, UserState};

use crate::api::extractors::Validated;
use crate::api::{respond, respond_with_field_errors};
use crate::audit::{self, AuditEntry};
use crate::auth::local::has_local_password;
use crate::auth::{CurrentUser, cookies, failed_auth, password};
use crate::middleware::{RequestContext, rate_limit};
use crate::state::AppState;
use server_macros::handler;
use shared::error::{ApiErrorCode, FieldError};

/// Audit action written on a successful deletion.
pub const AUDIT_ACTION: &str = "account.self_delete";

/// `(table, column)` pairs that hold a user id with **no** FK to `users`,
/// whose rows are the user's own and are deleted explicitly before the
/// `users` row. Everything else cascades.
pub const UNCONSTRAINED_USER_DELETES: &[(&str, &str)] = &[
    // Predates the FK convention (no FK to users or issues).
    ("progress_records", "user_id"),
    // Library ACL grants; no FK to users.
    ("library_user_access", "user_id"),
];

/// `(table, column)` pairs that hold a user id with no FK and are kept on
/// purpose: history and attribution that record *who did something*, not
/// data the user owns.
pub const KEPT_USER_REFERENCES: &[(&str, &str)] = &[
    // Append-only audit history; the actor is detached, never rewritten.
    ("audit_log", "actor_id"),
    // Who started a scan batch / a bulk metadata fetch (admin history).
    ("scan_batch", "actor_id"),
    ("metadata_batch", "created_by"),
    // Who accepted an issue's metadata review; the flag stays meaningful.
    ("issues", "metadata_review_accepted_by"),
];

pub fn routes() -> OpenApiRouter<AppState> {
    // Own sub-router so the rate-limit `route_layer` doesn't leak onto
    // sibling routes when this is merged into the `api` group.
    OpenApiRouter::new()
        .routes(routes!(delete_me))
        .route_layer(rate_limit::ACCOUNT_DELETE.build())
}

/// Body of `DELETE /me`. Exactly one field is required, depending on how
/// the account signs in (`password_editable` on `/auth/me`).
// No `Debug` derive: the body carries a plaintext password.
#[derive(Deserialize, ToSchema, garde::Validate)]
pub struct DeleteAccountReq {
    /// The account's current password. Required for accounts with a local
    /// password.
    #[serde(default)]
    #[garde(length(max = MAX_SECRET_LEN))]
    pub password: Option<String>,
    /// For accounts that sign in through an identity provider: the
    /// account's email, typed to confirm (case-insensitive), or its display
    /// name when the account has no email.
    #[serde(default)]
    #[garde(length(max = MAX_SECRET_LEN))]
    pub confirm: Option<String>,
}

/// Upper bound on either body field; keeps an oversized "password" from
/// reaching argon2.
const MAX_SECRET_LEN: usize = 1024;

/// How the caller proved intent; recorded on the audit row.
#[derive(Clone, Copy)]
enum Reauth {
    Password,
    Confirmation,
}

impl Reauth {
    fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Confirmation => "confirmation",
        }
    }
}

/// The account's email when it has one (non-blank).
fn usable_email(email: Option<&str>) -> Option<&str> {
    email.map(str::trim).filter(|e| !e.is_empty())
}

/// An account without a password confirms by typing its email
/// (case-insensitive) or, with no email, its display name (exact, trimmed).
fn confirmation_matches(email: Option<&str>, display_name: &str, typed: &str) -> bool {
    let typed = typed.trim();
    if typed.is_empty() {
        return false;
    }
    match usable_email(email) {
        Some(email) => typed.to_lowercase() == email.to_lowercase(),
        None => typed == display_name.trim(),
    }
}

fn field_error(field: &str, message: &str) -> Response {
    respond_with_field_errors(
        StatusCode::UNPROCESSABLE_ENTITY,
        ApiErrorCode::Validation,
        message,
        vec![FieldError {
            field: field.to_owned(),
            message: message.to_owned(),
        }],
    )
}

fn internal() -> Response {
    respond(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Internal,
        "internal error",
    )
}

#[utoipa::path(
    operation_id = "account_delete",
    delete,
    path = "/me",
    request_body = DeleteAccountReq,
    responses(
        (status = 204, description = "Account and personal data deleted; session, CSRF and refresh cookies cleared"),
        (status = 401, description = "not authenticated"),
        (status = 403, description = "password incorrect, or the caller used an app password"),
        (status = 409, description = "the caller is the instance's last active admin"),
        (status = 422, description = "password or confirmation missing / confirmation does not match"),
        (status = 429, description = "rate limited or locked out after failed attempts"),
    )
)]
#[handler]
pub async fn delete_me(
    State(app): State<AppState>,
    user: CurrentUser,
    jar: CookieJar,
    Extension(ctx): Extension<RequestContext>,
    Validated(req): Validated<DeleteAccountReq>,
) -> Response {
    if user.app_password_scope.is_some() {
        return respond(
            StatusCode::FORBIDDEN,
            ApiErrorCode::PermissionDenied,
            "deleting an account needs an interactive sign-in, not an app password",
        );
    }

    let row = match user::Entity::find_by_id(user.id).one(&app.db).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return respond(
                StatusCode::UNAUTHORIZED,
                ApiErrorCode::AuthInvalid,
                "account no longer exists",
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "account delete: user lookup failed");
            return internal();
        }
    };

    // Same predicate as `MeResp.password_editable`, so the dialog always asks
    // for what the server checks: a local account with a stored hash uses its
    // password; anything else (OIDC, including a local account later linked
    // to an issuer) confirms by typing.
    let reauth = if has_local_password(&row) {
        let stored = row.password_hash.as_deref().unwrap_or_default();
        let Some(given) = req.password.as_deref().filter(|p| !p.is_empty()) else {
            return field_error("password", "enter your password to delete your account");
        };
        // The email axis is keyed the way sign-in keys it (lower-cased).
        let email_key = usable_email(row.email.as_deref()).map(str::to_lowercase);
        // Same lockout gates as sign-in: refuse before any argon2 work.
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
            tracing::info!(reason = "bad_password", "account delete rejected");
            return respond(
                StatusCode::FORBIDDEN,
                ApiErrorCode::AuthInvalid,
                "password is incorrect",
            );
        }
        Reauth::Password
    } else {
        let typed = req.confirm.as_deref().unwrap_or_default();
        if !confirmation_matches(row.email.as_deref(), &row.display_name, typed) {
            let what = if usable_email(row.email.as_deref()).is_some() {
                "type your email address to confirm"
            } else {
                "type your display name to confirm"
            };
            return field_error("confirm", what);
        }
        Reauth::Confirmation
    };

    match delete_account(&app, &row, reauth, &ctx).await {
        Ok(Outcome::Deleted) => {}
        Ok(Outcome::LastAdmin) => {
            return respond(
                StatusCode::CONFLICT,
                ApiErrorCode::Conflict,
                "you are the only active admin; make another account an admin before deleting yours",
            );
        }
        Ok(Outcome::Gone) => {
            return respond(
                StatusCode::UNAUTHORIZED,
                ApiErrorCode::AuthInvalid,
                "account no longer exists",
            );
        }
        Err(e) => {
            tracing::error!(error = %e, user_id = %row.id, "account delete failed");
            return internal();
        }
    }

    tracing::info!(user_id = %row.id, reauth = reauth.as_str(), "account deleted by its owner");
    (StatusCode::NO_CONTENT, cookies::clear_auth(jar)).into_response()
}

enum Outcome {
    Deleted,
    LastAdmin,
    /// The row vanished between the extractor and the transaction (a
    /// double-submitted delete).
    Gone,
}

async fn delete_account(
    app: &AppState,
    row: &user::Model,
    reauth: Reauth,
    ctx: &RequestContext,
) -> Result<Outcome, sea_orm::DbErr> {
    let txn = app.db.begin().await?;

    // Lock every admin row in id order before deciding: a concurrent
    // self-delete by another admin waits here, then re-reads, so the two
    // can't each see the other as "the remaining admin".
    let admins = user::Entity::find()
        .filter(user::Column::Role.eq(UserRole::Admin))
        .order_by_asc(user::Column::Id)
        .lock_exclusive()
        .all(&txn)
        .await?;
    let is_admin = admins.iter().any(|a| a.id == row.id);
    if is_admin
        && !admins
            .iter()
            .any(|a| a.id != row.id && a.state == UserState::Active)
    {
        txn.rollback().await?;
        return Ok(Outcome::LastAdmin);
    }

    for (table, column) in UNCONSTRAINED_USER_DELETES {
        txn.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("DELETE FROM {table} WHERE {column} = $1"),
            [row.id.into()],
        ))
        .await?;
    }

    let deleted = user::Entity::delete_by_id(row.id).exec(&txn).await?;
    if deleted.rows_affected == 0 {
        txn.rollback().await?;
        return Ok(Outcome::Gone);
    }

    audit::record_in(
        &txn,
        AuditEntry {
            actor_id: row.id,
            action: AUDIT_ACTION,
            target_type: Some("user"),
            target_id: Some(row.id.to_string()),
            payload: serde_json::json!({
                "email_sha256": row
                    .email
                    .as_deref()
                    .map(|e| cookies::sha256_hex(&e.trim().to_lowercase())),
                "role": row.role,
                "reauth": reauth.as_str(),
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await?;

    txn.commit().await?;
    Ok(Outcome::Deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_uses_email_case_insensitively_else_display_name() {
        let email = Some("Reader@X.test");
        assert!(confirmation_matches(
            email,
            "Reader One",
            "  reader@x.TEST "
        ));
        assert!(!confirmation_matches(email, "Reader One", "Reader One"));
        assert!(!confirmation_matches(email, "Reader One", ""));

        for none in [None, Some("  ")] {
            assert!(confirmation_matches(none, "Reader One", "Reader One"));
            assert!(!confirmation_matches(none, "Reader One", "reader one"));
            assert!(!confirmation_matches(none, "Reader One", " "));
        }
    }
}
