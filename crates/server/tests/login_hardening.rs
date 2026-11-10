//! Sign-in hardening.
//!
//! - The login handler verifies the password before it reports the account
//!   state: a wrong password is the generic 401 for every account, and the
//!   disabled / unverified codes only answer a correct password. The
//!   failed-auth lockout counts every one of those attempts as before.
//! - The CSRF-exempt sign-in POSTs refuse a request a browser marks as
//!   cross-site (`Sec-Fetch-Site`) or whose `Origin` is neither the public
//!   origin nor the request's own `Host`.
//!
//! Timing is not asserted (flaky); the code paths are.

mod common;

use std::net::SocketAddr;

use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use sea_orm::ConnectionTrait;
use tower::ServiceExt;

const PW: &str = "correctly-horse-battery";
const WRONG: &str = "not-the-password-at-all";

/// The JSON body, or `null` for an empty one (204, or a 202 without body).
async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

/// POST JSON from a distinct client address per call (`n`), so neither the
/// per-IP token bucket nor the IP lockout axis interferes: only the email
/// axis accumulates across calls.
async fn post_from(
    app: &TestApp,
    n: u8,
    uri: &str,
    body: serde_json::Value,
    extra: &[(&str, &str)],
) -> Response<Body> {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    let mut req = req.body(Body::from(body.to_string())).unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([198, 51, 100, n], 40_000))));
    app.router.clone().oneshot(req).await.unwrap()
}

async fn login(app: &TestApp, n: u8, email: &str, password: &str) -> Response<Body> {
    post_from(
        app,
        n,
        "/auth/local/login",
        serde_json::json!({ "email": email, "password": password }),
        &[],
    )
    .await
}

async fn status_and_code(resp: Response<Body>) -> (StatusCode, String) {
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    (
        status,
        body["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    )
}

/// An admin (first user) plus `email` registered and moved to `state`.
async fn account_in_state(app: &TestApp, email: &str, state: &str) {
    for (who, n) in [("admin@example.com", 2u8), (email, 3u8)] {
        let resp = post_from(
            app,
            n,
            "/auth/local/register",
            serde_json::json!({ "email": who, "password": PW }),
            &[],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED, "register {who}");
    }
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    db.execute_unprepared(&format!(
        "UPDATE users SET state = '{state}' WHERE email = '{email}'"
    ))
    .await
    .unwrap();
}

#[tokio::test]
async fn disabled_account_reports_its_state_only_to_the_right_password() {
    let app = TestApp::spawn().await;
    account_in_state(&app, "off@example.com", "disabled").await;

    let (status, code) = status_and_code(login(&app, 10, "off@example.com", WRONG).await).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::UNAUTHORIZED, "auth.invalid")
    );

    let (status, code) = status_and_code(login(&app, 11, "off@example.com", PW).await).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::FORBIDDEN, "auth.disabled")
    );
}

#[tokio::test]
async fn pending_account_reports_its_state_only_to_the_right_password() {
    let app = TestApp::spawn().await;
    account_in_state(&app, "wait@example.com", "pending_verification").await;

    let (status, code) = status_and_code(login(&app, 10, "wait@example.com", WRONG).await).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::UNAUTHORIZED, "auth.invalid")
    );

    let (status, code) = status_and_code(login(&app, 11, "wait@example.com", PW).await).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::FORBIDDEN, "auth.email_unverified")
    );
}

#[tokio::test]
async fn unknown_address_is_the_generic_401() {
    let app = TestApp::spawn().await;
    let (status, code) = status_and_code(login(&app, 10, "ghost@example.com", PW).await).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::UNAUTHORIZED, "auth.invalid")
    );
}

/// Ten attempts on one address (each from its own IP) lock the address;
/// the eleventh is refused with `auth.locked_out` before any verify.
async fn assert_locks_after_ten(app: &TestApp, email: &str, password: &str) {
    for n in 0..10u8 {
        let resp = login(app, 20 + n, email, password).await;
        assert_ne!(
            resp.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "attempt {n} for {email} locked out too early"
        );
        assert_ne!(resp.status(), StatusCode::OK, "attempt {n} signed in");
    }
    let (status, code) = status_and_code(login(app, 40, email, password).await).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::TOO_MANY_REQUESTS, "auth.locked_out"),
        "{email}"
    );
}

#[tokio::test]
async fn lockout_counts_disabled_account_attempts_with_either_password() {
    let app = TestApp::spawn().await;
    account_in_state(&app, "off@example.com", "disabled").await;
    assert_locks_after_ten(&app, "off@example.com", WRONG).await;

    let app = TestApp::spawn().await;
    account_in_state(&app, "off@example.com", "disabled").await;
    assert_locks_after_ten(&app, "off@example.com", PW).await;
}

#[tokio::test]
async fn lockout_counts_pending_account_attempts_with_either_password() {
    let app = TestApp::spawn().await;
    account_in_state(&app, "wait@example.com", "pending_verification").await;
    assert_locks_after_ten(&app, "wait@example.com", WRONG).await;

    let app = TestApp::spawn().await;
    account_in_state(&app, "wait@example.com", "pending_verification").await;
    assert_locks_after_ten(&app, "wait@example.com", PW).await;
}

#[tokio::test]
async fn lockout_counts_unknown_address_attempts() {
    let app = TestApp::spawn().await;
    assert_locks_after_ten(&app, "ghost@example.com", WRONG).await;
}

/// The no-JavaScript form path redirects with the same codes.
#[tokio::test]
async fn form_login_reports_state_only_to_the_right_password() {
    let app = TestApp::spawn().await;
    account_in_state(&app, "off@example.com", "disabled").await;
    let form = |password: &str| {
        Request::builder()
            .method(Method::POST)
            .uri("/auth/local/login")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(format!(
                "email=off%40example.com&password={password}"
            )))
            .unwrap()
    };
    let location = |resp: &Response<Body>| {
        resp.headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };

    let resp = app.router.clone().oneshot(form(WRONG)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = location(&resp);
    assert!(loc.contains("error=auth.invalid"), "{loc}");

    let resp = app.router.clone().oneshot(form(PW)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = location(&resp);
    assert!(loc.contains("error=auth.disabled"), "{loc}");
}

// ───────── cross-site guard on the CSRF-exempt sign-in POSTs ─────────

const PUBLIC_ORIGIN: &str = "http://localhost:8080"; // TestApp's public_url

fn route_body(route: &str) -> serde_json::Value {
    match route {
        "/auth/local/login" => serde_json::json!({ "email": "x@example.com", "password": WRONG }),
        "/auth/local/register" => {
            serde_json::json!({ "email": "new@example.com", "password": PW })
        }
        _ => serde_json::json!({ "email": "x@example.com" }),
    }
}

const GUARDED: [&str; 3] = [
    "/auth/local/login",
    "/auth/local/register",
    "/auth/local/request-password-reset",
];

#[tokio::test]
async fn cross_site_sign_in_posts_are_refused() {
    let refused: [&[(&str, &str)]; 4] = [
        &[("sec-fetch-site", "cross-site")],
        &[("sec-fetch-site", "same-site")],
        &[
            ("origin", "https://evil.example"),
            ("host", "localhost:8080"),
        ],
        &[("origin", "null"), ("host", "localhost:8080")],
    ];
    for route in GUARDED {
        for (i, headers) in refused.iter().enumerate() {
            let app = TestApp::spawn().await;
            let resp = post_from(&app, 50 + i as u8, route, route_body(route), headers).await;
            let (status, code) = status_and_code(resp).await;
            assert_eq!(
                (status, code.as_str()),
                (StatusCode::FORBIDDEN, "auth.csrf"),
                "{route} {headers:?}"
            );
        }
    }
}

#[tokio::test]
async fn same_origin_and_headerless_sign_in_posts_proceed() {
    let allowed: [&[(&str, &str)]; 5] = [
        &[],
        &[("origin", PUBLIC_ORIGIN), ("sec-fetch-site", "same-origin")],
        // A LAN address that is not the public URL: Origin equals Host.
        &[
            ("origin", "http://192.168.1.20:8080"),
            ("host", "192.168.1.20:8080"),
            ("sec-fetch-site", "same-origin"),
        ],
        &[("origin", PUBLIC_ORIGIN), ("host", "app:8080")],
        &[("sec-fetch-site", "none")],
    ];
    for route in GUARDED {
        for (i, headers) in allowed.iter().enumerate() {
            let app = TestApp::spawn().await;
            let resp = post_from(&app, 60 + i as u8, route, route_body(route), headers).await;
            let status = resp.status();
            let (_, code) = status_and_code(resp).await;
            assert_ne!(code, "auth.csrf", "{route} {headers:?}");
            assert!(
                matches!(
                    status,
                    StatusCode::OK
                        | StatusCode::CREATED
                        | StatusCode::ACCEPTED
                        | StatusCode::UNAUTHORIZED
                        | StatusCode::NO_CONTENT
                ),
                "{route} {headers:?}: {status}"
            );
        }
    }
}

/// The OIDC callback is a cross-site redirect by design (its state cookie
/// gates it) and keeps no cross-site guard.
#[tokio::test]
async fn oidc_callback_is_not_cross_site_guarded() {
    let app = TestApp::spawn().await;
    let mut req = Request::builder()
        .method(Method::GET)
        .uri("/auth/oidc/callback?code=x&state=y")
        .header("sec-fetch-site", "cross-site")
        .header("origin", "https://idp.example")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([198, 51, 100, 99], 40_000))));
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("auth.csrf"), "{text}");
}
