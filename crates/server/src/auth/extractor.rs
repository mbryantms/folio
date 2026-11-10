//! Auth extractor — pulls the current user from either the session cookie or a
//! Bearer access token, validates `users.token_version`, and produces a
//! [`CurrentUser`] for handler signatures.
//!
//! Admin-only handlers should take [`RequireAdmin`] instead of `CurrentUser` —
//! that extractor returns 403 if the resolved user's role isn't `UserRole::Admin`, so
//! the check is structural and impossible to forget.
//!
//! **Account-state rule (one rule, every credential).** A request is admitted
//! only for a user whose `state` is `active` ([`admit_state`]): `disabled` is
//! 403 `auth.disabled`, `pending_verification` is 403 `auth.email_unverified`.
//! It applies to the session cookie, a Bearer JWT, and an app-password
//! (Bearer or OPDS Basic) alike, so `RequireAdmin` / `RequireProgressScope`
//! inherit it. A pending user is never meant to hold a session — local login
//! refuses them and registration issues none — so nothing a pending user needs
//! sits behind `CurrentUser`: verify-email, resend-verification, the password
//! reset pair and logout all authenticate by token or cookie on their own.

use axum::{
    extract::{FromRef, FromRequestParts},
    http::{StatusCode, request::Parts},
    response::IntoResponse,
};
use axum_extra::extract::CookieJar;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::state::AppState;
use entity::user::{self, Entity as UserEntity, UserRole, UserState};

use super::cookies::SESSION_COOKIE;

/// Request-extension marker for the reader-client surface: page bytes,
/// thumbnails and stored covers, OPDS 1.2 + 2.0 (feeds, downloads,
/// progress, KOReader sync), OPDS progression and the Komga shim.
/// `app::build_openapi_router_with` layers it onto exactly those routers
/// before they are merged into the `bare` group. [`CurrentUser`] resolves
/// an app password (Bearer or Basic) only when the marker is present;
/// everywhere else the token is refused with 403 before any DB or argon2
/// work, so a reader token can't reach the JSON API, admin routes or
/// account settings.
#[derive(Clone, Copy, Debug)]
pub struct AppPasswordSurface;

/// Route-template prefixes of every route under the reader-client surface
/// ([`AppPasswordSurface`]). The CSRF middleware runs before the marker is
/// attached, so it decides "is this a reader route?" from the matched route
/// template with [`is_reader_route`]: an app password skips the CSRF check
/// only there. `tests/app_passwords.rs::reader_route_prefixes_match_reader_surface`
/// checks every route of every `reader_surface(...)` router in `app.rs`
/// starts with one of these, and that no other route does.
pub const READER_ROUTE_PREFIXES: &[&str] = &[
    // OPDS 1.2 + 2.0 feeds, downloads, PSE, progress, KOReader sync,
    // OPDS progression.
    "/opds/",
    // Komga-compatible shim.
    "/api/v1/",
    // Page bytes + page thumbnails.
    "/issues/{id}/pages/",
    // Stored issue covers.
    "/issues/{id}/covers/",
];

/// True when `matched_path` (an axum route template such as
/// `/opds/v1/issues/{id}/progress`) belongs to the reader-client surface.
pub fn is_reader_route(matched_path: &str) -> bool {
    READER_ROUTE_PREFIXES
        .iter()
        .any(|p| matched_path.starts_with(p))
}

#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub id: Uuid,
    pub role: UserRole,
    pub display_name: String,
    pub email: Option<String>,
    /// `Some(scope)` only when this user resolved through an
    /// app-password Bearer/Basic token. Cookie + JWT users authenticated
    /// interactively, so they have implicit "all capabilities" and
    /// `RequireScope` lets them through without checking. `None` ≡
    /// "no scope restriction applies".
    pub app_password_scope: Option<String>,
}

#[derive(Debug)]
pub enum AuthRejection {
    Missing,
    Invalid,
    TokenVersionMismatch,
    UserDisabled,
    EmailUnverified,
    Forbidden,
    /// An app password presented outside the [`AppPasswordSurface`].
    AppPasswordNotAllowed,
    /// The client IP is in failed-auth lockout (app-password path; the
    /// remaining seconds go out as `Retry-After`).
    LockedOut(u64),
    /// No app-password verification permit freed up in time.
    Busy,
    Internal,
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> axum::response::Response {
        let (status, code, message) = match self {
            // Same answer sign-in gives a locked-out IP.
            Self::LockedOut(retry) => return super::failed_auth::lockout_response(retry),
            Self::Busy => {
                let mut resp = crate::api::error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "auth.busy",
                    "Too many sign-ins are being checked right now; try again shortly",
                );
                resp.headers_mut().insert(
                    axum::http::header::RETRY_AFTER,
                    axum::http::HeaderValue::from(super::app_password::BUSY_RETRY_AFTER_SECS),
                );
                return resp;
            }
            Self::Missing => (
                StatusCode::UNAUTHORIZED,
                "auth.required",
                "Authentication required",
            ),
            Self::Invalid => (StatusCode::UNAUTHORIZED, "auth.invalid", "Invalid token"),
            Self::TokenVersionMismatch => (
                StatusCode::UNAUTHORIZED,
                "auth.revoked",
                "Token revoked; please sign in again",
            ),
            Self::UserDisabled => (StatusCode::FORBIDDEN, "auth.disabled", "Account disabled"),
            Self::EmailUnverified => (
                StatusCode::FORBIDDEN,
                "auth.email_unverified",
                "Verify your email first",
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "auth.permission_denied",
                "Admin only",
            ),
            Self::AppPasswordNotAllowed => (
                StatusCode::FORBIDDEN,
                "auth.permission_denied",
                "App passwords only work with reader apps (OPDS). Sign in to use this endpoint.",
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "Internal error",
            ),
        };
        crate::api::error(status, code, message)
    }
}

impl<S> FromRequestParts<S> for CurrentUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app = AppState::from_ref(state);

        let token = extract_token(parts).ok_or(AuthRejection::Missing)?;

        // App-password Bearer flow (M7). The `app_` prefix is unambiguous
        // — JWTs always start with `eyJ` so there's no overlap. The token
        // is found by its lookup digest and argon2-checked against that one
        // row (`app_password::verify`). Reader routes only: anywhere else
        // the token is refused before the DB / argon2 work.
        if super::app_password::looks_like_app_password(&token) {
            if parts.extensions.get::<AppPasswordSurface>().is_none() {
                return Err(AuthRejection::AppPasswordNotAllowed);
            }
            let ctx = parts
                .extensions
                .get::<crate::middleware::RequestContext>()
                .cloned();
            return resolve_app_password(&app, ctx.as_ref(), &token).await;
        }

        let claims = app
            .jwt_keys
            .verify_access(&token)
            .map_err(|_| AuthRejection::Invalid)?;
        let user_id: Uuid = claims.sub.parse().map_err(|_| AuthRejection::Invalid)?;

        let row = UserEntity::find()
            .filter(user::Column::Id.eq(user_id))
            .one(&app.db)
            .await
            .map_err(|_| AuthRejection::Internal)?
            .ok_or(AuthRejection::Invalid)?;

        admit_state(row.state)?;
        if row.token_version != claims.tv {
            return Err(AuthRejection::TokenVersionMismatch);
        }

        Ok(CurrentUser {
            id: row.id,
            role: row.role,
            display_name: row.display_name,
            email: row.email,
            app_password_scope: None,
        })
    }
}

/// The account-state rule every credential path applies (module docs): only
/// `active` is admitted. Exhaustive on purpose, so a new [`UserState`] variant
/// has to decide here whether it may authenticate.
pub fn admit_state(state: UserState) -> Result<(), AuthRejection> {
    match state {
        UserState::Active => Ok(()),
        UserState::Disabled => Err(AuthRejection::UserDisabled),
        UserState::PendingVerification => Err(AuthRejection::EmailUnverified),
    }
}

/// Resolve an `app_…` Bearer token to its owning user. Bumps the row's
/// `last_used_at` on success (best-effort, see `app_password::verify`).
/// A wrong token is 401 `auth.invalid` as before; a locked-out client IP
/// gets sign-in's 429 `auth.locked_out`, and a request that found every
/// verification permit taken gets 503 `auth.busy`.
async fn resolve_app_password(
    app: &AppState,
    ctx: Option<&crate::middleware::RequestContext>,
    token: &str,
) -> Result<CurrentUser, AuthRejection> {
    use super::app_password::VerifyError;
    let resolved = super::app_password::verify(app, ctx, token)
        .await
        .map_err(|e| match e {
            VerifyError::Invalid => AuthRejection::Invalid,
            VerifyError::LockedOut(retry) => AuthRejection::LockedOut(retry),
            VerifyError::Busy => AuthRejection::Busy,
        })?;
    let row = UserEntity::find()
        .filter(user::Column::Id.eq(resolved.user_id))
        .one(&app.db)
        .await
        .map_err(|_| AuthRejection::Internal)?
        .ok_or(AuthRejection::Invalid)?;
    admit_state(row.state)?;
    Ok(CurrentUser {
        id: row.id,
        role: row.role,
        display_name: row.display_name,
        email: row.email,
        app_password_scope: Some(resolved.scope),
    })
}

/// Wraps `CurrentUser` and asserts that, **if** the caller authenticated
/// via an app-password Bearer/Basic token, that token's `scope` is at
/// least `read+progress`. Cookie + JWT callers always pass — they
/// authenticated interactively and have implicit full capability.
///
/// Used on `PUT /opds/v1/issues/{id}/progress` and the KOReader sync
/// shim so a `read`-scope token can browse + download but can't
/// silently write progress back into the user's account.
#[derive(Clone, Debug)]
pub struct RequireProgressScope(pub CurrentUser);

impl std::ops::Deref for RequireProgressScope {
    type Target = CurrentUser;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> FromRequestParts<S> for RequireProgressScope
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = CurrentUser::from_request_parts(parts, state).await?;
        if let Some(scope) = user.app_password_scope.as_deref()
            && scope != super::app_password::SCOPE_READ_PROGRESS
        {
            // v0.3.42 — surface scope-mismatch rejections. Previously a
            // silent 403; the Komga-compat PATCH retry-loop in Panels
            // produces a stream of these on a misconfigured (read-only)
            // app-password and an operator at INFO level sees nothing.
            tracing::info!(
                method = %parts.method,
                path = %parts.uri.path(),
                actual_scope = %scope,
                required_scope = %super::app_password::SCOPE_READ_PROGRESS,
                "auth: progress-scope check failed — app-password lacks `read+progress` scope (re-issue via /me/app-passwords)",
            );
            return Err(AuthRejection::Forbidden);
        }
        Ok(Self(user))
    }
}

/// Admin-only wrapper around [`CurrentUser`]. Returns 403 with code
/// `auth.permission_denied` when the resolved user's role isn't `UserRole::Admin`.
/// Use this for every `/admin/*` handler so the role check is impossible to
/// forget on a new route. The account must also be `active`: that comes from
/// [`CurrentUser`]'s state rule, so a `pending_verification` or `disabled`
/// admin is refused before the role is looked at.
#[derive(Clone, Debug)]
pub struct RequireAdmin(pub CurrentUser);

impl std::ops::Deref for RequireAdmin {
    type Target = CurrentUser;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> FromRequestParts<S> for RequireAdmin
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = CurrentUser::from_request_parts(parts, state).await?;
        if user.role != UserRole::Admin {
            return Err(AuthRejection::Forbidden);
        }
        Ok(Self(user))
    }
}

/// The user id named by a JWT access token on `headers`, chosen by the same
/// [`credential`] rule as [`CurrentUser`]. The signature, issuer and audience
/// are checked; expiry is not, and neither is the `users` row
/// (`token_version`, account state), because this runs before the handler
/// without a DB round-trip. So it answers "whose token is this", never "may
/// this request proceed": the per-user rate-limit key
/// ([`crate::middleware::rate_limit::UserKey`]) uses it, and the handler's
/// extractor still authenticates (an expired token then gets its 401 and the
/// client refreshes). App-password and Basic credentials give `None` (their
/// check is an argon2 verify, too slow for a key extractor).
pub fn access_token_subject(
    headers: &axum::http::HeaderMap,
    keys: &super::jwt::JwtKeys,
) -> Option<Uuid> {
    let token = match credential(headers)? {
        Credential::Bearer(t) => t.to_owned(),
        Credential::Cookie(t) => t,
        Credential::Basic(_) => return None,
    };
    if super::app_password::looks_like_app_password(&token) {
        return None;
    }
    keys.access_subject_ignoring_expiry(&token)?.parse().ok()
}

/// Where a request's credential comes from. One rule for every reader
/// ([`extract_token`] and [`access_token_subject`]), so the rate-limit key
/// always names the same token the handler authenticates.
enum Credential<'a> {
    /// `Authorization: Bearer <jwt|app_…>` (trimmed).
    Bearer(&'a str),
    /// `Authorization: Basic <b64>` (trimmed, still encoded).
    Basic(&'a str),
    /// `Cookie: __Host-comic_session=<jwt>`.
    Cookie(String),
}

fn credential(headers: &axum::http::HeaderMap) -> Option<Credential<'_>> {
    if let Some(v) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(rest) = v.strip_prefix("Bearer ") {
            return Some(Credential::Bearer(rest.trim()));
        }
        if let Some(rest) = v.strip_prefix("Basic ") {
            return Some(Credential::Basic(rest.trim()));
        }
    }
    let jar = CookieJar::from_headers(headers);
    jar.get(SESSION_COOKIE)
        .map(|c| Credential::Cookie(c.value().to_owned()))
}

fn extract_token(parts: &Parts) -> Option<String> {
    match credential(&parts.headers)? {
        Credential::Bearer(t) => Some(t.to_owned()),
        // Basic is the OPDS-client default. Only app-password tokens are
        // accepted here; a raw JWT carried via Basic would be a
        // session-token-in-URL-style footgun (clients log / surface the
        // Authorization header in places they shouldn't).
        Credential::Basic(b64) => extract_basic_app_password(b64, parts),
        Credential::Cookie(t) => Some(t),
    }
}

fn extract_basic_app_password(b64: &str, parts: &Parts) -> Option<String> {
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let s = std::str::from_utf8(&decoded).ok()?;
    let (_user, password) = s.split_once(':')?;
    if !super::app_password::looks_like_app_password(password) {
        // M7 (v0.3.41): the most likely silent misconfiguration for an
        // operator setting up an OPDS client (Panels, Tachiyomi-class,
        // KOReader) is pasting their account password into the client's
        // password field instead of issuing an app-password via
        // /me/app-passwords. The extractor returns None here so the
        // request fails with 401; this info log gives /admin/logs the
        // signal needed to diagnose without bumping global log level
        // to debug.
        tracing::info!(
            method = %parts.method,
            path = %parts.uri.path(),
            "auth: Basic credential rejected — password is not an app-password (issue one via /me/app-passwords)",
        );
        return None;
    }
    Some(password.to_owned())
}
