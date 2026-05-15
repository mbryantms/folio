//! OIDC code+PKCE flow (§17.1).
//!
//! Routes:
//!   GET  /auth/oidc/start    → 302 to issuer with PKCE; sets `__Host-comic_oidc` (state+verifier)
//!   GET  /auth/oidc/callback → exchange code, validate ID token, upsert user, set session cookies
//!
//! The OpenID Connect Core 1.0 flow is implemented via the `openidconnect` crate, which
//! handles discovery, JWKS following, PKCE, and ID-token validation (`aud`/`iss`/`exp`/`nbf`,
//! signature, `nonce`).
//!
//! `email_verified` defaults to `false` when missing (§12.7); `COMIC_OIDC_TRUST_UNVERIFIED_EMAIL=true`
//! opts in to the workaround.

use axum::{
    Extension, Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use axum_extra::extract::{CookieJar, cookie::Cookie};
use openidconnect::{
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
    core::{CoreClient, CoreProviderMetadata, CoreResponseType},
    reqwest::async_http_client,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{OnceCell, RwLock};
use uuid::Uuid;

use crate::config::AuthMode;
use crate::middleware::RequestContext;
use crate::middleware::rate_limit;
use crate::state::AppState;

use super::cookies::{
    self, csrf_cookie, new_refresh_token_raw, refresh_cookie, session_cookie, sha256_hex,
};
use super::jwt::JwtKeys;

use entity::auth_session::ActiveModel as SessionAM;
use entity::user::{self, ActiveModel as UserAM, Entity as UserEntity};

const OIDC_STATE_COOKIE: &str = "__Host-comic_oidc";
const OIDC_STATE_TTL_SECS: u64 = 5 * 60;
// Access + refresh TTLs come from config (`COMIC_JWT_ACCESS_TTL` / `_REFRESH_TTL`).
// Defaults: 24h / 30d. Validated at startup so unwrapping in handlers is safe.

/// Discovery-cache TTL. The OIDC provider's discovery document doesn't
/// change often — JWKS rotation is the only signal, and openidconnect's
/// internal cache handles that — but we still want to refresh occasionally
/// so an issuer's endpoint moves get picked up without a server restart.
const DISCOVERY_TTL: Duration = Duration::from_secs(5 * 60);

pub fn routes() -> Router<AppState> {
    Router::new().route("/auth/oidc/start", get(start)).route(
        "/auth/oidc/callback",
        get(callback).route_layer(rate_limit::OIDC_CALLBACK.build()),
    )
}

/// One entry in the discovery cache. Holds both the wired-up
/// [`CoreClient`] (consumed by start/callback) and the `end_session_endpoint`
/// URL (consumed by logout, optional — not every provider publishes it).
#[derive(Clone)]
pub(crate) struct DiscoveryEntry {
    pub client: CoreClient,
    pub end_session_endpoint: Option<String>,
    fetched_at: Instant,
}

/// Process-global discovery cache. Keyed by issuer URL so a future
/// multi-provider build can populate multiple entries; today we always
/// store under the single configured issuer.
pub(crate) static DISCOVERY_CACHE: OnceCell<Arc<RwLock<HashMap<String, DiscoveryEntry>>>> =
    OnceCell::const_new();

async fn discovery_cache() -> &'static Arc<RwLock<HashMap<String, DiscoveryEntry>>> {
    DISCOVERY_CACHE
        .get_or_init(|| async { Arc::new(RwLock::new(HashMap::new())) })
        .await
}

/// Clear the entire cache. Called by `PATCH /admin/settings` when an
/// `auth.oidc.*` row changes so the next OIDC handshake re-fetches the
/// discovery doc against the new issuer / client credentials. Also a
/// test hook for the wiremock harness.
pub(crate) async fn clear_discovery_cache() {
    if let Some(cell) = DISCOVERY_CACHE.get() {
        cell.write().await.clear();
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct StateCookie {
    csrf: String,
    pkce: String,
    nonce: String,
    redirect_after: Option<String>,
}

/// Resolve a cached [`DiscoveryEntry`] for the configured issuer. Refreshes
/// past [`DISCOVERY_TTL`]. The cache is process-global so concurrent
/// requests share a single live discovery handshake.
async fn discover_entry(app: &AppState) -> anyhow::Result<DiscoveryEntry> {
    let cfg = app.cfg();
    let issuer = cfg
        .oidc_issuer
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("oidc not configured"))?
        .to_owned();

    {
        let map = discovery_cache().await.read().await;
        if let Some(entry) = map.get(&issuer)
            && entry.fetched_at.elapsed() < DISCOVERY_TTL
        {
            return Ok(entry.clone());
        }
    }

    let client_id = cfg
        .oidc_client_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("oidc client_id not set"))?;
    let client_secret = cfg
        .oidc_client_secret
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("oidc client_secret not set"))?;

    tracing::info!(target: "auth.oidc", issuer = %issuer, "oidc discovery refresh");
    let provider =
        CoreProviderMetadata::discover_async(IssuerUrl::new(issuer.clone())?, async_http_client)
            .await?;

    let redirect = format!(
        "{}/auth/oidc/callback",
        cfg.public_url.trim_end_matches('/')
    );
    let client = CoreClient::from_provider_metadata(
        provider,
        ClientId::new(client_id.to_string()),
        Some(ClientSecret::new(client_secret.to_string())),
    )
    .set_redirect_uri(RedirectUrl::new(redirect)?);

    // `end_session_endpoint` is part of the OIDC RP-Initiated Logout
    // extension, not Core 1.0. openidconnect's CoreProviderMetadata
    // doesn't surface it, so we re-fetch the discovery document by hand
    // and pluck the field. Cheap (cached for DISCOVERY_TTL) and we
    // already paid for the request budget once during the typed
    // discovery call above.
    let end_session_endpoint = fetch_end_session_endpoint(&issuer).await;

    let entry = DiscoveryEntry {
        client,
        end_session_endpoint,
        fetched_at: Instant::now(),
    };
    discovery_cache()
        .await
        .write()
        .await
        .insert(issuer, entry.clone());
    Ok(entry)
}

async fn discover(app: &AppState) -> anyhow::Result<CoreClient> {
    Ok(discover_entry(app).await?.client)
}

/// Public wrapper around [`discover_entry`] so the logout handler in
/// `auth::local` can read `end_session_endpoint` without re-implementing
/// the cache or duplicating the discovery fetch.
pub(crate) async fn discover_entry_pub(app: &AppState) -> anyhow::Result<DiscoveryEntry> {
    discover_entry(app).await
}

/// Side-fetch the discovery doc to extract `end_session_endpoint`.
/// Returns `None` if the issuer doesn't publish one (Dex, for example,
/// only does in newer versions) or if the request fails — RP-initiated
/// logout silently degrades to local-only revoke.
async fn fetch_end_session_endpoint(issuer: &str) -> Option<String> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let resp = match reqwest::get(&url).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, %url, "discovery doc fetch failed");
            return None;
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), %url, "discovery doc non-2xx");
        return None;
    }
    let doc: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "discovery doc parse failed");
            return None;
        }
    };
    doc.get("end_session_endpoint")
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned())
}

#[derive(Debug, Deserialize)]
pub struct StartQuery {
    /// Where to send the user after a successful login. Validated to start with `/`.
    #[serde(default)]
    redirect_after: Option<String>,
}

pub async fn start(
    State(app): State<AppState>,
    jar: CookieJar,
    Query(q): Query<StartQuery>,
) -> Response {
    if !matches!(app.cfg().auth_mode, AuthMode::Oidc | AuthMode::Both) {
        return error(StatusCode::NOT_FOUND, "not_found", "oidc disabled");
    }
    let client = match discover(&app).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "oidc discovery failed");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "oidc discovery failed",
            );
        }
    };
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let nonce = Nonce::new_random();
    let csrf = CsrfToken::new_random();

    let csrf_for_url = csrf.clone();
    let nonce_for_url = nonce.clone();
    let (auth_url, _csrf_returned, _nonce_returned) = client
        .authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            move || csrf_for_url.clone(),
            move || nonce_for_url.clone(),
        )
        .add_scope(Scope::new("openid".into()))
        .add_scope(Scope::new("email".into()))
        .add_scope(Scope::new("profile".into()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    // Validate redirect_after to prevent open-redirect. The earlier check
    // (`starts_with('/') && !starts_with("//")`) missed three browser
    // quirks: `/\example.com` is treated as a protocol-relative URL by
    // Edge/Chrome on some legacy code paths; embedded control characters
    // can survive URL parsing in ways the auth handler doesn't see; and
    // a full absolute URL with leading slash like `/https://evil.tld`
    // could still parse as absolute downstream. Tighten:
    // - single leading `/`
    // - no `//`, `\\`, or `://` anywhere
    // - no control characters
    let redirect_after = q
        .redirect_after
        .as_ref()
        .filter(|s| is_safe_redirect_target(s))
        .cloned();

    let state = StateCookie {
        csrf: csrf.secret().to_string(),
        pkce: pkce_verifier.secret().to_string(),
        nonce: nonce.secret().to_string(),
        redirect_after,
    };
    let state_json = match serde_json::to_string(&state) {
        Ok(j) => j,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "state encode",
            );
        }
    };

    let mut state_cookie = Cookie::new(OIDC_STATE_COOKIE, state_json);
    state_cookie.set_http_only(true);
    state_cookie.set_secure(true);
    state_cookie.set_same_site(axum_extra::extract::cookie::SameSite::Lax);
    state_cookie.set_path("/auth/oidc/callback");
    state_cookie.set_max_age(time::Duration::seconds(OIDC_STATE_TTL_SECS as i64));

    let jar = jar.add(state_cookie);
    (jar, Redirect::to(auth_url.as_str())).into_response()
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

pub async fn callback(
    State(app): State<AppState>,
    jar: CookieJar,
    Extension(ctx): Extension<RequestContext>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    if !matches!(app.cfg().auth_mode, AuthMode::Oidc | AuthMode::Both) {
        return error(StatusCode::NOT_FOUND, "not_found", "oidc disabled");
    }

    // Brute-force lockout check (§17.7). Shared with local::login.
    if let Some(ip) = ctx.client_ip
        && let Ok(Some(retry)) = super::failed_auth::check_lockout_for(&app, ip).await
    {
        return super::failed_auth::lockout_response(retry);
    }

    if let Some(err) = q.error {
        tracing::warn!(
            error = %err,
            description = %q.error_description.unwrap_or_default(),
            "oidc issuer returned error"
        );
        super::failed_auth::record_failure_for(&app, &ctx).await;
        return error(StatusCode::UNAUTHORIZED, "auth.oidc_error", &err);
    }

    let (Some(code), Some(state_param)) = (q.code, q.state) else {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "missing code or state",
        );
    };

    // Recover the PKCE verifier and expected CSRF from the cookie.
    let state_cookie = match jar.get(OIDC_STATE_COOKIE) {
        Some(c) => c.value().to_owned(),
        None => {
            return error(
                StatusCode::BAD_REQUEST,
                "auth.invalid",
                "missing oidc state cookie",
            );
        }
    };
    let state: StateCookie = match serde_json::from_str(&state_cookie) {
        Ok(s) => s,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "auth.invalid",
                "corrupt oidc state cookie",
            );
        }
    };
    if !constant_time_eq::constant_time_eq(state.csrf.as_bytes(), state_param.as_bytes()) {
        return error(
            StatusCode::BAD_REQUEST,
            "auth.invalid",
            "oidc state mismatch",
        );
    }

    let client = match discover(&app).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "oidc discovery failed during callback");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "oidc discovery",
            );
        }
    };

    let token_resp = match client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(PkceCodeVerifier::new(state.pkce))
        .request_async(async_http_client)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "oidc token exchange failed");
            super::failed_auth::record_failure_for(&app, &ctx).await;
            return error(
                StatusCode::UNAUTHORIZED,
                "auth.invalid",
                "token exchange failed",
            );
        }
    };

    let id_token = match token_resp.id_token() {
        Some(t) => t,
        None => {
            super::failed_auth::record_failure_for(&app, &ctx).await;
            return error(
                StatusCode::BAD_REQUEST,
                "auth.invalid",
                "no id_token in response",
            );
        }
    };

    let id_verifier = client.id_token_verifier();
    let nonce_check = Nonce::new(state.nonce.clone());
    let claims = match id_token.claims(&id_verifier, &nonce_check) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "id_token verification failed");
            super::failed_auth::record_failure_for(&app, &ctx).await;
            return error(StatusCode::UNAUTHORIZED, "auth.invalid", "id_token invalid");
        }
    };

    let issuer = claims.issuer().to_string();
    let subject = claims.subject().to_string();
    let external_id = format!("oidc:{}|{}", issuer, subject);

    // email_verified policy (§12.7).
    let email_present = claims.email().is_some();
    let claim_email_verified = claims.email_verified();
    let email_verified = match (claim_email_verified, app.cfg().oidc_trust_unverified_email) {
        (Some(true), _) => true,
        (Some(false), _) => false,
        (None, true) => {
            tracing::warn!("OIDC missing email_verified claim; trusting per env opt-in");
            true
        }
        (None, false) => false,
    };
    // Hash the subject before logging — `sub` is a stable PII-bearing
    // identifier at most IdPs (it correlates to a real account). The
    // unhashed value is still persisted in `users.external_id` for the
    // DB-level lookup; logs only need correlation across login attempts.
    tracing::info!(
        target: "auth.oidc",
        subject_hash = %sha256_hex(&subject)[..12].to_owned(),
        issuer = %issuer,
        claim_email_verified_present = ?claim_email_verified.is_some(),
        claim_email_verified = ?claim_email_verified,
        "oidc login"
    );

    if !email_verified && email_present {
        return error(
            StatusCode::FORBIDDEN,
            "auth.email_unverified",
            "issuer reports email not verified",
        );
    }

    let email = claims
        .email()
        .map(|e| e.as_str().to_string().to_lowercase());
    let display_name = claims
        .preferred_username()
        .map(|u| u.as_str().to_string())
        .or_else(|| {
            claims
                .name()
                .and_then(|n| n.get(None))
                .map(|n| n.to_string())
        })
        .or_else(|| email.clone())
        .unwrap_or_else(|| subject.clone());

    // Upsert user (lookup by external_id).
    let user_id = Uuid::now_v7();
    let now = chrono::Utc::now().fixed_offset();
    let existing = UserEntity::find()
        .filter(user::Column::ExternalId.eq(external_id.clone()))
        .one(&app.db)
        .await
        .ok()
        .flatten();

    let user_row = if let Some(row) = existing {
        // Sync fields that may have changed at the issuer.
        let mut am: UserAM = row.clone().into();
        am.email = Set(email.clone());
        am.email_verified = Set(email_verified);
        am.display_name = Set(display_name.clone());
        am.last_login_at = Set(Some(now));
        am.updated_at = Set(now);
        match am.update(&app.db).await {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "user upsert failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    } else {
        // Collision check (audit M-4 / S-5): if a local user already owns
        // this email address, refuse to silently shadow them with a fresh
        // OIDC row. Auto-linking would let an attacker who controls an
        // OIDC account at the same email take over the local account; the
        // documented runbook is admin-driven manual merge instead.
        if let Some(ref e) = email
            && let Ok(Some(local)) = UserEntity::find()
                .filter(user::Column::Email.eq(e.clone()))
                .one(&app.db)
                .await
            && local.external_id.starts_with("local:")
        {
            tracing::warn!(
                target: "auth.oidc",
                local_user_id = %local.id,
                email_hash = %sha256_hex(e)[..12].to_owned(),
                "oidc/local email collision — refusing auto-link"
            );
            let _ = crate::audit::record(
                &app.db,
                crate::audit::AuditEntry {
                    actor_id: local.id,
                    action: "auth.oidc.collision",
                    target_type: Some("user"),
                    target_id: Some(local.id.to_string()),
                    payload: serde_json::json!({
                        "issuer": issuer,
                        "subject_hash": sha256_hex(&subject)[..12].to_owned(),
                    }),
                    ip: ctx.ip_string(),
                    user_agent: ctx.user_agent.clone(),
                },
            )
            .await;
            return error(
                StatusCode::CONFLICT,
                "auth.email_in_use",
                "an account with that email already exists; ask your admin to link or migrate it",
            );
        }
        // First-user admin bootstrap (§12.8).
        let user_count = UserEntity::find().count(&app.db).await.unwrap_or(1);
        let role = if user_count == 0 {
            tracing::warn!("first_admin_bootstrap: granting admin role to first user (oidc)");
            "admin"
        } else {
            "user"
        };
        let am = UserAM {
            id: Set(user_id),
            external_id: Set(external_id),
            display_name: Set(display_name),
            email: Set(email),
            email_verified: Set(email_verified),
            password_hash: Set(None),
            totp_secret: Set(None),
            state: Set("active".into()),
            role: Set(role.into()),
            token_version: Set(0),
            created_at: Set(now),
            updated_at: Set(now),
            last_login_at: Set(Some(now)),
            default_reading_direction: Set(None),
            default_fit_mode: Set(None),
            default_view_mode: Set(None),
            default_page_strip: Set(false),
            default_cover_solo: Set(true),
            theme: Set(None),
            accent_color: Set(None),
            density: Set(None),
            keybinds: Set(serde_json::json!({})),
            activity_tracking_enabled: Set(true),
            timezone: Set("UTC".into()),
            reading_min_active_ms: Set(30_000),
            reading_min_pages: Set(3),
            reading_idle_ms: Set(180_000),
            language: Set("en".into()),
            exclude_from_aggregates: Set(false),
            show_marker_count: Set(false),
        };
        match am.insert(&app.db).await {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "oidc user insert failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    };

    if user_row.state == "disabled" {
        return error(StatusCode::FORBIDDEN, "auth.disabled", "account disabled");
    }

    // Issue session cookies (mirrors auth/local.rs).
    let access_ttl = app.cfg().access_ttl();
    let refresh_ttl = app.cfg().refresh_ttl();
    let session_id = Uuid::now_v7();
    let raw_rt = new_refresh_token_raw();
    let hash = sha256_hex(&raw_rt);
    let session_now = chrono::Utc::now();
    let expires = session_now + chrono::Duration::seconds(refresh_ttl.as_secs() as i64);

    // Persist the raw id_token for RP-initiated logout. openidconnect's
    // `IdToken` implements Display, which emits the compact JWS form.
    let id_token_hint = Some(id_token.to_string());
    let am = SessionAM {
        id: Set(session_id),
        user_id: Set(user_row.id),
        refresh_token_hash: Set(hash),
        created_at: Set(session_now.fixed_offset()),
        last_used_at: Set(session_now.fixed_offset()),
        expires_at: Set(expires.fixed_offset()),
        user_agent: Set(ctx.user_agent.clone()),
        ip: Set(ctx.ip_string()),
        revoked_at: Set(None),
        id_token_hint: Set(id_token_hint),
    };
    if let Err(e) = am.insert(&app.db).await {
        tracing::error!(error = %e, "auth_session insert failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    let keys = match JwtKeys::from_secret(&app.secrets.jwt_ed25519, &app.cfg().public_url) {
        Ok(k) => k,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let access = match keys.issue_access(
        user_row.id,
        &user_row.role,
        user_row.token_version,
        chrono::Duration::seconds(access_ttl.as_secs() as i64),
    ) {
        Ok(t) => t,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };

    let csrf_token = cookies::new_csrf_token();
    let jar = jar
        .remove(cookies::clear(OIDC_STATE_COOKIE, "/auth/oidc/callback"))
        .add(session_cookie(access, access_ttl))
        .add(refresh_cookie(raw_rt, refresh_ttl))
        .add(csrf_cookie(csrf_token, access_ttl));

    // Redirect to the requested location, defaulting to /.
    let target = state.redirect_after.unwrap_or_else(|| "/".to_string());
    (jar, Redirect::to(&target)).into_response()
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

/// True if `s` is a safe path-only redirect target. Mirrors the checks in
/// `start` so tests can hit it directly.
pub fn is_safe_redirect_target(s: &str) -> bool {
    if !s.starts_with('/') {
        return false;
    }
    if s.starts_with("//") || s.starts_with("/\\") {
        return false;
    }
    if s.contains("://") || s.contains('\\') {
        return false;
    }
    if s.chars().any(char::is_control) {
        return false;
    }
    // Belt-and-suspenders: reject any colon in the *path* portion (before
    // `?` or `#`). Chrome historically parsed `/javascript:foo` loosely
    // and navigating to such a target is at best surprising. Query
    // strings may legitimately carry colons (`?ts=10:00:00`), so we only
    // scan the path segment.
    let without_query = s.split_once('?').map(|(p, _)| p).unwrap_or(s);
    let path_part = without_query
        .split_once('#')
        .map(|(p, _)| p)
        .unwrap_or(without_query);
    if path_part.contains(':') {
        return false;
    }
    true
}

#[cfg(test)]
mod redirect_tests {
    use super::is_safe_redirect_target;

    #[test]
    fn accepts_simple_paths() {
        assert!(is_safe_redirect_target("/"));
        assert!(is_safe_redirect_target("/library"));
        assert!(is_safe_redirect_target("/series/x-men/issues/1"));
        assert!(is_safe_redirect_target("/views/abc?sort=name"));
    }

    #[test]
    fn rejects_absolute_urls() {
        assert!(!is_safe_redirect_target("https://evil.tld/"));
        assert!(!is_safe_redirect_target("http://evil.tld/"));
        assert!(!is_safe_redirect_target("//evil.tld/path"));
    }

    #[test]
    fn rejects_backslash_smuggling() {
        // Various Edge/Chrome quirks where `\\` or `/\` parses as a scheme.
        assert!(!is_safe_redirect_target("/\\evil.tld"));
        assert!(!is_safe_redirect_target("/path\\back"));
        assert!(!is_safe_redirect_target("\\\\evil.tld"));
    }

    #[test]
    fn rejects_embedded_scheme_in_path() {
        // `://` anywhere in the value points at an embedded URL.
        assert!(!is_safe_redirect_target("/redirect=https://evil.tld"));
        // The colon-in-path check catches `javascript:` smuggling even
        // without `://` because the path segment carries the colon.
        assert!(!is_safe_redirect_target("/javascript:alert(1)"));
    }

    #[test]
    fn allows_colons_in_query_string() {
        // Legitimate uses of `:` in URL queries (timestamps, ratios) must
        // still be accepted — the colon-rejection is path-only.
        assert!(is_safe_redirect_target(
            "/series?since=2024-01-01T10:00:00Z"
        ));
        assert!(is_safe_redirect_target("/page?ratio=16:9"));
    }

    #[test]
    fn rejects_control_chars() {
        assert!(!is_safe_redirect_target("/path\n"));
        assert!(!is_safe_redirect_target("/path\r\nLocation: evil"));
        assert!(!is_safe_redirect_target("/path\t"));
    }

    #[test]
    fn rejects_non_root_anchor() {
        assert!(!is_safe_redirect_target("relative/path"));
        assert!(!is_safe_redirect_target(""));
        assert!(!is_safe_redirect_target("javascript:alert(1)"));
    }
}
