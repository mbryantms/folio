//! WP-1.6 ops hygiene: the tower layers `app::build_openapi_router` puts on
//! the JSON `api` group (`TimeoutLayer` + `CompressionLayer`) and — just as
//! important — the surfaces it deliberately leaves alone.
//!
//! - `/api/*` JSON is gzip-encoded when the client asks for it and left as
//!   identity when it doesn't.
//! - Page bytes (`/issues/{id}/pages/{n}`, bare group) are never encoded,
//!   even when the client advertises gzip: the reader relies on verbatim
//!   bytes, `Content-Length` and Range semantics.
//! - Thumbnail bytes (`/issues/{id}/pages/{n}/thumb`, bare group) are never
//!   encoded either, and carry the OP-7 cache contract: `Cache-Control:
//!   private, max-age=300` plus a strong digest `ETag`, with `If-None-Match`
//!   answered by a bodiless 304 that keeps both headers.
//! - The bare group as a whole is untouched (`/healthz` stays identity).
//! - The page-image upload (`POST /api/uploads`) sits outside the JSON
//!   layers (E8): its body may take longer than the JSON deadline to
//!   arrive, a client that goes silent mid-body is still dropped at the
//!   per-frame idle timeout, and it is never compressed. JSON routes, and
//!   the CBL file import, still 408 at the JSON deadline.
//!
//! The JSON timeout is exercised at the unit level in `app::tests` with a
//! tiny deadline. The group-membership guards here build the real router
//! through `app::router_with_timeouts` with short test deadlines, since
//! wall-clocking a 60 s timeout would be pointless.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{seed_issue, seed_library, seed_series};
use server::app::{JSON_ROUTE_TIMEOUT, RouteTimeouts};
use std::io::Write;
use std::time::{Duration, Instant};
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

/// One-page CBZ whose page is a real, decodable PNG — the thumbnail route
/// has to encode it, so the fake-signature payload above will not do.
fn decodable_cbz_bytes() -> Vec<u8> {
    use image::{ImageBuffer, ImageFormat, Rgba};
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_fn(64, 96, |x, y| Rgba([x as u8 * 3, y as u8 * 2, 90, 255]));
    let mut png: Vec<u8> = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
        .unwrap();
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.finish().unwrap().into_inner()
}

async fn register_admin(app: &TestApp) -> String {
    register_admin_authed(app).await.0
}

/// Registers the first-user admin; returns `(session, csrf)` cookie values.
async fn register_admin_authed(app: &TestApp) -> (String, String) {
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
    let cookie = |prefix: &str| -> String {
        resp.headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|c| c.starts_with(prefix))
            .expect(prefix)
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches(prefix)
            .to_owned()
    };
    (
        cookie("__Host-comic_session="),
        cookie("__Host-comic_csrf="),
    )
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

/// Audit OP-7 + the bare-group invariant together: thumbnail bytes leave
/// the server verbatim (no `Content-Encoding` however loudly the client
/// asks), with `Cache-Control: private, max-age=300`, a strong digest
/// `ETag`, and a 304 on `If-None-Match` that carries both headers and no
/// body. A layer leaking onto this route would re-encode the image or, on
/// a slow NAS, time out the inline generation.
#[tokio::test]
async fn thumbnail_bytes_are_never_compressed_and_carry_the_cache_contract() {
    let app = TestApp::spawn().await;
    let session = register_admin(&app).await;
    let db = &app.state().db;

    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(db, tmp.path()).await;
    let series = seed_series(db, lib, "Thumb Layers").await;
    let issue_id = seed_issue(
        db,
        lib,
        series,
        &tmp.path().join("Thumb Layers 001.cbz"),
        &decodable_cbz_bytes(),
        1.0,
    )
    .await;

    let uri = format!("/issues/{issue_id}/pages/0/thumb");
    let resp = get(&app, &session, &uri, Some("gzip, deflate, br, zstd")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        content_encoding(&resp),
        None,
        "thumbnail bytes must go out verbatim regardless of Accept-Encoding"
    );
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("image/webp")
    );
    assert_eq!(
        resp.headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("private, max-age=300"),
        "OP-7: five-minute private freshness, never `no-cache`, never `public`"
    );
    assert!(
        resp.headers().get(header::VARY).is_none_or(|v| !v
            .to_str()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("accept-encoding")),
        "no content negotiation on the thumb route, so no Vary: Accept-Encoding"
    );
    let etag = resp
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .expect("strong ETag")
        .to_owned();
    assert!(
        etag.starts_with('"') && etag.ends_with('"') && !etag.starts_with("W/"),
        "ETag must be a strong quoted validator, got {etag:?}"
    );
    let declared_len = resp
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
        .expect("Content-Length");
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.len(), declared_len);
    assert_eq!(
        etag,
        format!("\"{}\"", blake3::hash(&body).to_hex()),
        "ETag is the digest of the bytes on the wire"
    );

    // Revalidation: 304, no body, both cache headers repeated, still identity.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(&uri)
                .header(header::COOKIE, format!("__Host-comic_session={session}"))
                .header(header::ACCEPT_ENCODING, "gzip")
                .header(header::IF_NONE_MATCH, &etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(content_encoding(&resp), None);
    assert_eq!(
        resp.headers()
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok()),
        Some(etag.as_str())
    );
    assert_eq!(
        resp.headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("private, max-age=300")
    );
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert!(body.is_empty(), "304 must not carry a body");
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

// ───────── E8: long-body uploads run outside the JSON deadline ─────────

/// Short deadlines for the group-membership guards below: the JSON group
/// gives up well before [`BODY_STALL`], the long-body group (whole request
/// and per frame) well after.
const TEST_TIMEOUTS: RouteTimeouts = RouteTimeouts {
    json: Duration::from_millis(500),
    long_body: Duration::from_secs(30),
    long_body_idle: Duration::from_secs(10),
};
/// How long the slow client pauses mid-body.
const BODY_STALL: Duration = Duration::from_millis(1500);
const BOUNDARY: &str = "folio-http-layers-boundary";

/// `bytes` streamed in two chunks with a [`BODY_STALL`] pause between them
/// and no `Content-Length`, like a slow phone upload.
fn stalled_body(bytes: Vec<u8>) -> Body {
    use futures::StreamExt;
    let tail = axum::body::Bytes::from(bytes);
    let head = tail.slice(..tail.len() / 2);
    let tail = tail.slice(tail.len() / 2..);
    let stream = futures::stream::once(async move { Ok::<_, std::io::Error>(head) }).chain(
        futures::stream::once(async move {
            tokio::time::sleep(BODY_STALL).await;
            Ok(tail)
        }),
    );
    Body::from_stream(stream)
}

fn multipart_file(file_name: &str, content_type: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; \
         filename=\"{file_name}\"\r\nContent-Type: {content_type}\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn page_upload_body() -> Vec<u8> {
    let mut image = PNG_HEADER.to_vec();
    image.resize(4096, 0);
    multipart_file("page.png", "image/png", &image)
}

/// POST a stalled body through the real router built with `timeouts`;
/// returns the response and how long it took.
async fn post_stalled(
    app: &TestApp,
    auth: &(String, String),
    timeouts: RouteTimeouts,
    uri: &str,
    content_type: &str,
    body: Vec<u8>,
) -> (axum::response::Response, Duration) {
    let router = server::app::router_with_timeouts(app.state(), timeouts);
    let (session, csrf) = auth;
    let started = Instant::now();
    let resp = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::ACCEPT_ENCODING, "gzip, deflate, br, zstd")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={session}; __Host-comic_csrf={csrf}"),
                )
                .header("X-CSRF-Token", csrf)
                .body(stalled_body(body))
                .unwrap(),
        )
        .await
        .unwrap();
    (resp, started.elapsed())
}

fn multipart_type() -> String {
    format!("multipart/form-data; boundary={BOUNDARY}")
}

/// The served router gives the page upload minutes, not the JSON minute:
/// a 32 MiB page has to land at well under 1 Mbit/s, while a client that
/// goes silent is still dropped within the old JSON hold.
#[test]
fn production_timeouts_give_uploads_the_long_deadline() {
    let t = RouteTimeouts::PRODUCTION;
    assert_eq!(t.json, JSON_ROUTE_TIMEOUT);
    assert!(t.long_body >= Duration::from_secs(10 * 60), "{t:?}");
    assert!(t.long_body_idle <= JSON_ROUTE_TIMEOUT, "{t:?}");
}

#[tokio::test]
async fn page_upload_outlasts_the_json_deadline_and_is_not_compressed() {
    let app = TestApp::spawn().await;
    let auth = register_admin_authed(&app).await;

    let (resp, elapsed) = post_stalled(
        &app,
        &auth,
        TEST_TIMEOUTS,
        "/api/uploads",
        &multipart_type(),
        page_upload_body(),
    )
    .await;
    assert!(
        elapsed >= BODY_STALL && BODY_STALL > TEST_TIMEOUTS.json,
        "precondition: the body arrived after the JSON deadline ({elapsed:?})"
    );
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "the upload route must run under the long-body deadline, not the JSON one"
    );
    assert_eq!(
        content_encoding(&resp),
        None,
        "the upload route sits outside the CompressionLayer"
    );
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert!(body.len() > 32, "precondition: body above the size floor");
    let v: serde_json::Value = serde_json::from_slice(&body).expect("identity JSON body");
    assert_eq!(v["content_type"], "image/png");
}

/// A client that goes silent mid-body is dropped at the per-frame idle
/// timeout, long before the whole-request deadline, with the 408 envelope
/// and no staged leftovers.
#[tokio::test]
async fn page_upload_drops_a_stalled_client_at_the_idle_timeout() {
    let app = TestApp::spawn().await;
    let auth = register_admin_authed(&app).await;
    let timeouts = RouteTimeouts {
        long_body_idle: Duration::from_millis(300),
        ..TEST_TIMEOUTS
    };

    let (resp, elapsed) = post_stalled(
        &app,
        &auth,
        timeouts,
        "/api/uploads",
        &multipart_type(),
        page_upload_body(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::REQUEST_TIMEOUT);
    assert!(elapsed < BODY_STALL, "cut off mid-stall ({elapsed:?})");
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).expect("JSON envelope");
    assert_eq!(v["error"]["code"], "request_timeout", "{v}");
    let staged = app.state().cfg().data_path.join("uploads");
    let leftovers: Vec<_> = std::fs::read_dir(&staged)
        .map(|d| d.filter_map(Result::ok).map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "staged leftovers: {leftovers:?}");
}

/// JSON routes, and the CBL file import that deliberately stays in the
/// JSON group (its handler does the DB matching inline), are still cut
/// off at the JSON deadline when the body stalls.
#[tokio::test]
async fn json_group_routes_are_still_cut_off_at_the_json_deadline() {
    let app = TestApp::spawn().await;
    let auth = register_admin_authed(&app).await;

    let cases = [
        (
            "/api/me/cbl-lists",
            "application/json".to_owned(),
            br#"{"name":"Slow JSON","xml":"<ReadingList/>"}"#.to_vec(),
        ),
        (
            "/api/me/cbl-lists/upload",
            multipart_type(),
            multipart_file(
                "slow.cbl",
                "application/xml",
                b"<ReadingList><Name>Slow link</Name><Books/></ReadingList>",
            ),
        ),
    ];
    for (uri, content_type, body) in cases {
        let (resp, elapsed) =
            post_stalled(&app, &auth, TEST_TIMEOUTS, uri, &content_type, body).await;
        assert_eq!(
            resp.status(),
            StatusCode::REQUEST_TIMEOUT,
            "{uri}: a body stalling past the JSON deadline must 408"
        );
        assert!(
            elapsed < BODY_STALL,
            "{uri}: cut off at the JSON deadline, before the body resumed ({elapsed:?})"
        );
    }
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
