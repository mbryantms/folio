//! Library Scanner v1 — Milestone 5 health issue catalog (spec §10).
//!
//! Validates:
//!   - layout violations (file at root, empty folder) are persisted as rows
//!   - re-scanning the same problem upserts the existing row (no duplicates)
//!   - fixing the underlying problem auto-resolves the row on the next scan
//!   - missing ComicInfo emits a row only when `report_missing_comicinfo=true`
//!   - the `GET .../health-issues` endpoint hides resolved + dismissed by
//!     default and surfaces them when asked
//!   - `POST .../dismiss` flips dismissed_at and the issue stops appearing

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use entity::{library::ActiveModel as LibraryAM, library_health_issue::Entity as HealthEntity};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use server::library::scanner;
use std::io::Write;
use std::path::Path;
use tower::ServiceExt;
use uuid::Uuid;

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
                    r#"{"email":"hi@example.com","password":"correctly-horse-battery"}"#,
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

async fn create_library(app: &TestApp, root: &Path, report_missing: bool) -> Uuid {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = chrono::Utc::now().fixed_offset();
    LibraryAM {
        id: Set(id),
        name: Set("Health Lib".into()),
        root_path: Set(root.to_string_lossy().into_owned()),
        default_language: Set("eng".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(id.to_string()),
        scan_schedule_cron: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_scan_at: Set(None),
        ignore_globs: Set(serde_json::json!([])),
        report_missing_comicinfo: Set(report_missing),
        file_watch_enabled: Set(true),
        soft_delete_days: Set(30),
        thumbnails_enabled: Set(true),
        thumbnail_format: Set("webp".to_owned()),
        thumbnail_cover_quality: Set(server::library::thumbnails::DEFAULT_COVER_QUALITY as i32),
        thumbnail_page_quality: Set(server::library::thumbnails::DEFAULT_STRIP_QUALITY as i32),
        generate_page_thumbs_on_scan: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

async fn list_health(app: &TestApp, auth: &Authed, lib_id: Uuid, query: &str) -> serde_json::Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/libraries/{lib_id}/health-issues?{query}"))
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", auth.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp.into_body()).await
}

#[tokio::test]
async fn layout_violations_persist_and_auto_resolve() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();

    // One legit series + a stray file at root + an empty folder.
    let foo = tmp.path().join("Series Foo (2024)");
    std::fs::create_dir_all(&foo).unwrap();
    write_cbz(&foo.join("Foo 001.cbz"), 1);
    write_cbz(&tmp.path().join("orphan.cbz"), 2);
    let empty = tmp.path().join("Empty Series");
    std::fs::create_dir_all(&empty).unwrap();

    let lib_id = create_library(&app, tmp.path(), false).await;
    let state = app.state();

    scanner::scan_library(&state, lib_id).await.unwrap();

    let body = list_health(&app, &auth, lib_id, "").await;
    let issues = body.as_array().unwrap();
    let kinds: std::collections::HashSet<_> = issues
        .iter()
        .map(|v| v["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(kinds.contains("FileAtRoot"), "missing FileAtRoot: {body}");
    assert!(kinds.contains("EmptyFolder"), "missing EmptyFolder: {body}");

    // Re-scan with the orphan removed and the empty folder filled.
    std::fs::remove_file(tmp.path().join("orphan.cbz")).unwrap();
    write_cbz(&empty.join("Filler 001.cbz"), 3);

    scanner::scan_library(&state, lib_id).await.unwrap();
    let body2 = list_health(&app, &auth, lib_id, "").await;
    let issues2 = body2.as_array().unwrap();
    let kinds2: std::collections::HashSet<_> = issues2
        .iter()
        .map(|v| v["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        !kinds2.contains("FileAtRoot"),
        "FileAtRoot should auto-resolve after fixing the layout: {body2}",
    );
    assert!(
        !kinds2.contains("EmptyFolder"),
        "EmptyFolder should auto-resolve: {body2}",
    );

    // include_resolved=true brings them back.
    let body3 = list_health(&app, &auth, lib_id, "include_resolved=true").await;
    let issues3 = body3.as_array().unwrap();
    let resolved: Vec<&serde_json::Value> = issues3
        .iter()
        .filter(|v| v["resolved_at"].is_string())
        .collect();
    assert!(
        resolved.len() >= 2,
        "expected at least 2 resolved rows, got {body3}",
    );

    // No duplicates: re-running the same FileAtRoot scenario doesn't multiply.
    write_cbz(&tmp.path().join("orphan2.cbz"), 4);
    scanner::scan_library(&state, lib_id).await.unwrap();
    write_cbz(&tmp.path().join("orphan2.cbz"), 4); // identical content (same hash, same mtime path)
    scanner::scan_library(&state, lib_id).await.unwrap();
    let body4 = list_health(&app, &auth, lib_id, "").await;
    let count_orphan2: usize = body4
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| {
            v["kind"] == "FileAtRoot"
                && v["payload"]["data"]["path"]
                    .as_str()
                    .map(|p| p.ends_with("orphan2.cbz"))
                    .unwrap_or(false)
        })
        .count();
    assert_eq!(
        count_orphan2, 1,
        "FileAtRoot should upsert, not duplicate: {body4}"
    );
}

#[tokio::test]
async fn missing_comicinfo_only_when_requested() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();

    let foo = tmp.path().join("Series Bar (2024)");
    std::fs::create_dir_all(&foo).unwrap();
    write_cbz(&foo.join("Bar 001.cbz"), 1); // No ComicInfo.xml inside.

    // Default library: report_missing_comicinfo=false.
    let silent_lib = create_library(&app, tmp.path(), false).await;
    let state = app.state();
    scanner::scan_library(&state, silent_lib).await.unwrap();

    let body = list_health(&app, &auth, silent_lib, "").await;
    let kinds: std::collections::HashSet<_> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        !kinds.contains("MissingComicInfo"),
        "should not emit MissingComicInfo when flag is false: {body}",
    );

    // Same fixtures, second library, report_missing_comicinfo=true.
    let tmp2 = tempfile::tempdir().unwrap();
    let foo2 = tmp2.path().join("Series Bar (2024)");
    std::fs::create_dir_all(&foo2).unwrap();
    write_cbz(&foo2.join("Bar 001.cbz"), 1);
    let loud_lib = create_library(&app, tmp2.path(), true).await;
    scanner::scan_library(&state, loud_lib).await.unwrap();
    let body2 = list_health(&app, &auth, loud_lib, "").await;
    let kinds2: std::collections::HashSet<_> = body2
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        kinds2.contains("MissingComicInfo"),
        "should emit MissingComicInfo when flag is true: {body2}",
    );
}

#[tokio::test]
async fn dismiss_hides_issue() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();

    let foo = tmp.path().join("Series Baz (2024)");
    std::fs::create_dir_all(&foo).unwrap();
    write_cbz(&foo.join("Baz 001.cbz"), 1);
    write_cbz(&tmp.path().join("orphan.cbz"), 2);

    let lib_id = create_library(&app, tmp.path(), false).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    // Find the FileAtRoot issue id directly via the entity, then call dismiss.
    let issues = HealthEntity::find().all(&state.db).await.unwrap();
    let target = issues
        .iter()
        .find(|i| i.kind == "FileAtRoot")
        .expect("FileAtRoot row");
    let issue_id = target.id;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/libraries/{lib_id}/health-issues/{issue_id}/dismiss"
                ))
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Default list now hides the dismissed row.
    let body = list_health(&app, &auth, lib_id, "").await;
    let kinds: std::collections::HashSet<_> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(!kinds.contains("FileAtRoot"));

    // include_dismissed=true brings it back.
    let body2 = list_health(&app, &auth, lib_id, "include_dismissed=true").await;
    let dismissed = body2
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["dismissed_at"].is_string())
        .count();
    assert!(dismissed >= 1);
}
