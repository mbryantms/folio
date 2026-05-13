//! Auth extractor — pulls the current user from either the session cookie or a
//! Bearer access token, validates `users.token_version`, and produces a
//! [`CurrentUser`] for handler signatures.
//!
//! Admin-only handlers should take [`RequireAdmin`] instead of `CurrentUser` —
//! that extractor returns 403 if the resolved user's role isn't `"admin"`, so
//! the check is structural and impossible to forget.

use axum::{
    extract::{FromRef, FromRequestParts},
    http::{StatusCode, request::Parts},
    response::IntoResponse,
};
use axum_extra::extract::CookieJar;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::state::AppState;
use entity::user::{self, Entity as UserEntity};

use super::cookies::SESSION_COOKIE;
use super::jwt::JwtKeys;

#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub id: Uuid,
    pub role: String,
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
    Forbidden,
    Internal,
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> axum::response::Response {
        let (status, code, message) = match self {
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
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "auth.permission_denied",
                "Admin only",
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "Internal error",
            ),
        };
        let body = serde_json::json!({"error": {"code": code, "message": message}});
        (status, axum::Json(body)).into_response()
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
        // — JWTs always start with `eyJ` so there's no overlap. The
        // app-password DB lookup runs argon2 verify against active rows,
        // so this is slow on cold cache (10-30ms per active row) but the
        // working set is tiny in practice.
        if super::app_password::looks_like_app_password(&token) {
            return resolve_app_password(&app, &token).await;
        }

        let keys = JwtKeys::from_secret(&app.secrets.jwt_ed25519, &app.cfg.public_url)
            .map_err(|_| AuthRejection::Internal)?;
        let claims = keys
            .verify_access(&token)
            .map_err(|_| AuthRejection::Invalid)?;
        let user_id: Uuid = claims.sub.parse().map_err(|_| AuthRejection::Invalid)?;

        let row = UserEntity::find()
            .filter(user::Column::Id.eq(user_id))
            .one(&app.db)
            .await
            .map_err(|_| AuthRejection::Internal)?
            .ok_or(AuthRejection::Invalid)?;

        if row.state == "disabled" {
            return Err(AuthRejection::UserDisabled);
        }
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

/// Resolve an `app_…` Bearer token to its owning user. Bumps the row's
/// `last_used_at` on success (best-effort, see `app_password::verify`).
async fn resolve_app_password(app: &AppState, token: &str) -> Result<CurrentUser, AuthRejection> {
    let resolved = super::app_password::verify(&app.db, token, app.secrets.pepper.as_ref())
        .await
        .ok_or(AuthRejection::Invalid)?;
    let row = UserEntity::find()
        .filter(user::Column::Id.eq(resolved.user_id))
        .one(&app.db)
        .await
        .map_err(|_| AuthRejection::Internal)?
        .ok_or(AuthRejection::Invalid)?;
    if row.state == "disabled" {
        return Err(AuthRejection::UserDisabled);
    }
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
            return Err(AuthRejection::Forbidden);
        }
        Ok(Self(user))
    }
}

/// Admin-only wrapper around [`CurrentUser`]. Returns 403 with code
/// `auth.permission_denied` when the resolved user's role isn't `"admin"`.
/// Use this for every `/admin/*` handler so the role check is impossible to
/// forget on a new route.
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
        if user.role != "admin" {
            return Err(AuthRejection::Forbidden);
        }
        Ok(Self(user))
    }
}

fn extract_token(parts: &Parts) -> Option<String> {
    if let Some(v) = parts
        .headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        // Authorization: Bearer <jwt|app_…>
        if let Some(rest) = v.strip_prefix("Bearer ") {
            return Some(rest.trim().to_owned());
        }
        // Authorization: Basic <b64(user:password)> — OPDS-client default.
        // Only app-password tokens are accepted here; a raw JWT carried via
        // Basic would be a session-token-in-URL-style footgun (clients log
        // / surface the Authorization header in places they shouldn't).
        if let Some(rest) = v.strip_prefix("Basic ") {
            return extract_basic_app_password(rest.trim());
        }
    }
    // Cookie: __Host-comic_session=<jwt>
    let jar = CookieJar::from_headers(&parts.headers);
    jar.get(SESSION_COOKIE).map(|c| c.value().to_owned())
}

fn extract_basic_app_password(b64: &str) -> Option<String> {
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let s = std::str::from_utf8(&decoded).ok()?;
    let (_user, password) = s.split_once(':')?;
    if !super::app_password::looks_like_app_password(password) {
        return None;
    }
    Some(password.to_owned())
}
