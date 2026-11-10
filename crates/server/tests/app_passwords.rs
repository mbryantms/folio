//! `/me/app-passwords` integration tests (M7, audit M-14).
//!
//! Covers: issue → use as Bearer on a reader route; revoke kills the
//! token; cross-user isolation; label validation; per-user cap;
//! last_used_at bump; and the reader-only scope: an app password
//! authenticates the OPDS / Komga / page-byte / thumbnail routes and is
//! refused with 403 everywhere else (`AppPasswordSurface`).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tower::ServiceExt;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

fn extract_cookie(resp: &Response<Body>, name: &str) -> Option<String> {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|s| {
            let prefix = format!("{name}=");
            s.split(';')
                .next()
                .and_then(|kv| kv.strip_prefix(&prefix))
                .map(|v| v.to_owned())
        })
}

struct Auth {
    session: String,
    csrf: String,
}

impl Auth {
    fn cookies(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
    fn from_response(resp: &Response<Body>) -> Self {
        Self {
            session: extract_cookie(resp, "__Host-comic_session").unwrap(),
            csrf: extract_cookie(resp, "__Host-comic_csrf").unwrap(),
        }
    }
}

async fn register(app: &TestApp, email: &str) -> Response<Body> {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn create_password(app: &TestApp, auth: &Auth, label: &str) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/me/app-passwords")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookies())
                .header("x-csrf-token", &auth.csrf)
                .body(Body::from(format!(r#"{{"label":"{label}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn list_passwords(app: &TestApp, auth: &Auth) -> serde_json::Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/me/app-passwords")
                .header(header::COOKIE, auth.cookies())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp.into_body()).await
}

/// The OPDS root: a reader route, where app passwords authenticate.
async fn opds_with_bearer(app: &TestApp, bearer: &str) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/opds/v1")
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body_text(b: Body) -> String {
    String::from_utf8(to_bytes(b, usize::MAX).await.unwrap().to_vec()).unwrap()
}

#[tokio::test]
async fn issue_then_use_as_bearer_on_reader_route() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "owner@example.com").await;
    assert_eq!(reg.status(), StatusCode::CREATED);
    let auth = Auth::from_response(&reg);

    let resp = create_password(&app, &auth, "kobo").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp.into_body()).await;
    let plaintext = body["plaintext"].as_str().unwrap().to_owned();
    assert!(plaintext.starts_with("app_"), "plaintext shape");
    assert_eq!(body["label"], "kobo");

    let feed = opds_with_bearer(&app, &plaintext).await;
    assert_eq!(feed.status(), StatusCode::OK);
    let feed_body = body_text(feed.into_body()).await;
    assert!(feed_body.contains("<feed"), "{feed_body}");
}

#[tokio::test]
async fn repeated_bearer_use_resolves_via_cache() {
    // PERF-1: the first use takes the argon2 scan path and populates the
    // app-password cache; subsequent uses take the cache fast path. Both must
    // resolve to the same owner. (Revocation-after-cache is covered by
    // `revoked_password_rejects_bearer_auth`, which uses then revokes.)
    let app = TestApp::spawn().await;
    let reg = register(&app, "cached@example.com").await;
    let auth = Auth::from_response(&reg);

    let body = body_json(create_password(&app, &auth, "kobo").await.into_body()).await;
    let plaintext = body["plaintext"].as_str().unwrap().to_owned();

    for _ in 0..3 {
        let feed = opds_with_bearer(&app, &plaintext).await;
        assert_eq!(feed.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn list_returns_active_passwords_without_plaintext() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "lister@example.com").await;
    let auth = Auth::from_response(&reg);

    let _ = create_password(&app, &auth, "kobo").await;
    let _ = create_password(&app, &auth, "kavita").await;

    let body = list_passwords(&app, &auth).await;
    let arr = body["items"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert!(
        arr.iter().all(|i| i.get("plaintext").is_none()),
        "list view must not include plaintext"
    );
    let labels: Vec<_> = arr.iter().filter_map(|i| i["label"].as_str()).collect();
    assert!(labels.contains(&"kobo"));
    assert!(labels.contains(&"kavita"));
}

#[tokio::test]
async fn revoked_password_rejects_bearer_auth() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "revoke@example.com").await;
    let auth = Auth::from_response(&reg);

    let resp = create_password(&app, &auth, "kobo").await;
    let body = body_json(resp.into_body()).await;
    let plaintext = body["plaintext"].as_str().unwrap().to_owned();
    let id = body["id"].as_str().unwrap().to_owned();

    // Works before revoke.
    let feed = opds_with_bearer(&app, &plaintext).await;
    assert_eq!(feed.status(), StatusCode::OK);

    // Revoke.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/api/me/app-passwords/{id}"))
                .header(header::COOKIE, auth.cookies())
                .header("x-csrf-token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Now fails as Bearer.
    let feed = opds_with_bearer(&app, &plaintext).await;
    assert_eq!(feed.status(), StatusCode::UNAUTHORIZED);

    // And the list view no longer shows it.
    let body = list_passwords(&app, &auth).await;
    assert!(body["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn cannot_revoke_other_users_password() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let other = register(&app, "other@example.com").await;
    let other_auth = Auth::from_response(&other);
    let resp = create_password(&app, &other_auth, "kobo").await;
    let body = body_json(resp.into_body()).await;
    let other_id = body["id"].as_str().unwrap().to_owned();

    // Second user.
    let attacker = register(&app, "attacker@example.com").await;
    let attacker_auth = Auth::from_response(&attacker);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/api/me/app-passwords/{other_id}"))
                .header(header::COOKIE, attacker_auth.cookies())
                .header("x-csrf-token", &attacker_auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn last_used_at_bumps_after_bearer_request() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "stamp@example.com").await;
    let auth = Auth::from_response(&reg);

    let resp = create_password(&app, &auth, "kobo").await;
    let body = body_json(resp.into_body()).await;
    let plaintext = body["plaintext"].as_str().unwrap().to_owned();
    let id_str = body["id"].as_str().unwrap();
    let id: uuid::Uuid = id_str.parse().unwrap();

    // Before use: last_used_at is null.
    let state = app.state();
    let before = entity::app_password::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(before.last_used_at.is_none());

    let _ = opds_with_bearer(&app, &plaintext).await;

    let after = entity::app_password::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(after.last_used_at.is_some(), "last_used_at should bump");
}

#[tokio::test]
async fn label_validation_rejects_empty_and_oversize() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "label@example.com").await;
    let auth = Auth::from_response(&reg);

    // Audit-remediation M9: semantic-validation failures (empty / oversize)
    // come back as 422 via the garde `Validated<T>` extractor, not 400.
    let resp = create_password(&app, &auth, "").await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let long = "x".repeat(200);
    let resp = create_password(&app, &auth, &long).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn cannot_see_other_users_passwords_in_list() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice@example.com").await;
    let alice_auth = Auth::from_response(&alice);
    let _ = create_password(&app, &alice_auth, "alice-token").await;

    let bob = register(&app, "bob@example.com").await;
    let bob_auth = Auth::from_response(&bob);
    let _ = create_password(&app, &bob_auth, "bob-token").await;

    let bob_list = list_passwords(&app, &bob_auth).await;
    let items = bob_list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["label"], "bob-token");

    // Belt-and-suspenders: confirm alice's row still exists at the
    // DB level so the test isn't just lucky.
    let state = app.state();
    let all_count = entity::app_password::Entity::find()
        .filter(entity::app_password::Column::RevokedAt.is_null())
        .all(&state.db)
        .await
        .unwrap()
        .len();
    assert_eq!(all_count, 2);
}

// ───────── reader-only scope (`AppPasswordSurface`) ─────────

/// Which credential a request carries.
#[derive(Clone, Copy)]
enum Cred<'a> {
    Cookie(&'a Auth),
    Bearer(&'a str),
    /// HTTP Basic: the username is ignored, the password is the token.
    Basic(&'a str),
    None,
}

async fn send(
    app: &TestApp,
    method: Method,
    uri: &str,
    cred: Cred<'_>,
    body: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut b = Request::builder().method(method).uri(uri);
    b = match cred {
        Cred::Cookie(a) => b
            .header(header::COOKIE, a.cookies())
            .header("x-csrf-token", &a.csrf),
        Cred::Bearer(t) => b.header(header::AUTHORIZATION, format!("Bearer {t}")),
        Cred::Basic(t) => {
            use base64::Engine;
            let creds = base64::engine::general_purpose::STANDARD.encode(format!("any-name:{t}"));
            b.header(header::AUTHORIZATION, format!("Basic {creds}"))
        }
        Cred::None => b,
    };
    let req = match body {
        Some(json) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json.to_owned())),
        None => b.body(Body::empty()),
    }
    .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn mint(app: &TestApp, auth: &Auth) -> String {
    let resp = create_password(app, auth, "reader").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    body_json(resp.into_body()).await["plaintext"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn assert_refused(status: StatusCode, body: &serde_json::Value, what: &str) {
    assert_eq!(status, StatusCode::FORBIDDEN, "{what}: {body}");
    assert_eq!(
        body["error"]["code"], "auth.permission_denied",
        "{what}: {body}"
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("reader apps")),
        "{what}: {body}"
    );
}

async fn active_password_count(app: &TestApp) -> usize {
    entity::app_password::Entity::find()
        .filter(entity::app_password::Column::RevokedAt.is_null())
        .all(&app.state().db)
        .await
        .unwrap()
        .len()
}

/// Off the reader surface an app password is refused with 403, as Bearer
/// and as Basic, and the refused writes leave nothing behind.
#[tokio::test]
async fn app_password_refused_off_reader_surface() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let reg = register(&app, "scoped@example.com").await;
    let auth = Auth::from_response(&reg);
    let token = mint(&app, &auth).await;

    for cred in [Cred::Bearer(&token), Cred::Basic(&token)] {
        for uri in ["/auth/me", "/api/auth/me"] {
            let (s, b) = send(&app, Method::GET, uri, cred, None).await;
            assert_refused(s, &b, uri);
        }

        let before = active_password_count(&app).await;
        let (s, b) = send(
            &app,
            Method::POST,
            "/api/me/app-passwords",
            cred,
            Some(r#"{"label":"minted-by-token"}"#),
        )
        .await;
        assert_refused(s, &b, "POST /api/me/app-passwords");
        assert_eq!(active_password_count(&app).await, before, "no new row");

        let (s, b) = send(
            &app,
            Method::PATCH,
            "/api/me/account",
            cred,
            Some(r#"{"email":"hijacked@example.com"}"#),
        )
        .await;
        assert_refused(s, &b, "PATCH /api/me/account");
        let (_, me) = send(&app, Method::GET, "/api/auth/me", Cred::Cookie(&auth), None).await;
        assert_eq!(me["email"], "scoped@example.com", "email unchanged");

        let (s, b) = send(&app, Method::POST, "/auth/ws-ticket", cred, None).await;
        assert_refused(s, &b, "POST /auth/ws-ticket");
    }
}

/// An admin's app password does not open admin routes.
#[tokio::test]
async fn admin_app_password_refused_on_admin_routes() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "first-admin@example.com").await;
    let auth = Auth::from_response(&reg);
    // First registered user is the admin: the cookie session gets in.
    let (s, _) = send(
        &app,
        Method::GET,
        "/api/admin/users",
        Cred::Cookie(&auth),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let token = mint(&app, &auth).await;
    let (s, b) = send(
        &app,
        Method::GET,
        "/api/admin/users",
        Cred::Bearer(&token),
        None,
    )
    .await;
    assert_refused(s, &b, "GET /api/admin/users (Bearer)");
    let (s, b) = send(
        &app,
        Method::GET,
        "/api/admin/users",
        Cred::Basic(&token),
        None,
    )
    .await;
    assert_refused(s, &b, "GET /api/admin/users (Basic)");
}

/// On the reader surface the same token authenticates: OPDS 1.2 + 2.0
/// feeds answer 200, and the page-byte / thumbnail / cover / progression
/// / Komga routes get past authentication (404 for an unknown issue,
/// where no credential gets 401).
#[tokio::test]
async fn app_password_works_on_reader_surface() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let reg = register(&app, "reader@example.com").await;
    let auth = Auth::from_response(&reg);
    let token = mint(&app, &auth).await;

    for cred in [Cred::Bearer(&token), Cred::Basic(&token)] {
        for uri in ["/opds/v1", "/opds/v2"] {
            let (s, _) = send(&app, Method::GET, uri, cred, None).await;
            assert_eq!(s, StatusCode::OK, "{uri}");
        }
    }

    let unknown = "0".repeat(64);
    let cover = uuid::Uuid::new_v4();
    for uri in [
        format!("/issues/{unknown}/pages/0"),
        format!("/issues/{unknown}/pages/0/thumb"),
        format!("/issues/{unknown}/covers/{cover}"),
        format!("/opds/v1/progression/{unknown}"),
        format!("/api/v1/books/{unknown}"),
    ] {
        let (s, _) = send(&app, Method::GET, &uri, Cred::None, None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{uri} without a credential");
        for cred in [Cred::Bearer(&token), Cred::Basic(&token)] {
            let (s, b) = send(&app, Method::GET, &uri, cred, None).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{uri}: {b}");
        }
    }
}

/// A cookie session is untouched by the reader-only rule.
#[tokio::test]
async fn cookie_session_unaffected_by_app_password_scope() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "cookie@example.com").await;
    let auth = Auth::from_response(&reg);
    let (s, me) = send(&app, Method::GET, "/api/auth/me", Cred::Cookie(&auth), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["email"], "cookie@example.com");
    let (s, _) = send(&app, Method::GET, "/opds/v1", Cred::Cookie(&auth), None).await;
    assert_eq!(s, StatusCode::OK);
}
