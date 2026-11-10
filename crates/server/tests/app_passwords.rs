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
/// and as Basic, and the refused writes leave nothing behind. These
/// requests carry no session cookie, so the CSRF check lets them through
/// to the extractor, which answers with the message that says what an app
/// password is for.
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

// ───── CSRF: the app-password exemption is reader-routes only ─────

/// A cookie-authenticated request with the given extra `Authorization`
/// header and NO `X-CSRF-Token`.
async fn cookie_write_with_authorization(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: &Auth,
    authorization: &str,
    body: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, auth.cookies())
        .header(header::AUTHORIZATION, authorization);
    let req = match body {
        Some(json) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json.to_owned())),
        None => b.body(Body::empty()),
    }
    .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

fn basic(token: &str) -> String {
    use base64::Engine;
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("any-name:{token}"))
    )
}

fn assert_csrf_refused(status: StatusCode, body: &serde_json::Value, what: &str) {
    assert_eq!(status, StatusCode::FORBIDDEN, "{what}: {body}");
    assert_eq!(body["error"]["code"], "auth.csrf", "{what}: {body}");
}

/// Off the reader routes, an app-password-shaped `Authorization` header no
/// longer skips the CSRF check: a cookie-authenticated write without the
/// CSRF header is refused, whether the token is real or junk, Bearer or
/// Basic, on a `CurrentUser` route and on cookie-only logout.
#[tokio::test]
async fn app_password_header_gets_no_csrf_exemption_off_reader_routes() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let reg = register(&app, "csrf@example.com").await;
    let auth = Auth::from_response(&reg);
    let token = mint(&app, &auth).await;

    let headers = [
        basic("app_junkjunkjunk"),
        "Bearer app_junkjunkjunk".to_owned(),
        basic(&token),
        format!("Bearer {token}"),
    ];
    for authorization in &headers {
        let before = active_password_count(&app).await;
        let (s, b) = cookie_write_with_authorization(
            &app,
            Method::POST,
            "/api/me/app-passwords",
            &auth,
            authorization,
            Some(r#"{"label":"forged"}"#),
        )
        .await;
        assert_csrf_refused(s, &b, "POST /api/me/app-passwords");
        assert_eq!(active_password_count(&app).await, before, "no new row");

        for uri in ["/auth/logout", "/api/auth/logout"] {
            let (s, b) = cookie_write_with_authorization(
                &app,
                Method::POST,
                uri,
                &auth,
                authorization,
                None,
            )
            .await;
            assert_csrf_refused(s, &b, uri);
        }
    }

    // The session survived every refused logout.
    let (s, me) = send(&app, Method::GET, "/api/auth/me", Cred::Cookie(&auth), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["email"], "csrf@example.com");
}

/// A Bearer JWT stays exempt everywhere: no cookie, no CSRF header.
#[tokio::test]
async fn bearer_jwt_still_csrf_exempt() {
    let app = TestApp::spawn().await;
    let _reg = register(&app, "jwt@example.com").await;
    let row = entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq("jwt@example.com"))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let jwt = app
        .state()
        .jwt_keys
        .issue_access(
            row.id,
            "admin",
            row.token_version,
            chrono::Duration::hours(1),
        )
        .unwrap();
    let (s, b) = send(
        &app,
        Method::POST,
        "/api/me/app-passwords",
        Cred::Bearer(&jwt),
        Some(r#"{"label":"via-jwt"}"#),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
}

/// On a reader route an app password writes progress with no cookie and
/// no CSRF header (what reader apps send); with Basic, a request carrying
/// browser cross-site markers loses the exemption and is refused.
#[tokio::test]
async fn reader_route_app_password_write_and_browser_origin() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let reg = register(&app, "rw@example.com").await;
    let auth = Auth::from_response(&reg);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/me/app-passwords")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookies())
                .header("x-csrf-token", &auth.csrf)
                .body(Body::from(r#"{"label":"rw","scope":"read+progress"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let token = body_json(resp.into_body()).await["plaintext"]
        .as_str()
        .unwrap()
        .to_owned();

    // Unknown issue: getting past CSRF + auth lands on 404, never 403.
    let unknown = "0".repeat(64);
    let uri = format!("/opds/v1/issues/{unknown}/progress");
    let progress = r#"{"page":1}"#;
    for authorization in [basic(&token), format!("Bearer {token}")] {
        let req = Request::builder()
            .method(Method::PUT)
            .uri(&uri)
            .header(header::AUTHORIZATION, &authorization)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(progress))
            .unwrap();
        let resp = app.router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{authorization}");
    }

    // Basic + a browser marker: no exemption, so the CSRF check runs
    // (no cookie, no token) and refuses.
    for (name, value) in [
        ("origin", "https://elsewhere.example"),
        ("sec-fetch-site", "cross-site"),
        ("sec-fetch-site", "same-site"),
    ] {
        let req = Request::builder()
            .method(Method::PUT)
            .uri(&uri)
            .header(header::AUTHORIZATION, basic(&token))
            .header(name, value)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(progress))
            .unwrap();
        let resp = app.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = body_json(resp.into_body()).await;
        assert_csrf_refused(status, &body, &format!("Basic + {name}: {value}"));
    }

    // Bearer is never attached by a browser on its own: it stays exempt
    // on reader routes even with an Origin header.
    let req = Request::builder()
        .method(Method::PUT)
        .uri(&uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header("origin", "https://elsewhere.example")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(progress))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Pull the string literal that follows each `marker` in `src`.
fn literals_after(src: &str, marker: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find(marker) {
        rest = &rest[i + marker.len()..];
        let trimmed = rest.trim_start();
        if let Some(lit) = trimmed.strip_prefix('"')
            && let Some(end) = lit.find('"')
        {
            out.push(lit[..end].to_owned());
        }
    }
    out
}

fn module_source(module: &str) -> String {
    let (dir, name) = module.split_once("::").unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let file = root.join(dir).join(format!("{name}.rs"));
    let file = if file.exists() {
        file
    } else {
        root.join(dir).join(name).join("mod.rs")
    };
    std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))
}

/// The CSRF middleware decides "reader route" from
/// `READER_ROUTE_PREFIXES`; the extractor decides it from the
/// `reader_surface(...)` wrapping in `app.rs`. Keep the two in step: every
/// route of a wrapped router is covered by a prefix, and no other `bare`
/// route or JSON route is.
///
/// Limits of this check: it reads source text, so it only sees routes
/// written as a string literal in a `.route("…")` call of the module's own
/// file, and JSON routes that carry a `#[utoipa::path]`. A reader route
/// registered another way (a path constant, a sub-router merged from a
/// different file) is not seen; add its path here by hand when adding one.
#[test]
fn reader_route_prefixes_match_reader_surface() {
    use server::auth::extractor::is_reader_route;

    let app_rs = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app.rs"),
    )
    .unwrap();
    let start = app_rs.find("let bare = ").expect("bare group");
    let end = start + app_rs[start..].find("let api = ").expect("api group");
    let bare = &app_rs[start..end];

    // Every `api::X::routes()` / `auth::X::routes()` in the bare group,
    // split by whether `reader_surface(` wraps it.
    let mut reader = Vec::new();
    let mut other = Vec::new();
    for (i, _) in bare.match_indices("::routes()") {
        let head = &bare[..i];
        let from = head
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
            .map_or(0, |p| p + 1);
        let module = &head[from..];
        if !(module.starts_with("api::") || module.starts_with("auth::")) {
            continue;
        }
        let before = head[..from].trim_end();
        if before.ends_with("reader_surface(") {
            reader.push(module.to_owned());
        } else {
            other.push(module.to_owned());
        }
    }
    assert!(
        reader.len() >= 6,
        "reader_surface routers found: {reader:?}"
    );

    for module in &reader {
        let routes = literals_after(&module_source(module), ".route(");
        assert!(!routes.is_empty(), "{module}: no routes found");
        for path in routes {
            assert!(
                is_reader_route(&path),
                "{module} route {path} is under reader_surface but not in READER_ROUTE_PREFIXES"
            );
        }
    }
    for module in &other {
        let src = module_source(module);
        for path in literals_after(&src, ".route(")
            .into_iter()
            .chain(literals_after(&src, "path = "))
        {
            assert!(
                !is_reader_route(&path),
                "{module} route {path} is not a reader route but matches READER_ROUTE_PREFIXES"
            );
        }
    }
    // The JSON surface, with its real `/api` prefix.
    for path in server::app::openapi_spec().paths.paths.keys() {
        assert!(
            !is_reader_route(path),
            "{path} matches READER_ROUTE_PREFIXES but is not a reader route"
        );
    }
}

// ───────────── lookup digest, legacy rows, lockout, hashing gate ─────────────
//
// A token is found by its lookup digest (SHA-256) and argon2-checked against
// that one row; rows written before the digest column existed are tried, a
// bounded number per request, only when no row has the digest. The gate's
// verification counter (`AppPasswordGate::verifications`) counts argon2 work.

use server::auth::app_password::{self as ap, MAX_LEGACY_CANDIDATES};

const JUNK: &str = "app_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn verifications(app: &TestApp) -> u64 {
    app.state().app_password_gate.verifications()
}

/// GET the OPDS root with `Bearer <token>` from client IP `ip`.
async fn opds_from(app: &TestApp, token: &str, ip: [u8; 4]) -> Response<Body> {
    let mut req = Request::builder()
        .uri("/opds/v1")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            ip, 40000,
        ))));
    app.router.clone().oneshot(req).await.unwrap()
}

/// A distinct well-formed token that matches nothing.
fn junk(i: usize) -> String {
    format!("app_{:a>52}", format!("junk{i}"))
}

/// GET `uri` with `Bearer <token>` from client IP `ip`.
async fn get_from(app: &TestApp, uri: &str, token: &str, ip: [u8; 4]) -> Response<Body> {
    let mut req = Request::builder()
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            ip, 40000,
        ))));
    app.router.clone().oneshot(req).await.unwrap()
}

/// Current value of the app-password failure counter for `ip`.
async fn app_fail_count(app: &TestApp, ip: [u8; 4]) -> i64 {
    use redis::AsyncCommands;
    let key =
        server::auth::failed_auth::IpCounter::AppPassword.counter_key(std::net::IpAddr::from(ip));
    let mut redis = app.state().jobs.redis.clone();
    let n: Option<i64> = redis.get(key).await.unwrap();
    n.unwrap_or(0)
}

/// Push `ip` over the threshold on one counter, the way failures would.
async fn lock_out(app: &TestApp, which: server::auth::failed_auth::IpCounter, ip: [u8; 4]) {
    for _ in 0..server::auth::failed_auth::FAIL_THRESHOLD {
        server::auth::failed_auth::record_failure_on(
            app.state().jobs.redis.clone(),
            which,
            std::net::IpAddr::from(ip),
        )
        .await;
    }
}

async fn set_last_used_now(app: &TestApp, id: uuid::Uuid) {
    use sea_orm::sea_query::Expr;
    entity::app_password::Entity::update_many()
        .col_expr(
            entity::app_password::Column::LastUsedAt,
            Expr::value(chrono::Utc::now().fixed_offset()),
        )
        .filter(entity::app_password::Column::Id.eq(id))
        .exec(&app.state().db)
        .await
        .unwrap();
}

async fn user_id(app: &TestApp, email: &str) -> uuid::Uuid {
    entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq(email))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .id
}

/// Issue a token straight through `app_password::issue` (no per-user cap).
async fn issue_direct(app: &TestApp, owner: uuid::Uuid) -> (uuid::Uuid, String) {
    let state = app.state();
    ap::issue(
        &state.db,
        owner,
        "reader",
        ap::SCOPE_READ,
        state.secrets.pepper.as_ref(),
        server::auth::password::HashCost::TEST,
    )
    .await
    .unwrap()
}

/// Turn a row into one written before the digest column existed.
async fn make_legacy(app: &TestApp, id: uuid::Uuid) {
    use sea_orm::sea_query::Expr;
    entity::app_password::Entity::update_many()
        .col_expr(
            entity::app_password::Column::LookupDigest,
            Expr::value(Option::<Vec<u8>>::None),
        )
        .filter(entity::app_password::Column::Id.eq(id))
        .exec(&app.state().db)
        .await
        .unwrap();
}

async fn stored_digest(app: &TestApp, id: uuid::Uuid) -> Option<Vec<u8>> {
    entity::app_password::Entity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .lookup_digest
}

async fn owner(app: &TestApp, email: &str) -> (Auth, uuid::Uuid) {
    let reg = register(app, email).await;
    assert_eq!(reg.status(), StatusCode::CREATED);
    (Auth::from_response(&reg), user_id(app, email).await)
}

#[tokio::test]
async fn new_token_verifies_through_digest_with_one_argon2() {
    let app = TestApp::spawn().await;
    let (auth, _) = owner(&app, "digest@example.com").await;
    // Other active tokens must not add work: only the digest's row is hashed.
    let _ = mint(&app, &auth).await;
    let _ = mint(&app, &auth).await;
    let token = mint(&app, &auth).await;

    let row = entity::app_password::Entity::find()
        .filter(entity::app_password::Column::LookupDigest.eq(ap::lookup_digest(&token).to_vec()))
        .one(&app.state().db)
        .await
        .unwrap();
    assert!(row.is_some(), "a new token stores its lookup digest");

    assert_eq!(
        opds_with_bearer(&app, &token).await.status(),
        StatusCode::OK
    );
    assert_eq!(verifications(&app), 1);
    // Repeat use is served by the verified-token cache: no more hashing.
    assert_eq!(
        opds_with_bearer(&app, &token).await.status(),
        StatusCode::OK
    );
    assert_eq!(verifications(&app), 1);
    // Behind the cache, the digest path again costs exactly one.
    app.state().app_password_cache.clear();
    assert_eq!(
        opds_with_bearer(&app, &token).await.status(),
        StatusCode::OK
    );
    assert_eq!(verifications(&app), 2);
}

#[tokio::test]
async fn junk_token_with_only_digest_rows_runs_no_argon2() {
    let app = TestApp::spawn().await;
    let (auth, _) = owner(&app, "junk@example.com").await;
    let _ = mint(&app, &auth).await;
    let _ = mint(&app, &auth).await;

    for token in [JUNK, "app_", "app_short"] {
        let resp = opds_with_bearer(&app, token).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{token}");
        assert!(
            resp.headers().contains_key(header::WWW_AUTHENTICATE),
            "OPDS 401 keeps its Basic challenge"
        );
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["error"]["code"], "auth.invalid");
    }
    assert_eq!(verifications(&app), 0);
}

#[tokio::test]
async fn legacy_row_verifies_then_gets_its_digest() {
    let app = TestApp::spawn().await;
    let (_, uid) = owner(&app, "legacy@example.com").await;
    // Oldest first, so the target is the last legacy candidate tried.
    let (target_id, target) = issue_direct(&app, uid).await;
    make_legacy(&app, target_id).await;
    for _ in 0..3 {
        let (id, _) = issue_direct(&app, uid).await;
        make_legacy(&app, id).await;
    }

    assert_eq!(
        opds_with_bearer(&app, &target).await.status(),
        StatusCode::OK
    );
    assert_eq!(verifications(&app), 4, "legacy scan tried all four rows");
    assert_eq!(
        stored_digest(&app, target_id).await,
        Some(ap::lookup_digest(&target).to_vec()),
        "a verified legacy row is given its digest"
    );

    // Next time (cache cleared) it takes the digest path: one verification.
    app.state().app_password_cache.clear();
    assert_eq!(
        opds_with_bearer(&app, &target).await.status(),
        StatusCode::OK
    );
    assert_eq!(verifications(&app), 5);
}

#[tokio::test]
async fn legacy_scan_is_bounded() {
    let app = TestApp::spawn().await;
    let (_, uid) = owner(&app, "bounded@example.com").await;
    let extra = 3;
    let mut ids = Vec::new();
    for _ in 0..(MAX_LEGACY_CANDIDATES as usize + extra) {
        let (id, token) = issue_direct(&app, uid).await;
        make_legacy(&app, id).await;
        ids.push((id, token));
    }

    let resp = opds_with_bearer(&app, JUNK).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(verifications(&app), MAX_LEGACY_CANDIDATES);

    // The trade-off: the oldest never-used legacy token sits outside the
    // window and is not reached; a recently used one is.
    let before = verifications(&app);
    let (_, oldest) = &ids[0];
    assert_eq!(
        opds_with_bearer(&app, oldest).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(verifications(&app) - before, MAX_LEGACY_CANDIDATES);

    // Another old one, just used elsewhere, sorts first and is reached with
    // a single verification.
    let (recent_id, recent) = &ids[1];
    set_last_used_now(&app, *recent_id).await;
    let before = verifications(&app);
    assert_eq!(
        opds_with_bearer(&app, recent).await.status(),
        StatusCode::OK
    );
    assert_eq!(verifications(&app) - before, 1);
}

#[tokio::test]
async fn lockout_refuses_before_any_verification_per_ip() {
    let app = TestApp::spawn().await;
    let (auth, uid) = owner(&app, "lockout@example.com").await;
    let token = mint(&app, &auth).await;
    // One legacy row, so every junk attempt costs one verification while
    // the IP is not locked out.
    let (legacy_id, _) = issue_direct(&app, uid).await;
    make_legacy(&app, legacy_id).await;

    let a = [198, 51, 100, 7];
    let threshold = server::auth::failed_auth::FAIL_THRESHOLD as u64;
    for i in 0..threshold {
        let resp = opds_from(&app, &junk(i as usize), a).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    assert_eq!(verifications(&app), threshold);
    assert_eq!(app_fail_count(&app, a).await, threshold as i64);

    // Locked out: refused before any lookup or hashing, and not counted.
    let resp = opds_from(&app, &junk(99), a).await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(resp.headers().contains_key(header::RETRY_AFTER));
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.locked_out");
    // A valid but not yet verified token from the same IP is refused too.
    assert_eq!(
        opds_from(&app, &token, a).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        verifications(&app),
        threshold,
        "no hashing while locked out"
    );
    assert_eq!(app_fail_count(&app, a).await, threshold as i64);

    // Another IP is unaffected.
    assert_eq!(
        opds_from(&app, &token, [203, 0, 113, 9]).await.status(),
        StatusCode::OK
    );
    // Once verified (cached), the token costs no hashing and is served even
    // from the locked-out IP, the way a session cookie outlives a login
    // lockout.
    assert_eq!(opds_from(&app, &token, a).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn lockout_off_when_failed_auth_counters_are_disabled() {
    let app = TestApp::spawn().await;
    let state = app.state();
    let mut cfg = (*state.cfg()).clone();
    cfg.rate_limit_enabled = false;
    state.replace_cfg(cfg);

    let ip = [198, 51, 100, 8];
    for i in 0..(server::auth::failed_auth::FAIL_THRESHOLD as usize + 2) {
        let resp = opds_from(&app, &junk(i), ip).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    assert_eq!(app_fail_count(&app, ip).await, 0);
}

#[tokio::test]
async fn revocation_applies_on_the_next_request_on_every_path() {
    let app = TestApp::spawn().await;
    let (auth, _) = owner(&app, "revoked2@example.com").await;
    let revoke = |id: String| {
        let app = &app;
        let auth = &auth;
        async move {
            let (status, _) = send(
                app,
                Method::DELETE,
                &format!("/api/me/app-passwords/{id}"),
                Cred::Cookie(auth),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
        }
    };

    // Digest path: never used, revoked, refused without hashing.
    let body = body_json(create_password(&app, &auth, "a").await.into_body()).await;
    let token = body["plaintext"].as_str().unwrap().to_owned();
    revoke(body["id"].as_str().unwrap().to_owned()).await;
    assert_eq!(
        opds_with_bearer(&app, &token).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(verifications(&app), 0);

    // Cached path: used (cached), revoked, refused on the next request.
    let body = body_json(create_password(&app, &auth, "b").await.into_body()).await;
    let token = body["plaintext"].as_str().unwrap().to_owned();
    assert_eq!(
        opds_with_bearer(&app, &token).await.status(),
        StatusCode::OK
    );
    revoke(body["id"].as_str().unwrap().to_owned()).await;
    assert_eq!(
        opds_with_bearer(&app, &token).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn disabled_user_token_is_refused_on_digest_path() {
    let app = TestApp::spawn().await;
    let (auth, uid) = owner(&app, "disabled@example.com").await;
    let token = mint(&app, &auth).await;
    let mut am: entity::user::ActiveModel = entity::user::Entity::find_by_id(uid)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.state = sea_orm::Set(entity::user::UserState::Disabled);
    sea_orm::ActiveModelTrait::update(am, &app.state().db)
        .await
        .unwrap();

    let (status, body) = send(&app, Method::GET, "/opds/v1", Cred::Basic(&token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "auth.disabled");
}

#[tokio::test]
async fn two_rows_cannot_share_a_digest() {
    let app = TestApp::spawn().await;
    let (_, uid) = owner(&app, "unique@example.com").await;
    let (id, _) = issue_direct(&app, uid).await;
    let original = entity::app_password::Entity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let dup = entity::app_password::ActiveModel {
        id: sea_orm::Set(uuid::Uuid::now_v7()),
        user_id: sea_orm::Set(uid),
        label: sea_orm::Set("dup".into()),
        hash: sea_orm::Set(original.hash.clone()),
        last_used_at: sea_orm::Set(None),
        created_at: sea_orm::Set(chrono::Utc::now().fixed_offset()),
        revoked_at: sea_orm::Set(None),
        scope: sea_orm::Set("read".into()),
        lookup_digest: sea_orm::Set(original.lookup_digest.clone()),
    };
    let res = sea_orm::ActiveModelTrait::insert(dup, &app.state().db).await;
    assert!(res.is_err(), "the unique index refuses a duplicate digest");
}

#[tokio::test]
async fn busy_gate_answers_503_then_recovers() {
    let app = TestApp::spawn().await;
    let (auth, _) = owner(&app, "busy@example.com").await;
    let token = mint(&app, &auth).await;
    let gate = app.state().app_password_gate.clone();
    gate.set_permit_wait(std::time::Duration::from_millis(100));

    let held = gate.hold_all_permits().await;
    let (status, body) = send(&app, Method::GET, "/opds/v1", Cred::Bearer(&token), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "auth.busy");
    let resp = opds_with_bearer(&app, &token).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(resp.headers().contains_key(header::RETRY_AFTER));
    assert_eq!(verifications(&app), 0);

    drop(held);
    assert_eq!(
        opds_with_bearer(&app, &token).await.status(),
        StatusCode::OK
    );
    assert_eq!(verifications(&app), 1);
}

/// Concurrent junk while legacy rows remain: every request is charged before
/// it hashes, so even requests that end busy lock the address.
#[tokio::test]
async fn concurrent_busy_misses_still_lock_the_address() {
    let app = TestApp::spawn().await;
    let (_, uid) = owner(&app, "concurrent@example.com").await;
    let (legacy_id, _) = issue_direct(&app, uid).await;
    make_legacy(&app, legacy_id).await;
    let gate = app.state().app_password_gate.clone();
    gate.set_permit_wait(std::time::Duration::from_millis(50));
    let ip = [198, 51, 100, 20];

    let held = gate.hold_all_permits().await;
    let n = server::auth::failed_auth::FAIL_THRESHOLD as usize + 5;
    let reqs = (0..n).map(|i| {
        let token = junk(i);
        let app = &app;
        async move { opds_from(app, &token, ip).await.status() }
    });
    let statuses = futures::future::join_all(reqs).await;
    assert!(
        statuses
            .iter()
            .all(|s| *s == StatusCode::SERVICE_UNAVAILABLE),
        "{statuses:?}"
    );
    drop(held);

    let resp = opds_from(&app, &junk(500), ip).await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}

/// A request dropped part-way through the legacy scan has been counted.
#[tokio::test]
async fn dropped_request_mid_scan_is_counted() {
    let app = TestApp::spawn().await;
    let (_, uid) = owner(&app, "dropped@example.com").await;
    let (legacy_id, _) = issue_direct(&app, uid).await;
    make_legacy(&app, legacy_id).await;
    let gate = app.state().app_password_gate.clone();
    gate.set_permit_wait(std::time::Duration::from_secs(30));
    let ip = [198, 51, 100, 21];

    let held = gate.hold_all_permits().await;
    let router = app.router.clone();
    let task = tokio::spawn(async move {
        let mut req = Request::builder()
            .uri("/opds/v1")
            .header(header::AUTHORIZATION, format!("Bearer {}", junk(1)))
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                ip, 40000,
            ))));
        router.oneshot(req).await.unwrap().status()
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app_fail_count(&app, ip).await == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "miss not charged while the request waits to hash"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    task.abort();
    let _ = task.await;
    drop(held);
    assert_eq!(app_fail_count(&app, ip).await, 1);
    assert_eq!(verifications(&app), 0);
}

/// Busy at the legacy loop answers 503 and leaves exactly one counted miss.
#[tokio::test]
async fn busy_in_legacy_loop_is_counted_once() {
    let app = TestApp::spawn().await;
    let (_, uid) = owner(&app, "busyloop@example.com").await;
    for _ in 0..2 {
        let (id, _) = issue_direct(&app, uid).await;
        make_legacy(&app, id).await;
    }
    let gate = app.state().app_password_gate.clone();
    gate.set_permit_wait(std::time::Duration::from_millis(50));
    let ip = [198, 51, 100, 22];
    let held = gate.hold_all_permits().await;
    let resp = opds_from(&app, JUNK, ip).await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(resp.headers().contains_key(header::RETRY_AFTER));
    drop(held);
    assert_eq!(app_fail_count(&app, ip).await, 1);
}

async fn login_from(app: &TestApp, email: &str, ip: [u8; 4]) -> StatusCode {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/auth/local/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(
            r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#
        )))
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            ip, 40000,
        ))));
    app.router.clone().oneshot(req).await.unwrap().status()
}

/// Sign-in and app passwords lock out on separate counters.
#[tokio::test]
async fn sign_in_and_app_password_lockouts_are_separate() {
    use server::auth::failed_auth::IpCounter;
    let app = TestApp::spawn().await;
    let (auth, _) = owner(&app, "separate@example.com").await;
    let token = mint(&app, &auth).await;

    // A sign-in lockout does not refuse a valid, uncached app password.
    let a = [198, 51, 100, 30];
    lock_out(&app, IpCounter::SignIn, a).await;
    assert_eq!(
        login_from(&app, "separate@example.com", a).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(opds_from(&app, &token, a).await.status(), StatusCode::OK);

    // An app-password lockout, reached by real misses, does not refuse
    // sign-in with correct credentials.
    let b = [198, 51, 100, 31];
    for i in 0..server::auth::failed_auth::FAIL_THRESHOLD as usize {
        assert_eq!(
            opds_from(&app, &junk(i), b).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        opds_from(&app, &junk(77), b).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        login_from(&app, "separate@example.com", b).await,
        StatusCode::OK
    );
}

/// The same refused token, presented over and over, is charged once and
/// hashed once; a different token is still charged.
#[tokio::test]
async fn repeated_refused_token_is_charged_once() {
    let app = TestApp::spawn().await;
    let (_, uid) = owner(&app, "repeat@example.com").await;
    let (legacy_id, _) = issue_direct(&app, uid).await;
    make_legacy(&app, legacy_id).await;
    let ip = [198, 51, 100, 40];

    for i in 0..15 {
        let resp = opds_from(&app, JUNK, ip).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "attempt {i}");
        assert!(resp.headers().contains_key(header::WWW_AUTHENTICATE));
    }
    assert_eq!(verifications(&app), 1, "only the first presentation hashed");
    assert_eq!(app_fail_count(&app, ip).await, 1);

    assert_eq!(
        opds_from(&app, &junk(3), ip).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(app_fail_count(&app, ip).await, 2);
}

/// A legacy row revoked before it got a digest: refused, counted once, then
/// answered from the refusal cache.
#[tokio::test]
async fn revoked_legacy_row_is_counted_once() {
    let app = TestApp::spawn().await;
    let (_, uid) = owner(&app, "revlegacy@example.com").await;
    let (id, token) = issue_direct(&app, uid).await;
    make_legacy(&app, id).await;
    {
        use sea_orm::sea_query::Expr;
        entity::app_password::Entity::update_many()
            .col_expr(
                entity::app_password::Column::RevokedAt,
                Expr::value(chrono::Utc::now().fixed_offset()),
            )
            .filter(entity::app_password::Column::Id.eq(id))
            .exec(&app.state().db)
            .await
            .unwrap();
    }
    let ip = [198, 51, 100, 41];
    for _ in 0..5 {
        assert_eq!(
            opds_from(&app, &token, ip).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(app_fail_count(&app, ip).await, 1);
    assert_eq!(verifications(&app), 0);
}

/// A cached token whose user was deleted is refused on the next request.
#[tokio::test]
async fn deleted_users_cached_token_is_refused() {
    let app = TestApp::spawn().await;
    let (auth, uid) = owner(&app, "gone@example.com").await;
    let token = mint(&app, &auth).await;
    assert_eq!(
        opds_with_bearer(&app, &token).await.status(),
        StatusCode::OK
    );
    entity::user::Entity::delete_by_id(uid)
        .exec(&app.state().db)
        .await
        .unwrap();
    assert_eq!(
        opds_with_bearer(&app, &token).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

/// Page bytes and the Komga shim go through the same path: a junk token
/// costs no hashing, and a locked address gets the 429 there too.
#[tokio::test]
async fn other_reader_routes_use_the_same_path() {
    use server::auth::failed_auth::IpCounter;
    let app = TestApp::spawn().await;
    let (auth, _) = owner(&app, "routes@example.com").await;
    let _ = mint(&app, &auth).await;
    let issue = uuid::Uuid::now_v7();
    let page = format!("/issues/{issue}/pages/0");
    let book = format!("/api/v1/books/{issue}");
    let ip = [198, 51, 100, 50];

    for (i, uri) in [&page, &book].into_iter().enumerate() {
        let resp = get_from(&app, uri, &junk(i), ip).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
    assert_eq!(verifications(&app), 0);
    assert_eq!(app_fail_count(&app, ip).await, 2);

    let locked = [198, 51, 100, 51];
    lock_out(&app, IpCounter::AppPassword, locked).await;
    for (i, uri) in [&page, &book].into_iter().enumerate() {
        let resp = get_from(&app, uri, &junk(10 + i), locked).await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS, "{uri}");
    }
}
