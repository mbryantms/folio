//! Library Scanner v1 — Milestone 4 ignore rules + settings PATCH.
//!
//! Covers:
//!   - `PATCH /libraries/{id}` happy path (admin-only, fields persist)
//!   - 400 on an invalid glob pattern (server never accepts it into the DB)
//!   - the user-configured `ignore_globs` actually filters during a scan
//!   - dotfiles / Thumbs.db / __MACOSX (built-in patterns) are skipped without
//!     any user configuration

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use entity::issue::Entity as IssueEntity;
use sea_orm::EntityTrait;
use server::library::scanner;
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
                    r#"{"email":"ig@example.com","password":"correctly-horse-battery"}"#,
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

/// Returns `(uuid, slug)` so callers can construct slug-based URLs and
/// still pass the canonical UUID to scanner-internal helpers.
async fn create_library(app: &TestApp, auth: &Authed, root: &Path) -> (String, String) {
    let body = serde_json::json!({
        "name": "Ignore Lib",
        "root_path": root.to_string_lossy(),
    });
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
    (
        json["id"].as_str().unwrap().to_owned(),
        json["slug"].as_str().unwrap().to_owned(),
    )
}

async fn patch_library(
    app: &TestApp,
    auth: &Authed,
    lib_id: &str,
    patch: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/api/libraries/{lib_id}"))
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

fn write_cbz(path: &Path, marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.finish().unwrap();
}

#[tokio::test]
async fn patch_persists_settings() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    // Library validation requires non-empty root.
    std::fs::create_dir_all(tmp.path().join("placeholder")).unwrap();
    write_cbz(&tmp.path().join("placeholder").join("p.cbz"), 1);

    let (_lib_uuid, lib_slug) = create_library(&app, &auth, tmp.path()).await;

    let (status, body) = patch_library(
        &app,
        &auth,
        &lib_slug,
        serde_json::json!({
            "ignore_globs": ["**/Promos/*", "**/*.tmp"],
            "report_missing_comicinfo": true,
            "soft_delete_days": 7,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["ignore_globs"],
        serde_json::json!(["**/Promos/*", "**/*.tmp"])
    );
    assert_eq!(body["report_missing_comicinfo"], true);
    assert_eq!(body["soft_delete_days"], 7);
    assert_eq!(body["file_watch_enabled"], false); // unchanged default
}

#[tokio::test]
async fn invalid_glob_returns_400() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("placeholder")).unwrap();
    write_cbz(&tmp.path().join("placeholder").join("p.cbz"), 1);
    let (_lib_uuid, lib_slug) = create_library(&app, &auth, tmp.path()).await;

    let (status, body) = patch_library(
        &app,
        &auth,
        &lib_slug,
        serde_json::json!({ "ignore_globs": ["[unclosed"] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation.ignore_globs");
}

#[tokio::test]
async fn user_glob_excludes_files_during_scan() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();

    // Series Foo has two real issues plus a Promos sub-folder we want to skip.
    let foo = tmp.path().join("Series Foo (2024)");
    let promos = foo.join("Promos");
    std::fs::create_dir_all(&promos).unwrap();
    write_cbz(&foo.join("Foo 001.cbz"), 1);
    write_cbz(&foo.join("Foo 002.cbz"), 2);
    write_cbz(&promos.join("Foo Preview.cbz"), 3);

    let (lib_uuid_str, lib_slug) = create_library(&app, &auth, tmp.path()).await;

    // Apply the ignore glob.
    let (status, _) = patch_library(
        &app,
        &auth,
        &lib_slug,
        serde_json::json!({ "ignore_globs": ["**/Promos/**"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Scan; only the two top-level files should land.
    let state = app.state();
    let lib_uuid = uuid::Uuid::parse_str(&lib_uuid_str).unwrap();
    let stats = scanner::scan_library(&state, lib_uuid).await.unwrap();
    assert_eq!(stats.files_added, 2, "expected 2 added, got {stats:?}");

    let issues = IssueEntity::find().all(&state.db).await.unwrap();
    let paths: Vec<String> = issues.iter().map(|i| i.file_path.clone()).collect();
    for p in &paths {
        assert!(
            !p.contains("/Promos/"),
            "Promos path slipped through ignore: {p}",
        );
    }
}

#[tokio::test]
async fn builtin_patterns_skip_dotfiles_and_macosx() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();

    let foo = tmp.path().join("Series Foo (2024)");
    let macosx = foo.join("__MACOSX");
    let cache = foo.join(".cache");
    std::fs::create_dir_all(&macosx).unwrap();
    std::fs::create_dir_all(&cache).unwrap();
    write_cbz(&foo.join("Foo 001.cbz"), 10);
    write_cbz(&macosx.join("junk.cbz"), 11);
    write_cbz(&cache.join("hidden.cbz"), 12);
    // A bare Thumbs.db at the series root (not a CBZ — should be ignored
    // anyway, but we want to confirm the walker doesn't choke).
    std::fs::write(foo.join("Thumbs.db"), b"junk").unwrap();

    let (lib_uuid_str, _) = create_library(&app, &auth, tmp.path()).await;
    let state = app.state();
    let lib_uuid = uuid::Uuid::parse_str(&lib_uuid_str).unwrap();
    let stats = scanner::scan_library(&state, lib_uuid).await.unwrap();
    assert_eq!(stats.files_added, 1, "expected 1 added, got {stats:?}");
}
