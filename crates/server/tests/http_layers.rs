//! WP-1.6 ops hygiene: the tower layers `app::build_openapi_router` puts on
//! the JSON `api` group (`TimeoutLayer` + `CompressionLayer`) and — just as
//! important — the surfaces it deliberately leaves alone.
//!
//! - `/api/*` JSON is gzip-encoded when the client asks for it and left as
//!   identity when it doesn't.
//! - Page bytes (`/issues/{id}/pages/{n}`, bare group) are never encoded,
//!   even when the client advertises gzip: the reader relies on verbatim
//!   bytes, `Content-Length` and Range semantics.
//! - The bare group as a whole is untouched (`/healthz` stays identity).
//!
//! The timeout is exercised at the unit level in `app::tests` with a tiny
//! deadline; wall-clocking a 60 s timeout here would be pointless.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{seed_issue, seed_library, seed_series};
use std::io::Write;
use tower::ServiceExt;

const PNG_HEADER: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
/// RFC 1952 gzip member header: ID1 ID2.
const GZIP_MAGIC: &[u8] = &[0x1F, 0x8B];
const PAGE_LEN: usize = 256;

/// A PNG-sniffable page payload (real signature + deterministic fill) so the
/// server's content sniff yields `image/png` and the reader accepts it.
fn page_payload() -> Vec<u8> {
    let mut v = PNG_HEADER.to_vec();
    while v.len() < PAGE_LEN {
        v.push((v.len() & 0xFF) as u8);
    }
    v
}

/// One-page CBZ, Stored, built in memory.
fn cbz_bytes() -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&page_payload()).unwrap();
    zw.finish().unwrap().into_inner()
}

async fn register_admin(app: &TestApp) -> String {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"admin@example.com","password":"correctly-horse-battery"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("__Host-comic_session="))
        .expect("session cookie")
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("__Host-comic_session=")
        .to_owned()
}

async fn get(
    app: &TestApp,
    session: &str,
    uri: &str,
    accept_encoding: Option<&str>,
) -> axum::response::Response {
    let mut b = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, format!("__Host-comic_session={session}"));
    if let Some(enc) = accept_encoding {
        b = b.header(header::ACCEPT_ENCODING, enc);
    }
    app.router
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn content_encoding(resp: &axum::response::Response) -> Option<String> {
    resp.headers()
        .get(header::CONTENT_ENCODING)
        .map(|v| v.to_str().unwrap().to_owned())
}

#[tokio::test]
async fn json_api_response_is_gzipped_when_client_accepts_it() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;

    let resp = get(&app, &session, "/api/auth/me", Some("gzip")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        content_encoding(&resp).as_deref(),
        Some("gzip"),
        "JSON api route must honour Accept-Encoding: gzip"
    );
    let vary = resp
        .headers()
        .get_all(header::VARY)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect::<Vec<_>>()
        .join(",")
        .to_ascii_lowercase();
    assert!(
        vary.contains("accept-encoding"),
        "compressed response must Vary on Accept-Encoding, got {vary:?}"
    );
    // The body is a real gzip member, not JSON with a misleading header.
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert!(
        body.starts_with(GZIP_MAGIC),
        "body should start with the gzip magic, got {:?}",
        &body[..body.len().min(4)]
    );
}

#[tokio::test]
async fn json_api_response_stays_identity_without_accept_encoding() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;

    let resp = get(&app, &session, "/api/auth/me", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        content_encoding(&resp),
        None,
        "no Accept-Encoding → identity body"
    );
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).expect("plain JSON body");
    assert_eq!(v["email"], "admin@example.com");
}

#[tokio::test]
async fn page_bytes_are_never_compressed() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let db = &app.state().db;

    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(db, tmp.path()).await;
    let series = seed_series(db, lib, "Layers").await;
    let issue_id = seed_issue(
        db,
        lib,
        series,
        &tmp.path().join("Layers 001.cbz"),
        &cbz_bytes(),
        1.0,
    )
    .await;

    // Page indices are 0-based (`/pages/0` is the first page).
    let resp = get(
        &app,
        &session,
        &format!("/issues/{issue_id}/pages/0"),
        Some("gzip, deflate, br, zstd"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("image/png")
    );
    assert_eq!(
        content_encoding(&resp),
        None,
        "page bytes must go out verbatim regardless of Accept-Encoding"
    );
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<usize>().ok()),
        Some(PAGE_LEN),
        "Content-Length must survive — Range requests depend on it"
    );
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), page_payload().as_slice());
}

#[tokio::test]
async fn bare_group_is_not_wrapped_by_the_json_layers() {
    let app = TestApp::spawn().await;
    // `/healthz` is in the bare group and its JSON body is well over the
    // 32-byte compression floor, so a `Content-Encoding` here would mean the
    // layers leaked out of the `api` group onto the streaming surface.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .header(header::ACCEPT_ENCODING, "gzip")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(content_encoding(&resp), None);
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert!(body.len() > 32, "precondition: body above the size floor");
    serde_json::from_slice::<serde_json::Value>(&body).expect("identity JSON body");
}

// ───────── WP-6.3 / security-audit M-1: deny-by-default CORS ─────────
//
// The Rust binary is the single public origin, so no cross-origin browser
// access is ever granted. `app::deny_all_cors()` answers preflights itself
// (empty 200, no `Access-Control-Allow-*`) and leaves simple responses free
// of CORS headers, so a foreign page can neither send a non-simple request
// nor read a response. Non-browser clients (OPDS, KOReader, Komga shims)
// never consult CORS and are unaffected.

const EVIL_ORIGIN: &str = "https://evil.example";

fn assert_no_cors_grant(resp: &axum::response::Response, ctx: &str) {
    for h in [
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        header::ACCESS_CONTROL_ALLOW_METHODS,
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
    ] {
        assert!(
            resp.headers().get(&h).is_none(),
            "{ctx}: unexpected {h} = {:?}",
            resp.headers().get(&h)
        );
    }
}

#[tokio::test]
async fn cors_preflight_from_foreign_origin_is_denied() {
    let app = TestApp::spawn().await;
    for path in [
        "/api/auth/me",
        "/api/auth/local/login",
        "/opds/v1",
        "/healthz",
    ] {
        let resp = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri(path)
                    .header(header::ORIGIN, EVIL_ORIGIN)
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .header(
                        header::ACCESS_CONTROL_REQUEST_HEADERS,
                        "content-type, x-csrf-token",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Answered by the CORS layer itself — never forwarded to a handler or
        // the Next upstream — and grants nothing.
        assert_eq!(resp.status(), StatusCode::OK, "{path}");
        assert_no_cors_grant(&resp, path);
    }
}

#[tokio::test]
async fn cors_cross_origin_credentialed_read_gets_no_grant() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header(header::ORIGIN, EVIL_ORIGIN)
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // The request itself still runs (CORS is enforced by the browser on the
    // response), but without ACAO/ACAC the foreign page can't read it.
    assert_eq!(resp.status(), StatusCode::OK);
    assert_no_cors_grant(&resp, "/api/auth/me");
}

#[tokio::test]
async fn cors_layer_leaves_same_origin_and_non_browser_requests_alone() {
    let app = TestApp::spawn().await;
    // No Origin header (OPDS reader / KOReader / curl): plain response, no
    // CORS headers, no Vary noise.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/auth/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_no_cors_grant(&resp, "no-origin");
    assert!(
        resp.headers().get(header::VARY).is_none_or(|v| !v
            .to_str()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("origin")),
        "deny-all CORS must not add Vary: Origin"
    );
}
