//! archive-rewrite-1.0 M2 — HTTP surface for the page editor.
//!
//! Exercises the endpoint-only logic: admin gating, the per-library
//! `allow_archive_writeback` check, the editable-format guard
//! (cbz/cbt/cbr), and the dry-run path. The byte-level rewrite is covered
//! by `archive_edit.rs`.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use std::io::{Cursor, Write};
use std::path::Path;
use tempfile::tempdir;
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
                    r#"{"email":"aea@example.com","password":"correctly-horse-battery"}"#,
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

fn cbz_two_pages() -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for name in ["p1.png", "p2.png"] {
            let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                4,
                4,
                image::Rgb([7, 7, 7]),
            ));
            let mut pbuf = Cursor::new(Vec::new());
            img.write_to(&mut pbuf, image::ImageFormat::Png).unwrap();
            zw.start_file(name, opts).unwrap();
            zw.write_all(&pbuf.into_inner()).unwrap();
        }
        zw.finish().unwrap();
    }
    buf.into_inner()
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
    let s = resp.status();
    (s, body_json(resp.into_body()).await)
}

/// Seed a library (writeback flag per `writeback`) + series + one issue
/// with `file_name`. Returns the issue id.
async fn seed(
    app: &TestApp,
    dir: &Path,
    writeback: bool,
    file_name: &str,
    payload: Vec<u8>,
) -> String {
    let db = &app.state().db;
    let seed = LibrarySeed::new(dir);
    let seed = if writeback {
        seed.with_sidecar_writeback()
    } else {
        seed
    };
    let lib = seed.insert(db).await;
    let series = SeriesSeed::new(lib, "API Series").insert(db).await;
    let path = dir.join(file_name);
    IssueSeed::new(lib, series, &path, &payload, 1.0)
        .insert(db)
        .await
}

#[tokio::test]
async fn dry_run_returns_page_counts() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let issue_id = seed(&app, dir.path(), true, "issue.cbz", cbz_two_pages()).await;

    let (status, body) = post_edit(
        &app,
        &auth,
        &issue_id,
        serde_json::json!({ "ops": [{ "kind": "remove", "ordinal": 0 }], "dry_run": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "dry_run");
    assert_eq!(body["page_count_before"], 2);
    assert_eq!(body["page_count_after"], 1);
}

#[tokio::test]
async fn edit_rejected_when_writeback_disabled() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let issue_id = seed(&app, dir.path(), false, "issue.cbz", cbz_two_pages()).await;

    let (status, body) = post_edit(
        &app,
        &auth,
        &issue_id,
        serde_json::json!({ "ops": [{ "kind": "remove", "ordinal": 0 }] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["error"]["code"],
        "validation.archive_writeback_disabled"
    );
}

#[tokio::test]
async fn edit_rejected_for_unsupported_format() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    // A `.cb7` issue — 7-zip has no writer, so editing is unsupported
    // (CBZ/CBT/CBR are the editable formats).
    let issue_id = seed(
        &app,
        dir.path(),
        true,
        "issue.cb7",
        b"not-a-real-7z".to_vec(),
    )
    .await;

    let (status, body) = post_edit(
        &app,
        &auth,
        &issue_id,
        serde_json::json!({ "ops": [{ "kind": "remove", "ordinal": 0 }] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["error"]["code"],
        "validation.archive_format_unsupported"
    );
}

#[tokio::test]
async fn edit_invalid_ordinal_returns_422() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();
    let issue_id = seed(&app, dir.path(), true, "issue.cbz", cbz_two_pages()).await;

    let (status, body) = post_edit(
        &app,
        &auth,
        &issue_id,
        serde_json::json!({ "ops": [{ "kind": "remove", "ordinal": 9 }], "dry_run": true }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation.page_ops");
}
