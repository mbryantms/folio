//! `RequireAdmin` extractor coverage (M1).
//!
//! Confirms the structural admin guard rejects non-admins with 403
//! `auth.permission_denied` and accepts admins. Also verifies the request
//! context middleware persists `auth_sessions.ip` + `.user_agent`.
//!
//! E15: the account-state rule — only `state = active` authenticates, on
//! every credential (cookie, Bearer JWT, app-password Bearer + Basic,
//! refresh). A `pending_verification` admin is refused by `RequireAdmin` and
//! by `CurrentUser`; the email-verification flow still works end to end.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use base64::Engine;
use common::TestApp;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tower::ServiceExt;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.expect("collect body");
    serde_json::from_slice(&bytes).expect("json body")
}

struct Authed {
    session: String,
    csrf: String,
    refresh: String,
    user_id: String,
}

impl Authed {
    fn cookie(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
}

async fn register(app: &TestApp, email: &str, user_agent: &str) -> Authed {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::USER_AGENT, user_agent)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let cookies: Vec<String> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_owned)
        .collect();
    let extract = |prefix: &str| -> String {
        cookies
            .iter()
            .find(|c| c.starts_with(prefix))
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap()
                    .trim_start_matches(prefix)
                    .to_owned()
            })
            .expect(prefix)
    };
    let body = body_json(resp.into_body()).await;
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        refresh: extract("__Host-comic_refresh="),
        user_id: body["user"]["id"].as_str().unwrap().to_owned(),
    }
}

async fn admin_get(app: &TestApp, auth: &Authed, uri: &str) -> StatusCode {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

#[tokio::test]
async fn require_admin_rejects_non_admin() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com", "agent/admin").await;
    let user = register(&app, "user@example.com", "agent/user").await;
    // /admin/auth/config is the smallest admin-only handler that uses the new
    // extractor (no other side effects, no path params).
    let status = admin_get(&app, &user, "/api/admin/auth/config").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn require_admin_accepts_admin() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com", "agent/admin").await;
    let status = admin_get(&app, &admin, "/api/admin/auth/config").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn require_admin_returns_envelope() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com", "agent/admin").await;
    let user = register(&app, "user@example.com", "agent/user").await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/admin/auth/config")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        user.session, user.csrf
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.permission_denied");
}

#[tokio::test]
async fn session_persists_user_agent_from_request() {
    let app = TestApp::spawn().await;
    let authed = register(&app, "ua@example.com", "Mozilla/5.0 (RequestContextTest)").await;

    let user_uuid = uuid::Uuid::parse_str(&authed.user_id).expect("valid user id");
    let state = app.state();
    let rows = entity::auth_session::Entity::find()
        .filter(entity::auth_session::Column::UserId.eq(user_uuid))
        .all(&state.db)
        .await
        .expect("query auth_sessions");
    assert_eq!(rows.len(), 1, "expected exactly one session per register");
    let session = &rows[0];
    assert_eq!(
        session.user_agent.as_deref(),
        Some("Mozilla/5.0 (RequestContextTest)"),
        "user-agent header should be persisted on session insert",
    );
    // Without ConnectInfo wired through the test harness the fallback peer is
    // 127.0.0.1 — we just assert it's non-null so future XFF wiring is the
    // only thing left to test once the production listener is exercised.
    assert!(
        session.ip.is_some(),
        "ip should be persisted (fallback peer in tests)",
    );
}

// ───────── account-state rule (audit-2026-11 E15) ─────────

/// Which credential a request carries.
enum Cred<'a> {
    Cookie(&'a Authed),
    Bearer(&'a str),
    Basic(&'a str),
}

async fn get_with(app: &TestApp, uri: &str, cred: Cred<'_>) -> (StatusCode, serde_json::Value) {
    let builder = Request::builder().method(Method::GET).uri(uri);
    let builder = match cred {
        Cred::Cookie(a) => builder.header(header::COOKIE, a.cookie()),
        Cred::Bearer(t) => builder.header(header::AUTHORIZATION, format!("Bearer {t}")),
        Cred::Basic(t) => {
            let creds = base64::engine::general_purpose::STANDARD.encode(format!("reader:{t}"));
            builder.header(header::AUTHORIZATION, format!("Basic {creds}"))
        }
    };
    let resp = app
        .router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

async fn send_as(
    app: &TestApp,
    actor: &Authed,
    method: Method,
    uri: &str,
    body: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::COOKIE, actor.cookie())
                .header("x-csrf-token", &actor.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_owned())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

async fn promote(app: &TestApp, admin: &Authed, user: &Authed) {
    let uri = format!("/api/admin/users/{}", user.user_id);
    let (s, b) = send_as(app, admin, Method::PATCH, &uri, Some(r#"{"role":"admin"}"#)).await;
    assert_eq!(s, StatusCode::OK, "promote: {b}");
}

/// The admin moves `user` to `pending_verification` / back to `active`, as an
/// operator would (neither bumps `token_version`, so the user's tokens stay
/// cryptographically valid: only the state rule can refuse them).
async fn set_pending(app: &TestApp, admin: &Authed, user: &Authed, pending: bool) {
    let (s, b) = if pending {
        let uri = format!("/api/admin/users/{}", user.user_id);
        let body = r#"{"state":"pending_verification"}"#;
        send_as(app, admin, Method::PATCH, &uri, Some(body)).await
    } else {
        let uri = format!("/api/admin/users/{}/enable", user.user_id);
        send_as(app, admin, Method::POST, &uri, None).await
    };
    assert_eq!(s, StatusCode::OK, "set state: {b}");
}

fn assert_unverified(status: StatusCode, body: &serde_json::Value, what: &str) {
    assert_eq!(status, StatusCode::FORBIDDEN, "{what}: {body}");
    assert_eq!(
        body["error"]["code"], "auth.email_unverified",
        "{what}: {body}"
    );
}

async fn mint_app_password(app: &TestApp, auth: &Authed) -> String {
    let (s, body) = send_as(
        app,
        auth,
        Method::POST,
        "/api/me/app-passwords",
        Some(r#"{"label":"e15"}"#),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    body["plaintext"].as_str().unwrap().to_owned()
}

/// A `RequireAdmin` route and a `CurrentUser` route.
const ADMIN_ROUTE: &str = "/api/admin/auth/config";
const USER_ROUTE: &str = "/auth/me";

/// A signed-in admin moved to `pending_verification` is refused by
/// `RequireAdmin` and by `CurrentUser`, by cookie and by Bearer JWT; moved
/// back to `active`, the same tokens pass again.
#[tokio::test]
async fn pending_admin_is_refused_by_cookie_and_bearer() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com", "agent/admin").await;
    let other = register(&app, "other@example.com", "agent/other").await;
    promote(&app, &admin, &other).await;

    // Active admin: both credentials pass both extractors.
    for uri in [ADMIN_ROUTE, USER_ROUTE] {
        assert_eq!(
            get_with(&app, uri, Cred::Cookie(&other)).await.0,
            StatusCode::OK
        );
        assert_eq!(
            get_with(&app, uri, Cred::Bearer(&other.session)).await.0,
            StatusCode::OK
        );
    }

    set_pending(&app, &admin, &other, true).await;
    for uri in [ADMIN_ROUTE, USER_ROUTE] {
        let (s, b) = get_with(&app, uri, Cred::Cookie(&other)).await;
        assert_unverified(s, &b, &format!("cookie {uri}"));
        let (s, b) = get_with(&app, uri, Cred::Bearer(&other.session)).await;
        assert_unverified(s, &b, &format!("bearer {uri}"));
    }
    // The bootstrap admin is unaffected.
    assert_eq!(
        get_with(&app, ADMIN_ROUTE, Cred::Cookie(&admin)).await.0,
        StatusCode::OK
    );

    set_pending(&app, &admin, &other, false).await;
    for uri in [ADMIN_ROUTE, USER_ROUTE] {
        assert_eq!(
            get_with(&app, uri, Cred::Cookie(&other)).await.0,
            StatusCode::OK
        );
        assert_eq!(
            get_with(&app, uri, Cred::Bearer(&other.session)).await.0,
            StatusCode::OK
        );
    }
}

/// A pending admin can't reach the user-admin handlers, so it can't change
/// another account.
#[tokio::test]
async fn pending_admin_cannot_change_users() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com", "agent/admin").await;
    let other = register(&app, "other@example.com", "agent/other").await;
    promote(&app, &admin, &other).await;
    set_pending(&app, &admin, &other, true).await;

    let uri = format!("/api/admin/users/{}", admin.user_id);
    let (s, b) = send_as(
        &app,
        &other,
        Method::PATCH,
        &uri,
        Some(r#"{"role":"user"}"#),
    )
    .await;
    assert_unverified(s, &b, "pending admin PATCH");
    let uri = format!("/api/admin/users/{}/disable", admin.user_id);
    let (s, b) = send_as(&app, &other, Method::POST, &uri, None).await;
    assert_unverified(s, &b, "pending admin disable");

    let row = entity::user::Entity::find_by_id(uuid::Uuid::parse_str(&admin.user_id).unwrap())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.role, entity::user::UserRole::Admin, "target untouched");
    assert_eq!(
        row.state,
        entity::user::UserState::Active,
        "target untouched"
    );
}

/// App-passwords follow the same rule, as Bearer and as OPDS Basic.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_user_app_password_is_refused_bearer_and_basic() {
    const OPDS: &str = "/opds/v1/recent";
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com", "agent/admin").await;
    let reader = register(&app, "reader@example.com", "agent/reader").await;
    let token = mint_app_password(&app, &reader).await;

    assert_eq!(
        get_with(&app, USER_ROUTE, Cred::Bearer(&token)).await.0,
        StatusCode::OK
    );
    assert_eq!(
        get_with(&app, OPDS, Cred::Basic(&token)).await.0,
        StatusCode::OK
    );

    set_pending(&app, &admin, &reader, true).await;
    let (s, b) = get_with(&app, USER_ROUTE, Cred::Bearer(&token)).await;
    assert_unverified(s, &b, "app-password bearer");
    let (s, b) = get_with(&app, OPDS, Cred::Basic(&token)).await;
    assert_unverified(s, &b, "app-password basic");

    set_pending(&app, &admin, &reader, false).await;
    assert_eq!(
        get_with(&app, USER_ROUTE, Cred::Bearer(&token)).await.0,
        StatusCode::OK
    );
    assert_eq!(
        get_with(&app, OPDS, Cred::Basic(&token)).await.0,
        StatusCode::OK
    );
}

async fn refresh(app: &TestApp, auth: &Authed) -> StatusCode {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/refresh")
                .header(
                    header::COOKIE,
                    format!("{}; __Host-comic_refresh={}", auth.cookie(), auth.refresh),
                )
                .header("x-csrf-token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// A pending account can't rotate its refresh token into a fresh access
/// token either; the refused refresh rolls back, so the same refresh token
/// works once the account is active again.
#[tokio::test]
async fn pending_user_refresh_is_refused() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com", "agent/admin").await;
    let reader = register(&app, "reader@example.com", "agent/reader").await;

    set_pending(&app, &admin, &reader, true).await;
    assert_eq!(refresh(&app, &reader).await, StatusCode::UNAUTHORIZED);

    set_pending(&app, &admin, &reader, false).await;
    assert_eq!(refresh(&app, &reader).await, StatusCode::OK);
}

async fn post_json(app: &TestApp, uri: &str, body: serde_json::Value) -> axum::response::Response {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn session_cookie(resp: &axum::response::Response) -> Option<String> {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| c.split(';').next()?.strip_prefix("__Host-comic_session="))
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// The verification flow a pending user goes through needs no session:
/// register (202, no cookie) → login refused → resend → verify-email →
/// login → `CurrentUser` admits them.
#[tokio::test]
async fn pending_user_verification_flow_end_to_end() {
    let app = TestApp::spawn_with_smtp().await;
    let _admin = register(&app, "admin@example.com", "agent/admin").await;
    let creds = serde_json::json!({
        "email": "pending@example.com",
        "password": "correctly-horse-battery"
    });

    let resp = post_json(&app, "/auth/local/register", creds.clone()).await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert!(session_cookie(&resp).is_none(), "no session while pending");

    let resp = post_json(&app, "/auth/local/login", creds.clone()).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(session_cookie(&resp).is_none());
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.email_unverified", "{body}");

    app.email.clear().await;
    let resp = post_json(
        &app,
        "/auth/local/resend-verification",
        serde_json::json!({ "email": "pending@example.com" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let msg = app.email.last().await;
    let at = msg.body_text.find("token=").expect("verify link in email");
    let token: String = msg.body_text[at + "token=".len()..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/auth/local/verify-email?token={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let resp = post_json(&app, "/auth/local/login", creds).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let session = session_cookie(&resp).expect("session after verification");
    let (status, me) = get_with(&app, USER_ROUTE, Cred::Bearer(&session)).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["email"], "pending@example.com");
}
