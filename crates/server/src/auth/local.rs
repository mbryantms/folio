//! Local-mode auth handlers (§17.1).
//!
//! Routes mounted under `/auth/`:
//!
//!   POST  /auth/local/register
//!   POST  /auth/local/login
//!   POST  /auth/refresh         (cross-mode; works for OIDC users too)
//!   POST  /auth/logout          (cross-mode)
//!   GET   /auth/me              (cross-mode)
//!
//! Local self-serve recovery (verify-email, request-password-reset,
//! reset-password, resend-verification) is wired and uses the SMTP
//! sender configured under /admin/email.

use axum::{
    Extension, Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::{get, patch, post},
};
use axum_extra::extract::CookieJar;
use chrono::Duration as ChronoDuration;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;
use uuid::Uuid;

use crate::api::form_or_json::{FormOrJson, ResponseFormat, redirect_with_error};
use crate::config::AuthMode;
use crate::email::templates;
use crate::middleware::RequestContext;
use crate::middleware::rate_limit;
use crate::state::AppState;

use super::CurrentUser;
use super::cookies::{
    self, CSRF_COOKIE, REFRESH_COOKIE, REFRESH_PATH, SESSION_COOKIE, csrf_cookie, new_csrf_token,
    new_refresh_token_raw, refresh_cookie, session_cookie,
};
use super::email_token::{self, TokenPurpose};
use super::jwt::JwtKeys;
use super::password;

use entity::auth_session::{self, ActiveModel as SessionAM, Entity as SessionEntity};
use entity::user::{self, ActiveModel as UserAM, Entity as UserEntity};

// Access + refresh TTLs are operator-tunable via `COMIC_JWT_ACCESS_TTL` and
// `COMIC_JWT_REFRESH_TTL` (parsed at startup; see `Config::access_ttl` /
// `Config::refresh_ttl`). Defaults are 24h / 30d — long enough that a
// content-consumption session never bounces a user mid-issue.

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/auth/local/register",
            post(register).route_layer(rate_limit::REGISTER.build()),
        )
        .route(
            "/auth/local/login",
            post(login).route_layer(rate_limit::LOGIN.build()),
        )
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/me/preferences", patch(update_preferences))
        // Recovery flow (M4). All four use the failed-auth Redis sentinel
        // through their per-route rate-limit bucket; bodies are intentionally
        // generic (e.g. always 204 / never confirm email-existence) so the
        // endpoints don't double as user-enumeration oracles.
        .route(
            "/auth/local/verify-email",
            get(verify_email).route_layer(rate_limit::RESEND_VERIFICATION.build()),
        )
        .route(
            "/auth/local/resend-verification",
            post(resend_verification).route_layer(rate_limit::RESEND_VERIFICATION.build()),
        )
        .route(
            "/auth/local/request-password-reset",
            post(request_password_reset).route_layer(rate_limit::PASSWORD_RESET_REQUEST.build()),
        )
        .route(
            "/auth/local/reset-password",
            post(reset_password).route_layer(rate_limit::PASSWORD_RESET_REDEEM.build()),
        )
}

// ────────────── DTOs ──────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RegisterReq {
    pub email: String,
    pub password: String,
    pub display_name: Option<String>,
    /// Progressive-enhancement redirect target for the no-JS form path.
    /// Validated through `is_safe_redirect_target` before any 303. Ignored
    /// by the JSON path (the client routes via `useRouter().push`).
    #[serde(default)]
    pub next: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LoginReq {
    pub email: String,
    pub password: String,
    /// Progressive-enhancement redirect target for the no-JS form path.
    /// Validated through `is_safe_redirect_target` before any 303. Ignored
    /// by the JSON path.
    #[serde(default)]
    pub next: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MeResp {
    pub id: String,
    pub email: Option<String>,
    pub display_name: String,
    pub role: String,
    pub csrf_token: String,
    /// Phase 3: per-user reader direction preference. `null` means "auto"
    /// (the reader still falls back to ComicInfo `Manga=YesAndRightToLeft`
    /// detection per series).
    #[serde(default)]
    pub default_reading_direction: Option<String>,
    /// M4: reader default fit mode — `'width' | 'height' | 'original'`.
    #[serde(default)]
    pub default_fit_mode: Option<String>,
    /// M4: reader default view mode — `'single' | 'double' | 'webtoon'`.
    #[serde(default)]
    pub default_view_mode: Option<String>,
    /// M4: when true the reader opens with the page strip visible.
    #[serde(default)]
    pub default_page_strip: bool,
    /// Default for the reader's "cover stands alone in double-page view"
    /// toggle. Per-series localStorage overrides at runtime.
    pub default_cover_solo: bool,
    /// M4: theme token — `'system' | 'dark' | 'light' | 'amber'`.
    #[serde(default)]
    pub theme: Option<String>,
    /// M4: accent palette token (e.g. `'amber'`, `'blue'`, `'emerald'`, `'rose'`).
    #[serde(default)]
    pub accent_color: Option<String>,
    /// M4: density token — `'comfortable' | 'compact'`.
    #[serde(default)]
    pub density: Option<String>,
    /// M4: per-action keybind overrides for the reader. Empty object means
    /// "use defaults".
    #[serde(default)]
    pub keybinds: serde_json::Value,
    /// M6a: per-user opt-out for reading-activity capture.
    pub activity_tracking_enabled: bool,
    /// M6a: IANA timezone string for daily-bucket aggregations.
    pub timezone: String,
    /// M6a: minimum accumulated active ms before a session is recorded.
    pub reading_min_active_ms: i32,
    /// M6a: minimum distinct pages before a session is recorded.
    pub reading_min_pages: i32,
    /// M6a: idle threshold (ms) after which the client ends the session.
    pub reading_idle_ms: i32,
    /// Human-URLs M3: BCP-47 language tag used for the `NEXT_LOCALE` cookie
    /// and next-intl message bundle selection.
    pub language: String,
    /// Stats v2: opt-out from server-wide aggregates. When true, admin
    /// dashboards exclude this user's sessions from totals/top-series. Does
    /// not affect personal `/me/reading-stats`.
    pub exclude_from_aggregates: bool,
    /// Markers M8: when true, /settings renders a count badge on the
    /// Bookmarks sidebar row. Default false.
    pub show_marker_count: bool,
}

/// `PATCH /me/preferences` request body. Every field is optional; when a key
/// is absent the prior value is preserved. To clear a stored value, send
/// `null` (where the type allows).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PreferencesReq {
    /// `'ltr' | 'rtl' | null`. `null` clears the preference.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub default_reading_direction: Option<Option<String>>,
    /// `'width' | 'height' | 'original' | null`.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub default_fit_mode: Option<Option<String>>,
    /// `'single' | 'double' | 'webtoon' | null`.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub default_view_mode: Option<Option<String>>,
    pub default_page_strip: Option<bool>,
    /// Default cover-solo toggle; absent leaves the prior value untouched.
    pub default_cover_solo: Option<bool>,
    /// `'system' | 'dark' | 'light' | 'amber' | null`.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub theme: Option<Option<String>>,
    /// `'amber' | 'blue' | 'emerald' | 'rose' | null`.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub accent_color: Option<Option<String>>,
    /// `'comfortable' | 'compact' | null`.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub density: Option<Option<String>>,
    /// Replace the entire keybinds object. Send `{}` to clear all overrides.
    pub keybinds: Option<serde_json::Value>,
    /// M6a: opt-out toggle. `false` disables the client tracker hook.
    pub activity_tracking_enabled: Option<bool>,
    /// M6a: IANA timezone string. Server validates it's parseable; an unknown
    /// zone is rejected so the heatmap can't silently fall back to UTC.
    pub timezone: Option<String>,
    /// M6a: minimum active ms (1000..=600_000).
    pub reading_min_active_ms: Option<i32>,
    /// M6a: minimum distinct pages (1..=200).
    pub reading_min_pages: Option<i32>,
    /// M6a: idle threshold ms (30_000..=1_800_000).
    pub reading_idle_ms: Option<i32>,
    /// Human-URLs M3: BCP-47 language tag. Validated against the supported
    /// locale list — sending an unknown value 400s rather than silently
    /// falling back.
    pub language: Option<String>,
    /// Stats v2: privacy toggle. When true, admin server-wide aggregates
    /// exclude this user's sessions.
    pub exclude_from_aggregates: Option<bool>,
    /// Markers M8: per-user toggle for the Bookmarks sidebar count
    /// badge. Default false.
    pub show_marker_count: Option<bool>,
}

/// `serde` helper: distinguish "absent" (None) from "explicit null"
/// (Some(None)) so PATCH semantics work — present-but-null clears the field,
/// absent leaves it untouched.
fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LoginResp {
    pub user: MeResp,
}

// ───── Recovery DTOs (M4) ─────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RequestPasswordResetReq {
    pub email: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ResetPasswordReq {
    pub token: String,
    pub new_password: String,
    /// Optional second password field. The web form ships two `<input>`s
    /// for confirmation; submitting them as `new_password` + `confirm_password`
    /// lets the no-JS fallback validate parity server-side. Ignored when
    /// absent or equal.
    #[serde(default)]
    pub confirm_password: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ResendVerificationReq {
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct VerifyEmailQuery {
    pub token: String,
}

/// Token TTLs match the spec / `docs/architecture/rate-limits.md`:
/// - verify-email: 24h (less time-sensitive; users may not check email same-day)
/// - reset-password: 1h (sensitive credential operation; short window)
const VERIFY_EMAIL_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const PASSWORD_RESET_TTL: Duration = Duration::from_secs(60 * 60);

// ────────────── Handlers ──────────────

#[utoipa::path(
    post,
    path = "/auth/local/register",
    request_body = RegisterReq,
    responses(
        (status = 201, body = LoginResp, description = "registration succeeded; session cookies set"),
        (status = 400, description = "validation error"),
        (status = 403, description = "registration closed"),
        (status = 409, description = "email already in use")
    )
)]
pub async fn register(
    State(app): State<AppState>,
    jar: CookieJar,
    Extension(ctx): Extension<RequestContext>,
    FormOrJson { data: req, format }: FormOrJson<RegisterReq>,
) -> impl IntoResponse {
    let safe_next = sanitize_next(req.next.as_deref());
    let fail = |status: StatusCode, code: &str, msg: &str| -> axum::response::Response {
        auth_failure_response(format, "/sign-in", safe_next.as_deref(), status, code, msg)
    };

    if !matches!(app.cfg().auth_mode, AuthMode::Local | AuthMode::Both) {
        return fail(StatusCode::NOT_FOUND, "not_found", "local auth disabled");
    }
    if !app.cfg().local_registration_open {
        return fail(
            StatusCode::FORBIDDEN,
            "auth.registration_closed",
            "Registration is closed",
        );
    }
    let email_lower = req.email.trim().to_lowercase();
    if !email_lower.contains('@') || email_lower.len() > 254 {
        return fail(StatusCode::BAD_REQUEST, "validation", "invalid email");
    }
    if req.password.len() < 12 {
        return fail(
            StatusCode::BAD_REQUEST,
            "validation",
            "password must be at least 12 characters",
        );
    }

    // Conflict check first; then hash; then INSERT — yes there's a TOCTOU race, but
    // the unique index on lower(email) is the real defense.
    if let Ok(Some(_)) = UserEntity::find()
        .filter(user::Column::Email.eq(email_lower.clone()))
        .one(&app.db)
        .await
    {
        return fail(StatusCode::CONFLICT, "conflict", "email already in use");
    }

    let hash = match password::hash(&req.password, app.secrets.pepper.as_ref()) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(error = %e, "argon2 hash failed");
            return fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    // First-user admin bootstrap (§12.8): if no users exist yet, this user becomes admin
    // and skips email verification.
    let user_count = UserEntity::find().count(&app.db).await.unwrap_or(1);
    let smtp_configured = app
        .cfg()
        .smtp_host
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty());
    let (role, state) = if user_count == 0 {
        tracing::warn!("first_admin_bootstrap: granting admin role to first user");
        ("admin", "active")
    } else if smtp_configured {
        ("user", "pending_verification")
    } else {
        // No SMTP configured → no way to verify. Treat as active.
        ("user", "active")
    };

    let user_id = Uuid::now_v7();
    let now = chrono::Utc::now().fixed_offset();
    let display = req
        .display_name
        .clone()
        .unwrap_or_else(|| email_lower.split('@').next().unwrap_or("user").to_string());

    let am = UserAM {
        id: Set(user_id),
        external_id: Set(format!("local:{}", user_id)),
        display_name: Set(display.clone()),
        email: Set(Some(email_lower.clone())),
        email_verified: Set(state == "active"),
        password_hash: Set(Some(hash)),
        totp_secret: Set(None),
        state: Set(state.into()),
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

    let inserted = match am.insert(&app.db).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "user insert failed");
            return fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    if state == "pending_verification" {
        // Mint a 24h verification token and send the welcome+verify email.
        // Token issuance / send failure is logged but doesn't fail the
        // register — the user can request a resend.
        if let Some(email_addr) = inserted.email.as_deref() {
            let tok = email_token::issue(
                TokenPurpose::EmailVerification,
                inserted.id,
                VERIFY_EMAIL_TTL,
                app.secrets.email_token_key.as_ref(),
            );
            let msg = templates::verify_email(&app.cfg().public_url, email_addr, &tok);
            if let Err(e) = app.send_email(msg).await {
                tracing::warn!(error = %e, user_id = %inserted.id, "verify-email send failed at register");
            }
        }
        return match format {
            ResponseFormat::Json => (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({
                    "status": "pending_verification",
                    "message": "Check your email for a verification link"
                })),
            )
                .into_response(),
            // No-JS fallback: bounce back to /sign-in with the `pending`
            // banner. The email is not preserved in the URL — the
            // verification flow only needs the user to click the link in
            // the email they just received.
            ResponseFormat::Form => Redirect::to("/sign-in?pending=1").into_response(),
        };
    }

    // Active path: issue session & cookies.
    issue_session(
        &app,
        &inserted,
        jar,
        &ctx,
        StatusCode::CREATED,
        format,
        safe_next.as_deref(),
    )
    .await
}

#[utoipa::path(
    post,
    path = "/auth/local/login",
    request_body = LoginReq,
    responses(
        (status = 200, body = LoginResp, description = "login succeeded; cookies set"),
        (status = 401, description = "invalid credentials")
    )
)]
pub async fn login(
    State(app): State<AppState>,
    jar: CookieJar,
    Extension(ctx): Extension<RequestContext>,
    FormOrJson { data: req, format }: FormOrJson<LoginReq>,
) -> impl IntoResponse {
    let safe_next = sanitize_next(req.next.as_deref());
    let fail = |status: StatusCode, code: &str, msg: &str| -> axum::response::Response {
        auth_failure_response(format, "/sign-in", safe_next.as_deref(), status, code, msg)
    };

    if !matches!(app.cfg().auth_mode, AuthMode::Local | AuthMode::Both) {
        return fail(StatusCode::NOT_FOUND, "not_found", "local auth disabled");
    }

    // Brute-force lockout check (§17.7 auth.failed bucket). Refuse the
    // attempt outright if this IP has crossed the threshold in the last
    // 15 minutes — even credential-validation is skipped, so a continued
    // burst doesn't keep the password-hash CPU pegged.
    if let Some(ip) = ctx.client_ip
        && let Ok(Some(retry)) = super::failed_auth::check_lockout_for(&app, ip).await
    {
        return match format {
            ResponseFormat::Json => super::failed_auth::lockout_response(retry),
            ResponseFormat::Form => Redirect::to(&redirect_with_error(
                "/sign-in",
                "auth.locked",
                "too many attempts; try again later",
                safe_next.as_deref(),
            ))
            .into_response(),
        };
    }

    let email_lower = req.email.trim().to_lowercase();

    let user_row = UserEntity::find()
        .filter(user::Column::Email.eq(email_lower.clone()))
        .one(&app.db)
        .await
        .ok()
        .flatten();

    let Some(row) = user_row else {
        // Constant-time login: run the full argon2id verify against a
        // pre-computed dummy PHC string so the missing-user path takes
        // the same wall time as the wrong-password path. The literal we
        // used here pre-M3 failed PHC parse before any argon2 work
        // (audit S-4), which let a timing channel distinguish the two.
        let dummy = password::dummy_hash(app.secrets.pepper.as_ref());
        let _ = password::verify(dummy, &req.password, app.secrets.pepper.as_ref());
        super::failed_auth::record_failure_for(&app, &ctx).await;
        // INFO-level reason so operators tailing logs can see WHY a
        // login failed without having to deduce it from the response
        // body. Today's prod incident (2026-05-16) took an extra hour
        // because every reject path returned an opaque "invalid
        // credentials". Email is intentionally NOT logged — the
        // request body usually carries it in a higher-level access
        // log, and duplicating it here would just bloat the entry.
        tracing::info!(reason = "user_not_found", "login rejected");
        return fail(
            StatusCode::UNAUTHORIZED,
            "auth.invalid",
            "invalid credentials",
        );
    };

    if row.state == "disabled" {
        super::failed_auth::record_failure_for(&app, &ctx).await;
        tracing::info!(reason = "account_disabled", user_id = %row.id, "login rejected");
        return fail(StatusCode::FORBIDDEN, "auth.disabled", "account disabled");
    }
    if row.state == "pending_verification" {
        super::failed_auth::record_failure_for(&app, &ctx).await;
        tracing::info!(reason = "email_unverified", user_id = %row.id, "login rejected");
        return fail(
            StatusCode::FORBIDDEN,
            "auth.email_unverified",
            "verify your email first",
        );
    }
    let Some(stored) = row.password_hash.as_ref() else {
        super::failed_auth::record_failure_for(&app, &ctx).await;
        // Account has no local password set — usually an OIDC-only
        // user trying the local form. Distinct from `wrong_password`
        // so the operator can spot config drift (e.g. OIDC user with
        // hash=NULL trying to sign in via the form).
        tracing::info!(reason = "no_local_password", user_id = %row.id, "login rejected");
        return fail(
            StatusCode::UNAUTHORIZED,
            "auth.invalid",
            "invalid credentials",
        );
    };
    let ok = password::verify(stored, &req.password, app.secrets.pepper.as_ref()).unwrap_or(false);
    if !ok {
        super::failed_auth::record_failure_for(&app, &ctx).await;
        tracing::info!(reason = "wrong_password", user_id = %row.id, "login rejected");
        return fail(
            StatusCode::UNAUTHORIZED,
            "auth.invalid",
            "invalid credentials",
        );
    }

    // Update last_login_at (best-effort).
    let _ = UserAM {
        id: Set(row.id),
        last_login_at: Set(Some(chrono::Utc::now().fixed_offset())),
        ..Default::default()
    }
    .update(&app.db)
    .await;

    issue_session(
        &app,
        &row,
        jar,
        &ctx,
        StatusCode::OK,
        format,
        safe_next.as_deref(),
    )
    .await
}

#[utoipa::path(
    post,
    path = "/auth/refresh",
    responses(
        (status = 200, body = MeResp, description = "tokens rotated"),
        (status = 401, description = "refresh token invalid or revoked")
    )
)]
pub async fn refresh(
    State(app): State<AppState>,
    jar: CookieJar,
    Extension(ctx): Extension<RequestContext>,
) -> impl IntoResponse {
    let Some(rt_cookie) = jar.get(REFRESH_COOKIE).map(|c| c.value().to_owned()) else {
        return error(
            StatusCode::UNAUTHORIZED,
            "auth.required",
            "no refresh cookie",
        );
    };

    // The cookie value is a raw base64url-encoded 32-byte token (§17.2).
    // Look up by hash; rotate atomically.
    let presented_hash = sha256_hex(&rt_cookie);
    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };

    let sess = match SessionEntity::find()
        .filter(auth_session::Column::RefreshTokenHash.eq(presented_hash))
        .one(&txn)
        .await
    {
        Ok(Some(s)) => s,
        _ => {
            // No session matching this hash. Could be a replay of an already-rotated token —
            // we don't know the user, so we can't revoke siblings. Just reject.
            return error(
                StatusCode::UNAUTHORIZED,
                "auth.invalid",
                "refresh replay or expired",
            );
        }
    };

    if sess.revoked_at.is_some() {
        return error(StatusCode::UNAUTHORIZED, "auth.invalid", "session revoked");
    }
    if sess.expires_at < chrono::Utc::now().fixed_offset() {
        return error(StatusCode::UNAUTHORIZED, "auth.invalid", "session expired");
    }

    // Rotate.
    let new_rt_raw = new_refresh_token_raw();
    let new_hash = sha256_hex(&new_rt_raw);
    let now = chrono::Utc::now().fixed_offset();
    let refresh_ttl = app.cfg().refresh_ttl();
    let new_expires = chrono::Utc::now() + chrono::Duration::seconds(refresh_ttl.as_secs() as i64);

    let session_id = sess.id;
    let mut active: SessionAM = sess.clone().into();
    active.refresh_token_hash = Set(new_hash);
    active.last_used_at = Set(now);
    active.expires_at = Set(new_expires.fixed_offset());
    // Track most recent client IP/UA so /me/sessions reflects the latest
    // device that rotated through this session.
    if ctx.ip_string().is_some() {
        active.ip = Set(ctx.ip_string());
    }
    if ctx.user_agent.is_some() {
        active.user_agent = Set(ctx.user_agent.clone());
    }
    if active.update(&txn).await.is_err() {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    let user_row = match UserEntity::find()
        .filter(user::Column::Id.eq(sess.user_id))
        .one(&txn)
        .await
    {
        Ok(Some(u)) => u,
        _ => return error(StatusCode::UNAUTHORIZED, "auth.invalid", "user gone"),
    };

    if txn.commit().await.is_err() {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    // Refresh always replies JSON — the JS-side `useRefresh` driver is the
    // only caller, no form path exists.
    finalize_session(
        &app,
        &user_row,
        session_id,
        new_rt_raw,
        jar,
        StatusCode::OK,
        ResponseFormat::Json,
        None,
    )
}

#[utoipa::path(
    post,
    path = "/auth/logout",
    responses(
        (status = 204, description = "session revoked, cookies cleared"),
        (status = 302, description = "RP-initiated logout — caller is redirected to the OIDC issuer's end_session_endpoint"),
    )
)]
pub async fn logout(State(app): State<AppState>, jar: CookieJar) -> axum::response::Response {
    // Look up the session up-front so we can read `id_token_hint` before
    // revoking it. revoke_after has the same effect either way — the
    // refresh cookie is being cleared.
    let mut id_token_hint: Option<String> = None;
    let mut user_external_id: Option<String> = None;
    if let Some(rt) = jar.get(REFRESH_COOKIE).map(|c| c.value().to_owned()) {
        let presented_hash = sha256_hex(&rt);
        if let Ok(Some(sess)) = SessionEntity::find()
            .filter(auth_session::Column::RefreshTokenHash.eq(presented_hash.clone()))
            .one(&app.db)
            .await
        {
            id_token_hint = sess.id_token_hint.clone();
            // Need the user's external_id to know whether RP-logout applies
            // and which issuer to ask. Cheap lookup; same row we just hit.
            if let Ok(Some(u)) = UserEntity::find_by_id(sess.user_id).one(&app.db).await {
                user_external_id = Some(u.external_id);
            }
        }
        let _ = SessionEntity::update_many()
            .col_expr(
                auth_session::Column::RevokedAt,
                sea_orm::sea_query::Expr::value(chrono::Utc::now().fixed_offset()),
            )
            .filter(auth_session::Column::RefreshTokenHash.eq(presented_hash))
            .exec(&app.db)
            .await;
    }
    let cleared = jar
        .remove(cookies::clear(SESSION_COOKIE, "/"))
        .remove(cookies::clear(CSRF_COOKIE, "/"))
        .remove(cookies::clear(REFRESH_COOKIE, REFRESH_PATH));

    // RP-initiated logout (OIDC sessions only). If the session row was
    // born from an OIDC login AND the issuer publishes an
    // `end_session_endpoint`, redirect through it so the IdP also clears
    // its session and any other RPs federated through this account get
    // a single-sign-out chance. Falls back silently to 204 when we lack
    // a hint, when the discovery doc has no `end_session_endpoint`, or
    // when discovery itself errors.
    if let Some(hint) = id_token_hint
        && let Some(ext) = user_external_id
        && ext.starts_with("oidc:")
        && let Ok(entry) = super::oidc::discover_entry_pub(&app).await
        && let Some(end_session) = entry.end_session_endpoint
    {
        let post_logout = format!("{}/sign-in", app.cfg().public_url.trim_end_matches('/'));
        let mut url = match url::Url::parse(&end_session) {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!(error = %e, %end_session, "end_session_endpoint parse failed");
                return (StatusCode::NO_CONTENT, cleared).into_response();
            }
        };
        url.query_pairs_mut()
            .append_pair("id_token_hint", &hint)
            .append_pair("post_logout_redirect_uri", &post_logout);
        return (cleared, axum::response::Redirect::to(url.as_str())).into_response();
    }

    (StatusCode::NO_CONTENT, cleared).into_response()
}

#[utoipa::path(
    get,
    path = "/auth/me",
    responses(
        (status = 200, body = MeResp),
        (status = 401, description = "not authenticated")
    )
)]
pub async fn me(
    State(app): State<AppState>,
    user: CurrentUser,
    jar: CookieJar,
) -> impl IntoResponse {
    // Mint a fresh CSRF token on /auth/me — clients call this on app load.
    let csrf = new_csrf_token();

    // Re-fetch the row to pick up profile fields not carried on CurrentUser
    // (e.g. default_reading_direction, M4 reader prefs). Tolerate a transient
    // DB error by returning the prior shape with prefs at their defaults.
    let row = UserEntity::find_by_id(user.id)
        .one(&app.db)
        .await
        .ok()
        .flatten();

    let body = me_resp_from_parts(&user, csrf.clone(), row.as_ref());
    let jar = jar.add(csrf_cookie(csrf, app.cfg().access_ttl()));
    (StatusCode::OK, jar, Json(body)).into_response()
}

/// `PATCH /me/preferences` — update the calling user's preferences.
/// Phase 3 only ships `default_reading_direction`. CSRF-checked by middleware
/// (cookie auth, unsafe verb).
#[utoipa::path(
    patch,
    path = "/me/preferences",
    request_body = PreferencesReq,
    responses(
        (status = 200, body = MeResp),
        (status = 400, description = "validation error"),
        (status = 401, description = "not authenticated")
    )
)]
pub async fn update_preferences(
    State(app): State<AppState>,
    user: CurrentUser,
    jar: CookieJar,
    Json(req): Json<PreferencesReq>,
) -> impl IntoResponse {
    if let Some(Some(d)) = req.default_reading_direction.as_ref()
        && !matches!(d.as_str(), "ltr" | "rtl")
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "default_reading_direction must be 'ltr', 'rtl', or null",
        );
    }
    if let Some(Some(d)) = req.default_fit_mode.as_ref()
        && !matches!(d.as_str(), "width" | "height" | "original")
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "default_fit_mode must be 'width', 'height', 'original', or null",
        );
    }
    if let Some(Some(d)) = req.default_view_mode.as_ref()
        && !matches!(d.as_str(), "single" | "double" | "webtoon")
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "default_view_mode must be 'single', 'double', 'webtoon', or null",
        );
    }
    if let Some(Some(t)) = req.theme.as_ref()
        && !matches!(t.as_str(), "system" | "dark" | "light" | "amber")
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "theme must be 'system', 'dark', 'light', 'amber', or null",
        );
    }
    if let Some(Some(c)) = req.accent_color.as_ref()
        && !matches!(c.as_str(), "amber" | "blue" | "emerald" | "rose")
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "accent_color must be 'amber', 'blue', 'emerald', 'rose', or null",
        );
    }
    if let Some(Some(d)) = req.density.as_ref()
        && !matches!(d.as_str(), "comfortable" | "compact")
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "density must be 'comfortable', 'compact', or null",
        );
    }
    if let Some(kb) = req.keybinds.as_ref() {
        if !kb.is_object() {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "keybinds must be an object",
            );
        }
        // Each value must be a string (key combo). Reject anything else early
        // so we never store a malformed binding the client can't render.
        if let Some(map) = kb.as_object() {
            for (action, key) in map {
                if !key.is_string() {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "validation",
                        &format!("keybinds.{action} must be a string"),
                    );
                }
            }
        }
    }
    if let Some(tz) = req.timezone.as_deref()
        && tz.parse::<chrono_tz::Tz>().is_err()
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "timezone must be a valid IANA zone",
        );
    }
    if let Some(v) = req.reading_min_active_ms
        && !(1_000..=600_000).contains(&v)
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "reading_min_active_ms must be between 1000 and 600000",
        );
    }
    if let Some(v) = req.reading_min_pages
        && !(1..=200).contains(&v)
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "reading_min_pages must be between 1 and 200",
        );
    }
    if let Some(lang) = req.language.as_deref()
        && !SUPPORTED_LOCALES.contains(&lang)
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation.language",
            &format!("language must be one of: {}", SUPPORTED_LOCALES.join(", ")),
        );
    }
    if let Some(v) = req.reading_idle_ms
        && !(30_000..=1_800_000).contains(&v)
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "reading_idle_ms must be between 30000 and 1800000",
        );
    }

    let row = match UserEntity::find_by_id(user.id).one(&app.db).await {
        Ok(Some(r)) => r,
        _ => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let mut am: UserAM = row.into();
    if let Some(v) = req.default_reading_direction {
        am.default_reading_direction = Set(v);
    }
    if let Some(v) = req.default_fit_mode {
        am.default_fit_mode = Set(v);
    }
    if let Some(v) = req.default_view_mode {
        am.default_view_mode = Set(v);
    }
    if let Some(v) = req.default_page_strip {
        am.default_page_strip = Set(v);
    }
    if let Some(v) = req.default_cover_solo {
        am.default_cover_solo = Set(v);
    }
    if let Some(v) = req.theme {
        am.theme = Set(v);
    }
    if let Some(v) = req.accent_color {
        am.accent_color = Set(v);
    }
    if let Some(v) = req.density {
        am.density = Set(v);
    }
    if let Some(v) = req.keybinds {
        am.keybinds = Set(v);
    }
    if let Some(v) = req.activity_tracking_enabled {
        am.activity_tracking_enabled = Set(v);
    }
    if let Some(v) = req.timezone {
        am.timezone = Set(v);
    }
    if let Some(v) = req.reading_min_active_ms {
        am.reading_min_active_ms = Set(v);
    }
    if let Some(v) = req.reading_min_pages {
        am.reading_min_pages = Set(v);
    }
    if let Some(v) = req.reading_idle_ms {
        am.reading_idle_ms = Set(v);
    }
    if let Some(v) = req.language.clone() {
        am.language = Set(v);
    }
    if let Some(v) = req.exclude_from_aggregates {
        am.exclude_from_aggregates = Set(v);
    }
    if let Some(v) = req.show_marker_count {
        am.show_marker_count = Set(v);
    }
    am.updated_at = Set(chrono::Utc::now().fixed_offset());
    let updated = match am.update(&app.db).await {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(error = %e, "preferences update failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let csrf = new_csrf_token();
    let body = me_resp_from_row(&updated, csrf.clone());
    let mut jar = jar.add(csrf_cookie(csrf, app.cfg().access_ttl()));
    // If the language preference changed, refresh `NEXT_LOCALE` so the next
    // navigation picks up the new locale immediately (no relog needed).
    if req.language.is_some() {
        jar = jar.add(crate::auth::cookies::locale_cookie(
            updated.language.clone(),
        ));
    }
    (StatusCode::OK, jar, Json(body)).into_response()
}

/// Locales the server will accept on `PATCH /me/preferences { language }`.
/// Mirrors `web/i18n/request.ts::SUPPORTED_LOCALES` — bump in lockstep
/// when adding a new locale.
const SUPPORTED_LOCALES: [&str; 1] = ["en"];

// ────────────── Recovery handlers (M4) ──────────────

/// `POST /auth/local/request-password-reset`
///
/// Always returns 204 — the response doesn't leak whether the email is on
/// file (otherwise the endpoint doubles as a user-enumeration oracle).
/// When the email maps to an active local account, a 1-hour reset token
/// is sent via the configured EmailSender.
#[utoipa::path(
    post,
    path = "/auth/local/request-password-reset",
    request_body = RequestPasswordResetReq,
    responses((status = 204, description = "request accepted (whether or not the email exists)"))
)]
pub async fn request_password_reset(
    State(app): State<AppState>,
    FormOrJson { data: req, format }: FormOrJson<RequestPasswordResetReq>,
) -> impl IntoResponse {
    if !matches!(app.cfg().auth_mode, AuthMode::Local | AuthMode::Both) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let email_lower = req.email.trim().to_lowercase();
    // Look up the user. If not found, or not a local user, or disabled,
    // fall through to the 204 — we don't want to leak account existence.
    let row = UserEntity::find()
        .filter(user::Column::Email.eq(email_lower.clone()))
        .one(&app.db)
        .await
        .ok()
        .flatten();
    if let Some(row) = row
        && row.state != "disabled"
        && row.external_id.starts_with("local:")
        && let Some(addr) = row.email.as_deref()
    {
        let tok = email_token::issue(
            TokenPurpose::PasswordReset,
            row.id,
            PASSWORD_RESET_TTL,
            app.secrets.email_token_key.as_ref(),
        );
        let msg = templates::password_reset(&app.cfg().public_url, addr, &tok);
        if let Err(e) = app.send_email(msg).await {
            tracing::warn!(error = %e, user_id = %row.id, "password-reset send failed");
        } else {
            tracing::info!(user_id = %row.id, "password-reset email sent");
        }
    } else {
        // Don't log the email — log only the absence of a usable account.
        tracing::debug!("password-reset requested for unknown / non-local / disabled account");
    }
    match format {
        // JSON path keeps the 204 ("accepted; whether or not the email
        // matches an account is intentionally indistinguishable").
        ResponseFormat::Json => StatusCode::NO_CONTENT.into_response(),
        // Form path bounces back to /forgot-password with the same
        // anti-enumeration semantic — the page just shows the "check your
        // email" confirmation regardless.
        ResponseFormat::Form => Redirect::to("/forgot-password?sent=1").into_response(),
    }
}

/// `POST /auth/local/reset-password { token, new_password }`
///
/// Verifies the HMAC token, rehashes the new password, bumps
/// `token_version` (so every existing session for the user is
/// invalidated), and sends a confirmation email. Returns 204 on success.
#[utoipa::path(
    post,
    path = "/auth/local/reset-password",
    request_body = ResetPasswordReq,
    responses(
        (status = 204, description = "password reset; all other sessions revoked"),
        (status = 400, description = "token invalid, expired, or malformed"),
    )
)]
pub async fn reset_password(
    State(app): State<AppState>,
    FormOrJson { data: req, format }: FormOrJson<ResetPasswordReq>,
) -> impl IntoResponse {
    // The reset page POSTs the token in the body, so we redirect failures
    // back to the reset URL *with the token* so the user can retry on the
    // same page (only invalid-token cases bounce to forgot-password).
    let reset_target = format!(
        "/reset-password?token={}",
        urlencoding::encode(req.token.as_str())
    );
    let fail_at_reset = |status: StatusCode, code: &str, msg: &str| -> axum::response::Response {
        auth_failure_response(format, &reset_target, None, status, code, msg)
    };
    let fail_at_forgot = |status: StatusCode, code: &str, msg: &str| -> axum::response::Response {
        auth_failure_response(format, "/forgot-password", None, status, code, msg)
    };

    if !matches!(app.cfg().auth_mode, AuthMode::Local | AuthMode::Both) {
        return fail_at_reset(StatusCode::NOT_FOUND, "not_found", "local auth disabled");
    }
    if req.new_password.len() < 12 {
        return fail_at_reset(
            StatusCode::BAD_REQUEST,
            "validation",
            "password must be at least 12 characters",
        );
    }
    // Confirm-password parity (form fallback only — the JS path validates
    // before submit). If the field is present and doesn't match, reject.
    if let Some(confirm) = req.confirm_password.as_deref()
        && confirm != req.new_password
    {
        return fail_at_reset(
            StatusCode::BAD_REQUEST,
            "validation",
            "passwords do not match",
        );
    }
    let user_id = match email_token::verify(
        TokenPurpose::PasswordReset,
        &req.token,
        app.secrets.email_token_key.as_ref(),
    ) {
        Ok(uid) => uid,
        Err(e) => {
            tracing::info!(error = ?e, "reset-password token rejected");
            return fail_at_forgot(
                StatusCode::BAD_REQUEST,
                "auth.token_invalid",
                "reset link is invalid or expired",
            );
        }
    };

    let row = match UserEntity::find_by_id(user_id).one(&app.db).await {
        Ok(Some(r)) => r,
        _ => {
            return fail_at_forgot(
                StatusCode::BAD_REQUEST,
                "auth.token_invalid",
                "reset link is invalid or expired",
            );
        }
    };
    if row.state == "disabled" || !row.external_id.starts_with("local:") {
        return fail_at_forgot(
            StatusCode::BAD_REQUEST,
            "auth.token_invalid",
            "reset link is invalid or expired",
        );
    }

    let hash = match password::hash(&req.new_password, app.secrets.pepper.as_ref()) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(error = %e, "argon2 hash failed during reset");
            return fail_at_reset(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let mut am: UserAM = row.clone().into();
    am.password_hash = Set(Some(hash));
    am.token_version = Set(row.token_version + 1);
    am.updated_at = Set(chrono::Utc::now().fixed_offset());
    if let Err(e) = am.update(&app.db).await {
        tracing::error!(error = %e, user_id = %user_id, "reset-password update failed");
        return fail_at_reset(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    // Best-effort confirmation email so a compromised user notices.
    if let Some(addr) = row.email.as_deref() {
        let confirm = templates::password_changed(&app.cfg().public_url, addr);
        if let Err(e) = app.send_email(confirm).await {
            tracing::warn!(error = %e, user_id = %user_id, "password-changed confirmation send failed");
        }
    }
    tracing::info!(user_id = %user_id, "password reset via email token");

    match format {
        ResponseFormat::Json => StatusCode::NO_CONTENT.into_response(),
        ResponseFormat::Form => Redirect::to("/sign-in?reset=1").into_response(),
    }
}

/// `GET /auth/local/verify-email?token=...`
///
/// Consumes a 24h verification token; on success flips
/// `state=pending_verification` → `active` and `email_verified=true`, then
/// 302s to `/sign-in?verified=1`. Re-clicking a still-valid token after
/// activation is a 302 to the same target (idempotent / harmless).
#[utoipa::path(
    get,
    path = "/auth/local/verify-email",
    responses(
        (status = 302, description = "redirect to /sign-in?verified=1"),
        (status = 400, description = "token invalid or expired"),
    )
)]
pub async fn verify_email(
    State(app): State<AppState>,
    Query(q): Query<VerifyEmailQuery>,
) -> impl IntoResponse {
    if !matches!(app.cfg().auth_mode, AuthMode::Local | AuthMode::Both) {
        return error(StatusCode::NOT_FOUND, "not_found", "local auth disabled");
    }
    let user_id = match email_token::verify(
        TokenPurpose::EmailVerification,
        &q.token,
        app.secrets.email_token_key.as_ref(),
    ) {
        Ok(uid) => uid,
        Err(e) => {
            tracing::info!(error = ?e, "verify-email token rejected");
            return error(
                StatusCode::BAD_REQUEST,
                "auth.token_invalid",
                "verification link is invalid or expired",
            );
        }
    };
    let row = match UserEntity::find_by_id(user_id).one(&app.db).await {
        Ok(Some(r)) => r,
        _ => {
            return error(
                StatusCode::BAD_REQUEST,
                "auth.token_invalid",
                "verification link is invalid or expired",
            );
        }
    };
    if row.state == "active" {
        // Idempotent re-click — just bounce to sign-in.
        return Redirect::to("/sign-in?verified=1").into_response();
    }
    if row.state != "pending_verification" {
        return error(
            StatusCode::BAD_REQUEST,
            "auth.token_invalid",
            "verification link is invalid or expired",
        );
    }
    let mut am: UserAM = row.clone().into();
    am.state = Set("active".into());
    am.email_verified = Set(true);
    am.updated_at = Set(chrono::Utc::now().fixed_offset());
    if let Err(e) = am.update(&app.db).await {
        tracing::error!(error = %e, user_id = %user_id, "verify-email update failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    tracing::info!(user_id = %user_id, "email verified");
    Redirect::to("/sign-in?verified=1").into_response()
}

/// `POST /auth/local/resend-verification { email }`
///
/// Always 204 (no enumeration). Sends a fresh 24h token only when the
/// account exists, is local, and is `pending_verification`.
#[utoipa::path(
    post,
    path = "/auth/local/resend-verification",
    request_body = ResendVerificationReq,
    responses((status = 204, description = "request accepted (whether or not the email exists)"))
)]
pub async fn resend_verification(
    State(app): State<AppState>,
    Json(req): Json<ResendVerificationReq>,
) -> impl IntoResponse {
    if !matches!(app.cfg().auth_mode, AuthMode::Local | AuthMode::Both) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let email_lower = req.email.trim().to_lowercase();
    let row = UserEntity::find()
        .filter(user::Column::Email.eq(email_lower))
        .one(&app.db)
        .await
        .ok()
        .flatten();
    if let Some(row) = row
        && row.state == "pending_verification"
        && row.external_id.starts_with("local:")
        && let Some(addr) = row.email.as_deref()
    {
        let tok = email_token::issue(
            TokenPurpose::EmailVerification,
            row.id,
            VERIFY_EMAIL_TTL,
            app.secrets.email_token_key.as_ref(),
        );
        let msg = templates::verify_email(&app.cfg().public_url, addr, &tok);
        if let Err(e) = app.send_email(msg).await {
            tracing::warn!(error = %e, user_id = %row.id, "resend-verification send failed");
        } else {
            tracing::info!(user_id = %row.id, "verify-email resent");
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

// ────────────── Helpers ──────────────

/// Validate a `next` redirect target through the same allowlist used by OIDC
/// (`is_safe_redirect_target`). Returns `Some(_)` only for in-app, absolute
/// paths with no protocol escape vectors. Trims whitespace first so a
/// stray newline in a copy-pasted form value doesn't make it through.
fn sanitize_next(next: Option<&str>) -> Option<String> {
    next.map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| crate::auth::oidc::is_safe_redirect_target(s))
        .map(str::to_owned)
}

/// Build the response for an auth-handler failure: JSON envelope on the
/// JSON path, 303 → `<base>?error=&message=&next=` on the form path.
/// Centralized so every early-return shares the same shape.
fn auth_failure_response(
    format: ResponseFormat,
    base: &str,
    next: Option<&str>,
    status: StatusCode,
    code: &str,
    message: &str,
) -> axum::response::Response {
    match format {
        ResponseFormat::Json => error(status, code, message),
        ResponseFormat::Form => {
            Redirect::to(&redirect_with_error(base, code, message, next)).into_response()
        }
    }
}

async fn issue_session(
    app: &AppState,
    user_row: &user::Model,
    jar: CookieJar,
    ctx: &RequestContext,
    success_status: StatusCode,
    format: ResponseFormat,
    redirect_to: Option<&str>,
) -> axum::response::Response {
    let session_id = Uuid::now_v7();
    let raw_rt = new_refresh_token_raw();
    let hash = sha256_hex(&raw_rt);
    let now = chrono::Utc::now();
    let expires = now + chrono::Duration::seconds(app.cfg().refresh_ttl().as_secs() as i64);

    let am = SessionAM {
        id: Set(session_id),
        user_id: Set(user_row.id),
        refresh_token_hash: Set(hash),
        created_at: Set(now.fixed_offset()),
        last_used_at: Set(now.fixed_offset()),
        expires_at: Set(expires.fixed_offset()),
        user_agent: Set(ctx.user_agent.clone()),
        ip: Set(ctx.ip_string()),
        revoked_at: Set(None),
        // Local sessions never participate in RP-initiated logout.
        id_token_hint: Set(None),
    };
    if let Err(e) = am.insert(&app.db).await {
        tracing::error!(error = %e, "auth_session insert failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    finalize_session(
        app,
        user_row,
        session_id,
        raw_rt,
        jar,
        success_status,
        format,
        redirect_to,
    )
}

#[allow(clippy::too_many_arguments)]
fn finalize_session(
    app: &AppState,
    user_row: &user::Model,
    session_id: Uuid,
    raw_rt: String,
    jar: CookieJar,
    success_status: StatusCode,
    format: ResponseFormat,
    redirect_to: Option<&str>,
) -> axum::response::Response {
    let keys = match JwtKeys::from_secret(&app.secrets.jwt_ed25519, &app.cfg().public_url) {
        Ok(k) => k,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };

    let access_ttl = app.cfg().access_ttl();
    let refresh_ttl = app.cfg().refresh_ttl();
    let access = match keys.issue_access(
        user_row.id,
        &user_row.role,
        user_row.token_version,
        ChronoDuration::seconds(access_ttl.as_secs() as i64),
    ) {
        Ok(t) => t,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };

    // Refresh token is a raw 32-byte random value (§17.2). Cookie value = raw,
    // DB column = sha256(raw). Single-use rotation by hash compare.
    let _ = session_id; // session_id is recorded in the DB row; not embedded in the cookie.
    let cookie_rt = raw_rt;

    let csrf = new_csrf_token();

    let body = LoginResp {
        user: me_resp_from_row(user_row, csrf.clone()),
    };

    let jar = jar
        .add(session_cookie(access, access_ttl))
        .add(refresh_cookie(cookie_rt, refresh_ttl))
        .add(csrf_cookie(csrf, access_ttl))
        // Mirror the user's stored language onto `NEXT_LOCALE` so next-intl
        // picks it up on the next nav. Authenticated locale wins over any
        // pre-existing `Accept-Language`-derived cookie value.
        .add(crate::auth::cookies::locale_cookie(
            user_row.language.clone(),
        ));

    match format {
        ResponseFormat::Json => (success_status, jar, Json(body)).into_response(),
        ResponseFormat::Form => {
            // Progressive-enhancement happy path: 303 → validated `next` or
            // `/`. `Redirect::to` already sets 303 See Other, which is the
            // POST-redirect-GET status we want — the browser follows with
            // GET, picking up the cookies on the way.
            let target = redirect_to.unwrap_or("/");
            (jar, Redirect::to(target)).into_response()
        }
    }
}

fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    format!("{:x}", h.finalize())
}

/// Build a `MeResp` from a fully-loaded user row. Used by login, refresh,
/// and the preferences PATCH so the response shape stays consistent.
pub(crate) fn me_resp_from_row(row: &user::Model, csrf_token: String) -> MeResp {
    MeResp {
        id: row.id.to_string(),
        email: row.email.clone(),
        display_name: row.display_name.clone(),
        role: row.role.clone(),
        csrf_token,
        default_reading_direction: row.default_reading_direction.clone(),
        default_fit_mode: row.default_fit_mode.clone(),
        default_view_mode: row.default_view_mode.clone(),
        default_page_strip: row.default_page_strip,
        default_cover_solo: row.default_cover_solo,
        theme: row.theme.clone(),
        accent_color: row.accent_color.clone(),
        density: row.density.clone(),
        keybinds: row.keybinds.clone(),
        activity_tracking_enabled: row.activity_tracking_enabled,
        timezone: row.timezone.clone(),
        reading_min_active_ms: row.reading_min_active_ms,
        reading_min_pages: row.reading_min_pages,
        reading_idle_ms: row.reading_idle_ms,
        language: row.language.clone(),
        exclude_from_aggregates: row.exclude_from_aggregates,
        show_marker_count: row.show_marker_count,
    }
}

/// Build a `MeResp` from the auth extractor + an optionally re-fetched row.
/// `/auth/me` uses this — when the row is missing (transient DB blip) the
/// preference fields fall back to defaults so the client still hydrates.
fn me_resp_from_parts(user: &CurrentUser, csrf_token: String, row: Option<&user::Model>) -> MeResp {
    if let Some(row) = row {
        me_resp_from_row(row, csrf_token)
    } else {
        MeResp {
            id: user.id.to_string(),
            email: user.email.clone(),
            display_name: user.display_name.clone(),
            role: user.role.clone(),
            csrf_token,
            default_reading_direction: None,
            default_fit_mode: None,
            default_view_mode: None,
            default_page_strip: false,
            default_cover_solo: true,
            theme: None,
            accent_color: None,
            density: None,
            keybinds: serde_json::json!({}),
            activity_tracking_enabled: true,
            timezone: "UTC".into(),
            reading_min_active_ms: 30_000,
            reading_min_pages: 3,
            reading_idle_ms: 180_000,
            language: "en".into(),
            exclude_from_aggregates: false,
            show_marker_count: false,
        }
    }
}

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
