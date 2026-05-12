//! `GET /admin/auth/config` and `GET /auth/config` — read-only views of the
//! auth configuration.
//!
//! The admin variant returns the full surface (issuer URL, client_id, trust
//! flag) and is gated by `RequireAdmin`. The public variant
//! (`/auth/config`, audit M-7) is unauthenticated and returns only the
//! booleans the sign-in page needs to render — never any secrets.
//!
//! M6e + auth-hardening M6. The server reads its auth knobs from env at
//! boot. Editing them at runtime requires a restart, so this endpoint is
//! intentionally read-only — the UI surfaces a banner pointing to env
//! vars + restart, and admins make the change in their compose file or
//! systemd unit.

use axum::{
    Json, Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;

use crate::auth::RequireAdmin;
use crate::config::AuthMode;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/auth/config", get(get_config))
        .route("/auth/config", get(get_public_config))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AuthConfigView {
    /// `'oidc' | 'local' | 'both'`
    pub auth_mode: String,
    pub oidc: OidcConfigView,
    pub local: LocalConfigView,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OidcConfigView {
    pub configured: bool,
    pub issuer: Option<String>,
    pub client_id: Option<String>,
    /// True when `COMIC_OIDC_TRUST_UNVERIFIED_EMAIL` is set. Surfaced because
    /// it materially weakens email-claim trust.
    pub trust_unverified_email: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LocalConfigView {
    /// Local mode is enabled (auth_mode = 'local' or 'both').
    pub enabled: bool,
    /// Self-serve registration is open. False locks the system to the
    /// existing user set + the first-user admin bootstrap.
    pub registration_open: bool,
    /// SMTP wired (host non-empty). Without it, email verification + reset
    /// flows fall back to bypass-on-register.
    pub smtp_configured: bool,
}

#[utoipa::path(
    get,
    path = "/admin/auth/config",
    responses(
        (status = 200, body = AuthConfigView),
        (status = 403, description = "admin only"),
    )
)]
pub async fn get_config(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let cfg = &app.cfg;
    let oidc_configured = match cfg.auth_mode {
        AuthMode::Oidc | AuthMode::Both => cfg
            .oidc_issuer
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        AuthMode::Local => false,
    };
    let local_enabled = matches!(cfg.auth_mode, AuthMode::Local | AuthMode::Both);
    let smtp_configured = cfg
        .smtp_host
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);

    let view = AuthConfigView {
        auth_mode: cfg.auth_mode.to_string(),
        oidc: OidcConfigView {
            configured: oidc_configured,
            issuer: cfg.oidc_issuer.clone(),
            // Never return the client_id when in local-only mode.
            client_id: if oidc_configured {
                cfg.oidc_client_id.clone()
            } else {
                None
            },
            trust_unverified_email: cfg.oidc_trust_unverified_email,
        },
        local: LocalConfigView {
            enabled: local_enabled,
            registration_open: cfg.local_registration_open,
            smtp_configured,
        },
    };
    Json(view).into_response()
}

/// Minimum surface for the unauthenticated `/sign-in` page. Mirrors a
/// subset of [`AuthConfigView`] but returns only the booleans the UI
/// needs to render the right CTAs — issuer/client_id are intentionally
/// withheld.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PublicAuthConfigView {
    /// `'oidc' | 'local' | 'both'` — drives the form/SSO toggle.
    pub auth_mode: String,
    /// True when the OIDC provider is wired AND `auth_mode` permits it.
    /// When false, the sign-in page hides the SSO button.
    pub oidc_enabled: bool,
    /// True when local registration is open. False locks the system to
    /// the existing user set + first-user admin bootstrap.
    pub registration_open: bool,
}

#[utoipa::path(
    get,
    path = "/auth/config",
    responses(
        (status = 200, body = PublicAuthConfigView),
    )
)]
pub async fn get_public_config(State(app): State<AppState>) -> Response {
    let cfg = &app.cfg;
    let oidc_enabled = matches!(cfg.auth_mode, AuthMode::Oidc | AuthMode::Both)
        && cfg
            .oidc_issuer
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        && cfg
            .oidc_client_id
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
    let local_enabled = matches!(cfg.auth_mode, AuthMode::Local | AuthMode::Both);
    Json(PublicAuthConfigView {
        auth_mode: cfg.auth_mode.to_string(),
        oidc_enabled,
        // `registration_open` only makes sense when local mode is on.
        registration_open: local_enabled && cfg.local_registration_open,
    })
    .into_response()
}
