//! archive-rewrite-1.0 M0 — per-library page-edit settings.
//!
//! Covers the two knobs this plan adds on top of the sister plan's
//! writeback columns:
//!   - `archive_writeback_jpeg_quality` persists and is range-validated
//!     (60..=100) with a friendly 422.
//!   - `allow_archive_writeback` can be enabled on a writable mount but is
//!     refused (422) when the library root isn't on a writable mount.
//!     The read-only case is simulated by removing the library root after
//!     creation so `statvfs` fails and `mount_writable` reports false.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use std::io::Write;
use std::path::Path;
use tower::ServiceExt;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
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
                    r#"{"email":"aes@example.com","password":"correctly-horse-battery"}"#,
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

fn write_cbz(path: &Path) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.finish().unwrap();
}

async fn create_library(app: &TestApp, auth: &Authed, root: &Path) -> String {
    let body = serde_json::json!({ "name": "Edit Lib", "root_path": root.to_string_lossy() });
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/libraries")
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp.into_body()).await;
    json["slug"].as_str().unwrap().to_owned()
}

async fn patch_library(
    app: &TestApp,
    auth: &Authed,
    slug: &str,
    patch: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/api/libraries/{slug}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(patch.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let s = resp.status();
    (s, body_json(resp.into_body()).await)
}

fn seed_library_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("placeholder")).unwrap();
    write_cbz(&tmp.path().join("placeholder").join("p.cbz"));
    tmp
}

#[tokio::test]
async fn jpeg_quality_persists_and_validates() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = seed_library_dir();
    let slug = create_library(&app, &auth, tmp.path()).await;

    // Default is 92 on a fresh library.
    let (status, body) = patch_library(
        &app,
        &auth,
        &slug,
        serde_json::json!({ "archive_writeback_jpeg_quality": 80 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["archive_writeback_jpeg_quality"], 80);

    // Below the 60 floor → 422 from garde.
    let (status, _) = patch_library(
        &app,
        &auth,
        &slug,
        serde_json::json!({ "archive_writeback_jpeg_quality": 50 }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn enable_writeback_on_writable_mount_succeeds() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = seed_library_dir();
    let slug = create_library(&app, &auth, tmp.path()).await;

    let (status, body) = patch_library(
        &app,
        &auth,
        &slug,
        serde_json::json!({ "allow_archive_writeback": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["allow_archive_writeback"], true);
    assert_eq!(body["root_path_writable"], true);
}

#[tokio::test]
async fn enable_writeback_on_readonly_mount_rejected() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = seed_library_dir();
    let slug = create_library(&app, &auth, tmp.path()).await;

    // Remove the library root so `statvfs` on it fails → mount_writable
    // reports false (fail-closed), standing in for a read-only mount.
    drop(tmp);

    let (status, body) = patch_library(
        &app,
        &auth,
        &slug,
        serde_json::json!({ "allow_archive_writeback": true }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["error"]["code"],
        "validation.archive_writeback_mount_readonly"
    );
}

#[tokio::test]
async fn enable_cbr_conversion_requires_writeback() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = seed_library_dir();
    let slug = create_library(&app, &auth, tmp.path()).await;

    // Master toggle off → enabling CBR conversion is a 422.
    let (status, body) = patch_library(
        &app,
        &auth,
        &slug,
        serde_json::json!({ "auto_convert_cbr_on_scan": true }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["error"]["code"],
        "validation.archive_writeback_dependency"
    );

    // With the master toggle on (writable mount), it persists.
    let (status, body) = patch_library(
        &app,
        &auth,
        &slug,
        serde_json::json!({
            "allow_archive_writeback": true,
            "auto_convert_cbr_on_scan": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["auto_convert_cbr_on_scan"], true);
}
