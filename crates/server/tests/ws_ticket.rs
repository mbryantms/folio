//! WebSocket auth ticket flow (§9.6).
//!
//! Covers:
//!   - Mint requires cookie auth (401 anonymous)
//!   - Mint returns a ticket + 30s expires_in
//!   - WS upgrade with `?ticket=<good>` succeeds (passes auth — actually
//!     completing the handshake needs a real WS client; we settle for the
//!     ticket being consumed and an upgrade-attempt being rejected with the
//!     "missing upgrade headers" 400, not a 401)
//!   - WS upgrade with `?ticket=<unknown>` returns 401
//!   - Tickets are one-time-use: a second consume of the same ticket is 401

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use tower::ServiceExt;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.expect("collect body");
    serde_json::from_slice(&bytes).expect("json body")
}

async fn register(app: &TestApp, email: &str, password: &str) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"{password}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn extract_cookie(resp: &axum::http::Response<Body>, name: &str) -> Option<String> {
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

async fn mint_ticket(app: &TestApp, session: &str, csrf: &str) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/ws-ticket")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={session}; __Host-comic_csrf={csrf}"),
                )
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn mint_requires_auth() {
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/ws-ticket")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // CSRF middleware runs first and rejects POST without a CSRF cookie/header
    // (also unauthorized in spirit). Either FORBIDDEN or UNAUTHORIZED is fine
    // — the point is anonymous callers don't get a ticket.
    assert!(
        resp.status() == StatusCode::UNAUTHORIZED || resp.status() == StatusCode::FORBIDDEN,
        "got {}",
        resp.status()
    );
}

#[tokio::test]
async fn mint_returns_ticket_for_admin() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "first@example.com", "correctly-horse-battery").await;
    assert_eq!(reg.status(), StatusCode::CREATED);
    let session = extract_cookie(&reg, "__Host-comic_session").unwrap();
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();

    let resp = mint_ticket(&app, &session, &csrf).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let ticket = body["ticket"].as_str().expect("ticket field");
    assert!(uuid::Uuid::parse_str(ticket).is_ok(), "ticket is a UUID");
    assert_eq!(body["expires_in"], 300);
}

#[tokio::test]
async fn ws_upgrade_with_unknown_ticket_is_401() {
    let app = TestApp::spawn().await;
    // No registration — go straight to the WS endpoint with a fake ticket.
    let bogus = uuid::Uuid::now_v7().to_string();
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/ws/scan-events?ticket={bogus}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tickets_are_one_time_use() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "admin@example.com", "correctly-horse-battery").await;
    let session = extract_cookie(&reg, "__Host-comic_session").unwrap();
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();

    let mint = mint_ticket(&app, &session, &csrf).await;
    assert_eq!(mint.status(), StatusCode::OK);
    let ticket = body_json(mint.into_body()).await["ticket"]
        .as_str()
        .unwrap()
        .to_owned();

    // First consume — expect 400 (missing the WS upgrade headers) or a
    // successful upgrade attempt. The 401 case would mean the ticket wasn't
    // accepted, which is the bug we're guarding against.
    let first = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/ws/scan-events?ticket={ticket}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        first.status(),
        StatusCode::UNAUTHORIZED,
        "first consume must accept the ticket; status was {}",
        first.status()
    );

    // Second consume of the same ticket — should now be a 401 since GETDEL
    // removed it on first use.
    let second = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/ws/scan-events?ticket={ticket}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ws_upgrade_without_ticket_or_cookie_is_401() {
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/ws/scan-events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ───── Origin rule for cookie-authenticated upgrades ─────
//
// A cookie-authenticated upgrade must carry an `Origin` equal to the
// configured public origin (`http://localhost:8080` in the harness). A
// ticket-authenticated upgrade has no Origin requirement.

const PUBLIC_ORIGIN: &str = "http://localhost:8080";

/// Perform a real WebSocket handshake over TCP against the router and
/// return the response status line. `extra_headers` are raw header lines.
async fn handshake_status(app: &TestApp, uri: &str, extra_headers: &[String]) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let mut client = TcpStream::connect(addr).await.unwrap();
    let mut request = format!(
        "GET {uri} HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Upgrade: websocket\r\n\
         Connection: upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n"
    );
    for h in extra_headers {
        request.push_str(h);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    client.write_all(request.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let mut read = 0;
    loop {
        let n = client.read(&mut buf[read..]).await.unwrap();
        assert!(n > 0, "closed before the response head finished");
        read += n;
        if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf[..read]).into_owned();
    head.lines().next().unwrap_or_default().to_owned()
}

async fn admin_cookies(app: &TestApp) -> (String, String) {
    let reg = register(app, "origin-admin@example.com", "correctly-horse-battery").await;
    assert_eq!(reg.status(), StatusCode::CREATED);
    (
        extract_cookie(&reg, "__Host-comic_session").unwrap(),
        extract_cookie(&reg, "__Host-comic_csrf").unwrap(),
    )
}

fn cookie_header(session: &str, csrf: &str) -> String {
    format!("Cookie: __Host-comic_session={session}; __Host-comic_csrf={csrf}")
}

async fn cookie_upgrade_without_upgrade_headers(
    app: &TestApp,
    session: &str,
    csrf: &str,
    origin: Option<&str>,
) -> axum::http::Response<Body> {
    let mut b = Request::builder()
        .method(Method::GET)
        .uri("/ws/scan-events")
        .header(
            header::COOKIE,
            format!("__Host-comic_session={session}; __Host-comic_csrf={csrf}"),
        );
    if let Some(o) = origin {
        b = b.header(header::ORIGIN, o);
    }
    app.router
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn fresh_ticket(app: &TestApp, session: &str, csrf: &str) -> String {
    let mint = mint_ticket(app, session, csrf).await;
    assert_eq!(mint.status(), StatusCode::OK);
    body_json(mint.into_body()).await["ticket"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn cookie_upgrade_from_public_origin_upgrades() {
    let app = TestApp::spawn().await;
    let (session, csrf) = admin_cookies(&app).await;
    let status = handshake_status(
        &app,
        "/ws/scan-events",
        &[
            cookie_header(&session, &csrf),
            format!("Origin: {PUBLIC_ORIGIN}"),
        ],
    )
    .await;
    assert!(status.starts_with("HTTP/1.1 101 "), "{status}");
}

#[tokio::test]
async fn cookie_upgrade_from_foreign_origin_is_403() {
    let app = TestApp::spawn().await;
    let (session, csrf) = admin_cookies(&app).await;
    for origin in [
        "https://evil.example",
        "http://sibling.localhost:8080",
        "http://localhost:3000",
        "null",
    ] {
        let resp =
            cookie_upgrade_without_upgrade_headers(&app, &session, &csrf, Some(origin)).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "origin {origin}");
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["error"]["code"], "auth.permission_denied", "{body}");
    }
    // And over a real handshake: refused before the upgrade.
    let status = handshake_status(
        &app,
        "/ws/scan-events",
        &[
            cookie_header(&session, &csrf),
            "Origin: https://evil.example".to_owned(),
        ],
    )
    .await;
    assert!(status.starts_with("HTTP/1.1 403 "), "{status}");
}

#[tokio::test]
async fn cookie_upgrade_without_origin_is_403() {
    let app = TestApp::spawn().await;
    let (session, csrf) = admin_cookies(&app).await;
    let resp = cookie_upgrade_without_upgrade_headers(&app, &session, &csrf, None).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.permission_denied", "{body}");
    let status = handshake_status(&app, "/ws/scan-events", &[cookie_header(&session, &csrf)]).await;
    assert!(status.starts_with("HTTP/1.1 403 "), "{status}");
}

#[tokio::test]
async fn ticket_upgrade_without_origin_upgrades() {
    let app = TestApp::spawn().await;
    let (session, csrf) = admin_cookies(&app).await;
    let ticket = fresh_ticket(&app, &session, &csrf).await;
    let status = handshake_status(&app, &format!("/ws/scan-events?ticket={ticket}"), &[]).await;
    assert!(status.starts_with("HTTP/1.1 101 "), "{status}");
}

#[tokio::test]
async fn ticket_upgrade_with_foreign_origin_upgrades() {
    // The ticket is the credential: it was minted by a cookie + CSRF POST,
    // which a foreign page cannot make, and it is single-use.
    let app = TestApp::spawn().await;
    let (session, csrf) = admin_cookies(&app).await;
    let ticket = fresh_ticket(&app, &session, &csrf).await;
    let status = handshake_status(
        &app,
        &format!("/ws/scan-events?ticket={ticket}"),
        &["Origin: https://evil.example".to_owned()],
    )
    .await;
    assert!(status.starts_with("HTTP/1.1 101 "), "{status}");
}
