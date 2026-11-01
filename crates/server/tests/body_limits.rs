//! Explicit request-body limits (audit 2026-11 N-A2) and the async staged
//! upload (N-A1).
//!
//! - Buffered extractors on both route groups stop at
//!   `app::DEFAULT_BODY_LIMIT` (2 MiB) and answer 413 with the canonical
//!   envelope, not axum's plain-text rejection.
//! - `POST /api/uploads` raises its own limit to 32 MiB (+ framing): a
//!   31 MiB image stages, a 33 MiB one is refused, and the staged file is
//!   consumed by the page editor only in a writeback library.
//! - The Next.js fallback proxy refuses a body over
//!   `upstream::MAX_PROXY_BODY_BYTES`, declared or streamed.
//! - `/ws/scan-events` closes a client that sends an oversize frame.

mod common;

use std::io::{Cursor, Write};
use std::path::Path;
use std::time::Duration;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use server::jobs::archive_edit::{ArchiveEditJob, PageOp, edit_one_issue};
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const MIB: usize = 1024 * 1024;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| panic!("not JSON: {:?}", String::from_utf8_lossy(&bytes)))
}

struct Authed {
    session: String,
    csrf: String,
}

impl Authed {
    fn cookie(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
}

async fn register_admin(app: &TestApp) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"limits@example.com","password":"correctly-horse-battery"}"#,
                ))
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
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

fn png_bytes(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb(rgb)));
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

/// `len` bytes that `image::guess_format` reads as PNG (signature + zeros).
/// The upload only sniffs; decoding happens in the edit job.
fn png_signed_blob(len: usize) -> Vec<u8> {
    let mut v = vec![0u8; len];
    v[..PNG_SIGNATURE.len()].copy_from_slice(PNG_SIGNATURE);
    v
}

fn cbz_one_page() -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("p1.png", opts).unwrap();
        zw.write_all(&png_bytes(4, 4, [1, 2, 3])).unwrap();
        zw.finish().unwrap();
    }
    buf.into_inner()
}

const BOUNDARY: &str = "folio-body-limit-boundary";

fn multipart_file(bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(bytes.len() + 256);
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"page.png\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

async fn post_upload(
    app: &TestApp,
    auth: &Authed,
    image: &[u8],
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/uploads")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .header(header::COOKIE, auth.cookie())
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(multipart_file(image)))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn post_edit(
    app: &TestApp,
    auth: &Authed,
    issue_id: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/issues/{issue_id}/archive/edit"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookie())
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// Every file in the uploads dir, `.part` leftovers included.
fn staged_files(app: &TestApp) -> Vec<String> {
    let dir = app.state().cfg().data_path.join("uploads");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

async fn seed_issue(app: &TestApp, dir: &Path, writeback: bool) -> String {
    let db = &app.state().db;
    let seed = LibrarySeed::new(dir);
    let seed = if writeback {
        seed.with_sidecar_writeback()
    } else {
        seed
    };
    let lib = seed.insert(db).await;
    let series = SeriesSeed::new(lib, "Limits Series").insert(db).await;
    IssueSeed::new(lib, series, &dir.join("issue.cbz"), &cbz_one_page(), 1.0)
        .insert(db)
        .await
}

// ───── buffered JSON routes ─────

#[tokio::test]
async fn json_route_rejects_3_mib_body_with_413_envelope() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    // A syntactically plausible JSON body, 3 MiB: the body limit fires
    // while buffering, before serde ever sees it.
    let mut big = String::from(r#"{"ops":[],"pad":""#);
    big.push_str(&"x".repeat(3 * MIB));
    big.push_str(r#""}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/issues/does-not-matter/archive/edit")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookie())
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(big))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "too_large", "{body}");
}

#[tokio::test]
async fn validated_route_rejects_3_mib_body_with_413_envelope() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    // `Validated<T>` maps its own JSON rejections (400 / 422); the body
    // limit must still come out as 413 `too_large`, not a 400.
    let mut big = String::from(r#"{"name":""#);
    big.push_str(&"x".repeat(3 * MIB));
    big.push_str(r#""}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/me/collections")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookie())
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(big))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "too_large", "{body}");
}

#[tokio::test]
async fn json_route_accepts_body_under_the_limit() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    // 1.5 MiB of padding: past the old "small JSON" intuition, under the
    // explicit 2 MiB limit, so it reaches the handler (404: no such issue).
    let mut body = String::from(r#"{"ops":[],"pad":""#);
    body.push_str(&"x".repeat(MIB + MIB / 2));
    body.push_str(r#""}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/issues/does-not-exist/archive/edit")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookie())
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bare_route_rejects_3_mib_body_with_413_envelope() {
    let app = TestApp::spawn().await;
    // `/csp-report` is a bare route (CSRF-exempt, no auth) that buffers its
    // body with the `Bytes` extractor.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/csp-report")
                .header(header::CONTENT_TYPE, "application/csp-report")
                .body(Body::from(vec![b'x'; 3 * MIB]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "too_large", "{body}");
}

// ───── staged upload ─────

#[tokio::test]
async fn upload_accepts_31_mib_image() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let blob = png_signed_blob(31 * MIB);
    let (status, body) = post_upload(&app, &auth, &blob).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["content_type"], "image/png");
    let id = body["id"].as_str().unwrap().to_owned();
    let staged = app.state().cfg().data_path.join("uploads").join(&id);
    assert_eq!(std::fs::metadata(&staged).unwrap().len(), blob.len() as u64);
    // Only the final name is left behind, never a `.part`.
    assert_eq!(staged_files(&app), vec![id]);
}

#[tokio::test]
async fn upload_rejects_33_mib_image_with_413() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (status, body) = post_upload(&app, &auth, &png_signed_blob(33 * MIB)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["error"]["code"], "too_large");
    // Nothing staged, partial or otherwise.
    assert!(staged_files(&app).is_empty(), "{:?}", staged_files(&app));
}

#[tokio::test]
async fn upload_rejects_non_image_and_cleans_up() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (status, body) = post_upload(&app, &auth, b"definitely not an image").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "validation.not_an_image");
    assert!(staged_files(&app).is_empty(), "{:?}", staged_files(&app));
}

#[tokio::test]
async fn upload_feeds_the_editor_in_a_writeback_library() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let issue_id = seed_issue(&app, dir.path(), true).await;

    let (status, body) = post_upload(&app, &auth, &png_bytes(8, 16, [9, 9, 9])).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let image_id: Uuid = body["id"].as_str().unwrap().parse().unwrap();

    let (status, body) = post_edit(
        &app,
        &auth,
        &issue_id,
        serde_json::json!({ "ops": [{ "kind": "replace", "ordinal": 0, "image_id": image_id }] }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["status"], "queued");

    // Run the queued edit inline (the worker isn't running in tests).
    edit_one_issue(
        &app.state(),
        &ArchiveEditJob {
            issue_id: issue_id.clone(),
            ops: vec![PageOp::Replace {
                ordinal: 0,
                image_id,
            }],
            bulk_op: None,
            actor_id: None,
            actor_ip: None,
            actor_ua: None,
            attempt: 0,
        },
    )
    .await
    .unwrap();

    let archive = archive::open(
        &dir.path().join("issue.cbz"),
        archive::ArchiveLimits::default(),
    )
    .unwrap();
    assert_eq!(archive.pages().len(), 1);
    let mut cbz = archive::cbz::Cbz::open(
        dir.path().join("issue.cbz"),
        archive::ArchiveLimits::default(),
    )
    .unwrap();
    let page = cbz.read_entry_bytes_by_name("p0001.png").unwrap();
    let img = image::load_from_memory(&page).unwrap();
    assert_eq!((img.width(), img.height()), (8, 16));
}

#[tokio::test]
async fn upload_is_refused_by_the_editor_without_writeback() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let issue_id = seed_issue(&app, dir.path(), false).await;

    // Staging is library-agnostic; the `allow_archive_writeback` gate sits
    // on the edit that would consume it.
    let (status, body) = post_upload(&app, &auth, &png_bytes(8, 16, [9, 9, 9])).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let image_id = body["id"].as_str().unwrap().to_owned();

    let (status, body) = post_edit(
        &app,
        &auth,
        &issue_id,
        serde_json::json!({ "ops": [{ "kind": "replace", "ordinal": 0, "image_id": image_id }] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        body["error"]["code"],
        "validation.archive_writeback_disabled"
    );
    assert_eq!(
        std::fs::read(dir.path().join("issue.cbz")).unwrap(),
        cbz_one_page()
    );
}

// ───── upstream proxy ─────

#[tokio::test]
async fn proxy_rejects_declared_oversize_body_with_413() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&upstream)
        .await;
    let app = TestApp::spawn_with_web_upstream(upstream.uri()).await;
    let declared = server::upstream::MAX_PROXY_BODY_BYTES + 1;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/some/next/page")
                .header(header::CONTENT_TYPE, "text/plain")
                .header(header::CONTENT_LENGTH, declared.to_string())
                // Double-submit CSRF: any matching cookie + header pair.
                .header(header::COOKIE, "__Host-comic_csrf=t0k3n")
                .header("X-CSRF-Token", "t0k3n")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "too_large", "{body}");
    // `MockServer` verifies `expect(0)` on drop: Next never saw it.
}

#[tokio::test]
async fn proxy_cuts_off_streamed_oversize_body_with_413() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&upstream)
        .await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    // No Content-Length: four 1 KiB chunks against a 2 KiB cap.
    let chunks = futures::stream::iter(
        (0..4).map(|_| Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b'a'; 1024]))),
    );
    let req = Request::builder()
        .method(Method::POST)
        .uri("/action")
        .header(header::HOST, "folio.example.com")
        .body(Body::from_stream(chunks))
        .unwrap();
    let resp = server::upstream::forward_with_body_limit(
        &client,
        &upstream.uri(),
        req,
        Duration::from_secs(5),
        None,
        None,
        2048,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "too_large", "{body}");
}

#[tokio::test]
async fn proxy_forwards_streamed_body_under_the_cap() {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&upstream)
        .await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let chunks = futures::stream::iter(
        (0..2).map(|_| Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b'a'; 1024]))),
    );
    let req = Request::builder()
        .method(Method::POST)
        .uri("/action")
        .header(header::HOST, "folio.example.com")
        .body(Body::from_stream(chunks))
        .unwrap();
    let resp = server::upstream::forward_with_body_limit(
        &client,
        &upstream.uri(),
        req,
        Duration::from_secs(5),
        None,
        None,
        2048,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let received = upstream.received_requests().await.unwrap();
    assert_eq!(received[0].body.len(), 2048);
}

// ───── /ws/scan-events ─────

/// A client→server frame, masked as RFC 6455 requires of clients.
fn masked_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![0x80 | opcode];
    let len = payload.len();
    if len < 126 {
        f.push(0x80 | len as u8);
    } else if len <= usize::from(u16::MAX) {
        f.push(0x80 | 126);
        f.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        f.push(0x80 | 127);
        f.extend_from_slice(&(len as u64).to_be_bytes());
    }
    let mask = [0x11, 0x22, 0x33, 0x44];
    f.extend_from_slice(&mask);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

#[tokio::test]
async fn ws_scan_events_closes_client_sending_oversize_frame() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let mut client = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "GET /ws/scan-events HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Upgrade: websocket\r\n\
         Connection: upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         Cookie: {}\r\n\
         \r\n",
        auth.cookie()
    );
    client.write_all(request.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let mut read = 0;
    loop {
        let n = client.read(&mut buf[read..]).await.unwrap();
        assert!(n > 0, "closed before the handshake finished");
        read += n;
        if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf[..read]);
    assert!(head.starts_with("HTTP/1.1 101 "), "{head}");

    // A small data frame is ignored and the socket stays open.
    client
        .write_all(&masked_frame(0x1, b"hello"))
        .await
        .unwrap();
    let mut probe = [0u8; 64];
    assert!(
        tokio::time::timeout(Duration::from_millis(300), client.read(&mut probe))
            .await
            .is_err(),
        "socket closed or sent data after a small frame"
    );

    // An oversize frame (past `WS_MAX_FRAME_BYTES` and
    // `WS_MAX_MESSAGE_BYTES`) fails the server's read and it hangs up.
    let oversize = vec![b'x'; server::api::ws_scan_events::WS_MAX_MESSAGE_BYTES * 2];
    // The server may hang up mid-write; a broken pipe here is the point.
    let _ = client.write_all(&masked_frame(0x1, &oversize)).await;
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match client.read(&mut probe).await {
                Ok(0) | Err(_) => return true,
                // A close frame (opcode 0x8) also counts as hanging up.
                Ok(_) if probe[0] & 0x0f == 0x8 => return true,
                Ok(_) => {}
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(
        closed,
        "server kept the socket open after an oversize frame"
    );
}
