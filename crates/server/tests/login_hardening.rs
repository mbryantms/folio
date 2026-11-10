//! Sign-in hardening.
//!
//! - The login handler verifies the password before it reports the account
//!   state: a wrong password is the generic 401 for every account, and the
//!   disabled / unverified codes only answer a correct password. The
//!   failed-auth lockout counts every one of those attempts as before.
//! - The CSRF-exempt sign-in POSTs refuse a request a browser marks as
//!   cross-site (`Sec-Fetch-Site`, which decides alone when present) or,
//!   without it, whose `Origin` is neither the public origin, nor the
//!   request's own `Host`, nor a trusted proxy's `X-Forwarded-Host`.
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
const FORM: &str = "application/x-www-form-urlencoded";
const JSON: &str = "application/json";

/// POST `body` as `content_type` from client address `n`, with `extra`.
async fn post_raw(
    app: &TestApp,
    n: u8,
    uri: &str,
    content_type: &str,
    body: &str,
    extra: &[(&str, &str)],
) -> Response<Body> {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, content_type);
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    let mut req = req.body(Body::from(body.to_owned())).unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([198, 51, 100, n], 40_000))));
    app.router.clone().oneshot(req).await.unwrap()
}

fn location(resp: &Response<Body>) -> String {
    resp.headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

/// Every CSRF-exempt POST the guard covers, with a JSON body its handler
/// accepts.
const GUARDED: [(&str, &str); 5] = [
    (
        "/auth/local/login",
        r#"{"email":"x@example.com","password":"not-the-password-at-all"}"#,
    ),
    (
        "/auth/local/register",
        r#"{"email":"new@example.com","password":"correctly-horse-battery"}"#,
    ),
    (
        "/auth/local/request-password-reset",
        r#"{"email":"x@example.com"}"#,
    ),
    (
        "/auth/local/reset-password",
        r#"{"token":"bogus","new_password":"correctly-horse-battery"}"#,
    ),
    (
        "/auth/local/resend-verification",
        r#"{"email":"x@example.com"}"#,
    ),
];

#[tokio::test]
async fn cross_site_sign_in_posts_are_refused_on_every_guarded_route() {
    let refused: [&[(&str, &str)]; 5] = [
        &[("sec-fetch-site", "cross-site")],
        &[("sec-fetch-site", "same-site")],
        &[("sec-fetch-site", "cross-site"), ("origin", PUBLIC_ORIGIN)],
        &[
            ("origin", "https://evil.example"),
            ("host", "localhost:8080"),
        ],
        &[("origin", "null"), ("host", "localhost:8080")],
    ];
    for (route, body) in GUARDED {
        let app = TestApp::spawn().await;
        for (i, headers) in refused.iter().enumerate() {
            let resp = post_raw(&app, 50 + i as u8, route, JSON, body, headers).await;
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
async fn same_origin_and_headerless_sign_in_posts_proceed_on_every_guarded_route() {
    let allowed: [&[(&str, &str)]; 6] = [
        &[],
        &[("sec-fetch-site", "same-origin"), ("origin", "null")],
        &[("sec-fetch-site", "none"), ("origin", "null")],
        &[("origin", PUBLIC_ORIGIN)],
        // A LAN address that is not the public URL: Origin equals Host.
        &[
            ("origin", "http://192.168.1.20:8080"),
            ("host", "192.168.1.20:8080"),
        ],
        &[
            ("origin", "https://lan.example"),
            ("host", "lan.example:443"),
        ],
    ];
    for (route, body) in GUARDED {
        for (i, headers) in allowed.iter().enumerate() {
            let app = TestApp::spawn().await;
            let resp = post_raw(&app, 60 + i as u8, route, JSON, body, headers).await;
            let status = resp.status();
            let (_, code) = status_and_code(resp).await;
            assert_ne!(code, "auth.csrf", "{route} {headers:?}");
            assert!(
                matches!(
                    status,
                    StatusCode::OK
                        | StatusCode::CREATED
                        | StatusCode::ACCEPTED
                        | StatusCode::NO_CONTENT
                        | StatusCode::BAD_REQUEST
                        | StatusCode::UNAUTHORIZED
                ),
                "{route} {headers:?}: {status}"
            );
        }
    }
}

/// The no-JavaScript forms: a same-origin form post under a strict
/// referrer policy carries `Sec-Fetch-Site: same-origin` with `Origin:
/// null`. It reaches the handler, which answers the form its usual way.
#[tokio::test]
async fn same_origin_form_posts_with_a_null_origin_reach_the_handler() {
    let headers: &[(&str, &str)] = &[("sec-fetch-site", "same-origin"), ("origin", "null")];
    let app = TestApp::spawn().await;

    let resp = post_raw(
        &app,
        70,
        "/auth/local/login",
        FORM,
        "email=x%40example.com&password=not-the-password-at-all",
        headers,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(
        location(&resp).starts_with("/sign-in?error=auth.invalid"),
        "{}",
        location(&resp)
    );

    let resp = post_raw(
        &app,
        71,
        "/auth/local/register",
        FORM,
        "email=first%40example.com&password=correctly-horse-battery",
        headers,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(!location(&resp).contains("error="), "{}", location(&resp));

    let resp = post_raw(
        &app,
        72,
        "/auth/local/request-password-reset",
        FORM,
        "email=x%40example.com",
        headers,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/forgot-password?sent=1");

    let resp = post_raw(
        &app,
        73,
        "/auth/local/reset-password",
        FORM,
        "token=bogus&new_password=correctly-horse-battery",
        headers,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(
        location(&resp).starts_with("/forgot-password?error=auth.token_invalid"),
        "{}",
        location(&resp)
    );
}

/// Without `Sec-Fetch-Site`, `Origin` decides; default ports are equivalent.
#[tokio::test]
async fn origin_rule_without_sec_fetch_site() {
    let app = TestApp::spawn().await;
    let cases: [(&[(&str, &str)], bool); 6] = [
        (&[("origin", PUBLIC_ORIGIN)], true),
        (
            &[
                ("origin", "http://192.168.1.20:8080"),
                ("host", "192.168.1.20:8080"),
            ],
            true,
        ),
        (
            &[
                ("origin", "https://lan.example"),
                ("host", "lan.example:443"),
            ],
            true,
        ),
        (
            &[("origin", "http://lan.example"), ("host", "lan.example:80")],
            true,
        ),
        (&[("origin", "null"), ("host", "localhost:8080")], false),
        (
            &[
                ("origin", "http://lan.example:8080"),
                ("host", "lan.example:80"),
            ],
            false,
        ),
    ];
    for (i, (headers, passes)) in cases.into_iter().enumerate() {
        let body = format!(r#"{{"email":"case{i}@example.com","password":"{WRONG}"}}"#);
        let resp = post_raw(
            &app,
            80 + i as u8,
            "/auth/local/login",
            JSON,
            &body,
            headers,
        )
        .await;
        let (status, code) = status_and_code(resp).await;
        let expected = if passes {
            (StatusCode::UNAUTHORIZED, "auth.invalid")
        } else {
            (StatusCode::FORBIDDEN, "auth.csrf")
        };
        assert_eq!((status, code.as_str()), expected, "{headers:?}");
    }
}

/// A refused form navigation is sent back to its page with the generic
/// `auth.csrf` error; a refused JSON call gets the 403 envelope.
#[tokio::test]
async fn refused_form_posts_redirect_back_and_json_gets_the_envelope() {
    let app = TestApp::spawn().await;
    let cross: &[(&str, &str)] = &[("sec-fetch-site", "cross-site")];
    let cases = [
        (
            "/auth/local/login",
            "email=x%40example.com&password=pw",
            "/sign-in?error=auth.csrf",
        ),
        (
            "/auth/local/register",
            "email=x%40example.com&password=pw",
            "/sign-in?error=auth.csrf",
        ),
        (
            "/auth/local/request-password-reset",
            "email=x%40example.com",
            "/forgot-password?error=auth.csrf",
        ),
        (
            "/auth/local/reset-password",
            "token=tok-123&new_password=pw",
            "/reset-password?token=tok-123&error=auth.csrf",
        ),
    ];
    for (i, (route, body, target)) in cases.into_iter().enumerate() {
        let resp = post_raw(&app, 90 + i as u8, route, FORM, body, cross).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER, "{route}");
        assert!(
            location(&resp).starts_with(target),
            "{route}: {}",
            location(&resp)
        );
    }
    // Sec-Fetch-Mode: navigate marks a form navigation too.
    let resp = post_raw(
        &app,
        95,
        "/auth/local/login",
        "text/plain",
        "x",
        &[
            ("sec-fetch-site", "cross-site"),
            ("sec-fetch-mode", "navigate"),
        ],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let resp = post_raw(&app, 96, "/auth/local/login", JSON, "{}", cross).await;
    let (status, code) = status_and_code(resp).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::FORBIDDEN, "auth.csrf")
    );
}

/// Redis key of a failed-auth counter, derived as `auth::failed_auth`
/// derives it (SHA-256 of the lower-cased address, first 8 bytes in hex).
fn email_counter_key(email: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(email.trim().to_lowercase().as_bytes());
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("auth_fail_email:{hex}")
}

async fn counter(app: &TestApp, key: &str) -> Option<i64> {
    use redis::AsyncCommands;
    let mut redis = app.state().jobs.redis.clone();
    redis.get(key).await.unwrap()
}

/// A refused cross-site POST never reaches the handler, so it charges
/// neither lockout axis.
#[tokio::test]
async fn refused_cross_site_posts_do_not_touch_the_lockout_counters() {
    let app = TestApp::spawn().await;
    let email = "counted@example.com";
    let body = format!(r#"{{"email":"{email}","password":"{WRONG}"}}"#);
    let resp = post_raw(
        &app,
        100,
        "/auth/local/login",
        JSON,
        &body,
        &[("sec-fetch-site", "cross-site")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(counter(&app, &email_counter_key(email)).await, None);
    assert_eq!(counter(&app, "auth_fail:198.51.100.100").await, None);

    // Control: the same attempt same-origin is counted on both axes.
    let resp = post_raw(&app, 100, "/auth/local/login", JSON, &body, &[]).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(counter(&app, &email_counter_key(email)).await, Some(1));
    assert_eq!(counter(&app, "auth_fail:198.51.100.100").await, Some(1));
}

/// A trusted proxy's `X-Forwarded-Host` counts as the request's host for
/// the `Origin` rule; an untrusted peer's is ignored.
#[tokio::test]
async fn forwarded_host_counts_only_from_a_trusted_proxy() {
    let headers: &[(&str, &str)] = &[
        ("origin", "https://comics.lan"),
        ("host", "folio:8080"),
        ("x-forwarded-host", "comics.lan"),
        ("x-forwarded-proto", "https"),
    ];
    let body = format!(r#"{{"email":"x@example.com","password":"{WRONG}"}}"#);

    let trusted = TestApp::spawn_with_trusted_proxies("198.51.100.0/24").await;
    let resp = post_raw(&trusted, 110, "/auth/local/login", JSON, &body, headers).await;
    let (status, code) = status_and_code(resp).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::UNAUTHORIZED, "auth.invalid")
    );

    let untrusted = TestApp::spawn().await;
    let resp = post_raw(&untrusted, 110, "/auth/local/login", JSON, &body, headers).await;
    let (status, code) = status_and_code(resp).await;
    assert_eq!(
        (status, code.as_str()),
        (StatusCode::FORBIDDEN, "auth.csrf")
    );
}

/// The `/api`-prefixed mounts of the sign-in routes are not CSRF-exempt:
/// without the double-submit token they are refused even same-origin, so
/// they are not reachable cross-site either.
#[tokio::test]
async fn api_mounts_of_the_sign_in_routes_need_the_csrf_token() {
    let app = TestApp::spawn().await;
    for (route, body) in GUARDED {
        let api = format!("/api{route}");
        let resp = post_raw(
            &app,
            120,
            &api,
            JSON,
            body,
            &[("sec-fetch-site", "same-origin")],
        )
        .await;
        let (status, code) = status_and_code(resp).await;
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::FORBIDDEN, "auth.csrf"),
            "{api}"
        );
    }
}
