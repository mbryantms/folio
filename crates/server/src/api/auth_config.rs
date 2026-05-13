//! `GET /admin/auth/config` and `GET /auth/config` — views of the auth
//! configuration; `POST /admin/auth/oidc/discover` — pre-save probe of an
//! OIDC issuer's discovery document.
//!
//! The admin GET variant returns the full surface (issuer URL, client_id,
//! trust flag) and is gated by `RequireAdmin`. The public variant
//! (`/auth/config`, audit M-7) is unauthenticated and returns only the
//! booleans the sign-in page needs to render — never any secrets.
//!
//! Editing is now wired via `PATCH /admin/settings` (runtime-config-admin
//! M3): the live `Config` is rebuilt + the OIDC discovery cache is
//! evicted on save. The discover endpoint below is a separate read-only
//! probe so an admin can preview an issuer's endpoints before committing.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::auth::RequireAdmin;
use crate::config::AuthMode;
use crate::middleware::rate_limit;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/auth/config", get(get_config))
        .route("/auth/config", get(get_public_config))
        .route(
            "/admin/auth/oidc/discover",
            post(probe_discovery).route_layer(rate_limit::OIDC_CALLBACK.build()),
        )
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
    let cfg = app.cfg();
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
    let cfg = app.cfg();
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

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct OidcDiscoverReq {
    /// The OIDC issuer URL the admin is about to save. Used as the base
    /// for `${issuer}/.well-known/openid-configuration`.
    pub issuer: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OidcDiscoverResp {
    pub issuer: String,
    pub authorization_endpoint: Option<String>,
    pub token_endpoint: Option<String>,
    pub jwks_uri: Option<String>,
    pub end_session_endpoint: Option<String>,
    pub userinfo_endpoint: Option<String>,
    /// Truncated to the most useful fields above. The full doc is large
    /// and not all of it is admin-interesting; if a future need arises
    /// the raw JSON can be added under a separate field.
    pub scopes_supported: Option<Vec<String>>,
}

#[utoipa::path(
    post,
    path = "/admin/auth/oidc/discover",
    request_body = OidcDiscoverReq,
    responses(
        (status = 200, body = OidcDiscoverResp),
        (status = 400, description = "invalid issuer URL"),
        (status = 403, description = "admin only"),
        (status = 502, description = "discovery doc unreachable / malformed"),
    )
)]
pub async fn probe_discovery(
    State(_app): State<AppState>,
    _admin: RequireAdmin,
    Json(req): Json<OidcDiscoverReq>,
) -> Response {
    let issuer = req.issuer.trim();
    if issuer.is_empty() || !(issuer.starts_with("http://") || issuer.starts_with("https://")) {
        return error(
            StatusCode::BAD_REQUEST,
            "oidc.invalid_issuer",
            "issuer must be an http(s) URL",
        );
    }
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "reqwest client build failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            return error(
                StatusCode::BAD_GATEWAY,
                "oidc.discovery_unreachable",
                &format!("could not fetch discovery doc: {e}"),
            );
        }
    };
    if !resp.status().is_success() {
        return error(
            StatusCode::BAD_GATEWAY,
            "oidc.discovery_status",
            &format!("discovery doc returned {}", resp.status()),
        );
    }
    let doc: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return error(
                StatusCode::BAD_GATEWAY,
                "oidc.discovery_malformed",
                &format!("discovery doc was not JSON: {e}"),
            );
        }
    };

    let pick_str = |k: &str| doc.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    let pick_vec = |k: &str| {
        doc.get(k).and_then(|v| v.as_array()).map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
    };

    Json(OidcDiscoverResp {
        issuer: pick_str("issuer").unwrap_or_else(|| req.issuer.clone()),
        authorization_endpoint: pick_str("authorization_endpoint"),
        token_endpoint: pick_str("token_endpoint"),
        jwks_uri: pick_str("jwks_uri"),
        end_session_endpoint: pick_str("end_session_endpoint"),
        userinfo_endpoint: pick_str("userinfo_endpoint"),
        scopes_supported: pick_vec("scopes_supported"),
    })
    .into_response()
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
