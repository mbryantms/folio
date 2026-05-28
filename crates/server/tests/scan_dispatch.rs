//! Library Scanner v1 — Milestone 2 dispatch smoke test.
//!
//! Verifies that `POST /libraries/{id}/scan`:
//!   - returns 202 + scan_id (not the synchronous 200 it used to)
//!   - the second trigger while the first is still in-flight reuses the same
//!     scan_id and reports `coalesced: true` (spec §3.2)
//!
//! The TestApp router is built without spawning the apalis monitor, so jobs
//! land in Redis but no worker drains them. That gives us a deterministic
//! "in-flight" window for the coalescing assertion.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use entity::{library::ActiveModel as LibraryAM, scan_run::Entity as ScanRunEntity};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
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
                    r#"{"email":"scan-dispatch@example.com","password":"correctly-horse-battery"}"#,
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

/// Insert a library row directly into the DB so this test is purely about
/// the `POST /libraries/{id}/scan` dispatch behavior. The HTTP create
/// endpoint auto-enqueues an initial scan (so a freshly-created library
/// populates without a second click), which would race the in-flight
/// assertions below.
async fn create_library(app: &TestApp, _auth: &Authed) -> String {
    create_library_with_root(app, &format!("/tmp/scan-dispatch-{}", Uuid::now_v7())).await
}

async fn create_library_with_root(app: &TestApp, root_path: &str) -> String {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = chrono::Utc::now().fixed_offset();
    LibraryAM {
        id: Set(id),
        name: Set("Dispatch Lib".into()),
        root_path: Set(root_path.to_owned()),
        default_language: Set("eng".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(id.to_string()),
        scan_schedule_cron: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_scan_at: Set(None),
        ignore_globs: Set(serde_json::json!([])),
        report_missing_comicinfo: Set(false),
        file_watch_enabled: Set(true),
        soft_delete_days: Set(30),
        thumbnails_enabled: Set(true),
        thumbnail_format: Set("webp".to_owned()),
        thumbnail_cover_quality: Set(server::library::thumbnails::DEFAULT_COVER_QUALITY as i32),
        thumbnail_page_quality: Set(server::library::thumbnails::DEFAULT_STRIP_QUALITY as i32),
        generate_page_thumbs_on_scan: Set(false),
        allow_archive_writeback: Set(false),
        metadata_writeback_enabled: Set(false),
        archive_backup_retain_count: Set(1),
        archive_backup_retain_days: Set(30),
        archive_writeback_jpeg_quality: Set(92),
        cbr_convert_confirmed_at: Set(None),
        metadata_publisher_blacklist: Set(serde_json::json!([])),
        filename_ignore_leading_numbers: Set(false),
        filename_assume_issue_one: Set(false),
        metadata_auto_apply_strong_matches: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();
    id.to_string()
}

fn write_minimal_cbz(path: &Path, unique_marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&unique_marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.finish().unwrap();
}

async fn post_scan(app: &TestApp, auth: &Authed, lib_id: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/libraries/{lib_id}/scan"))
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
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[tokio::test]
async fn enqueue_returns_202_with_scan_id() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let lib_id = create_library(&app, &auth).await;

    let (status, body) = post_scan(&app, &auth, &lib_id).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(
        body["scan_id"].as_str().is_some(),
        "missing scan_id: {body}"
    );
    assert_eq!(body["state"], "queued");
    assert_eq!(body["coalesced"], false);
}

#[tokio::test]
async fn second_trigger_while_in_flight_is_coalesced() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let lib_id = create_library(&app, &auth).await;

    let (s1, b1) = post_scan(&app, &auth, &lib_id).await;
    assert_eq!(s1, StatusCode::ACCEPTED);
    let first_scan_id = b1["scan_id"].as_str().unwrap().to_owned();
    assert_eq!(b1["coalesced"], false);

    // No worker is running in TestApp, so the in_flight Redis key stays set.
    let (s2, b2) = post_scan(&app, &auth, &lib_id).await;
    assert_eq!(s2, StatusCode::ACCEPTED);
    let second_scan_id = b2["scan_id"].as_str().unwrap();
    assert_eq!(
        second_scan_id, first_scan_id,
        "coalesced trigger should reuse the in-flight scan_id"
    );
    assert_eq!(b2["coalesced"], true);
    assert_eq!(b2["state"], "coalesced");
}

async fn post_scan_all(
    app: &TestApp,
    auth: &Authed,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/admin/libraries/scan-all")
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
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[tokio::test]
async fn scan_all_enqueues_one_scan_per_library() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    // Three libraries — proves the handler iterates, not just hits the
    // first one. Different root_paths so they don't accidentally share
    // an in-flight gate via path collision.
    let lib_a = create_library_with_root(&app, "/tmp/scan-all-a").await;
    let lib_b = create_library_with_root(&app, "/tmp/scan-all-b").await;
    let lib_c = create_library_with_root(&app, "/tmp/scan-all-c").await;

    let (status, body) = post_scan_all(&app, &auth, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["total"], 3);
    assert_eq!(body["newly_enqueued"], 3);
    assert_eq!(body["already_running"], 0);
    assert_eq!(body["failed"], 0);
    assert_eq!(body["force"], false);

    let items = body["enqueued"].as_array().expect("enqueued array");
    assert_eq!(items.len(), 3);
    let library_ids: std::collections::HashSet<&str> = items
        .iter()
        .map(|i| i["library_id"].as_str().unwrap())
        .collect();
    for id in [&lib_a, &lib_b, &lib_c] {
        assert!(
            library_ids.contains(id.as_str()),
            "missing library_id {id} in enqueued list",
        );
    }
    for item in items {
        assert!(item["scan_id"].as_str().is_some(), "missing scan_id");
        assert_eq!(item["was_already_running"], false);
        assert!(item["name"].as_str().is_some());
        assert!(item["slug"].as_str().is_some());
    }
}

#[tokio::test]
async fn scan_all_coalesces_libraries_already_running() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let lib_a = create_library_with_root(&app, "/tmp/scan-all-coalesce-a").await;
    let _lib_b = create_library_with_root(&app, "/tmp/scan-all-coalesce-b").await;

    // Pre-enqueue a scan for lib_a so scan-all hits the coalesce gate
    // for that one. The TestApp has no worker, so the in-flight Redis
    // key stays set.
    let (s1, _) = post_scan(&app, &auth, &lib_a).await;
    assert_eq!(s1, StatusCode::ACCEPTED);

    let (status, body) = post_scan_all(&app, &auth, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["total"], 2);
    assert_eq!(body["newly_enqueued"], 1);
    assert_eq!(body["already_running"], 1);

    // Find lib_a's entry and assert it reports coalesced; the other
    // library should be freshly queued.
    let items = body["enqueued"].as_array().unwrap();
    let a_item = items
        .iter()
        .find(|i| i["library_id"] == lib_a)
        .expect("lib_a entry present");
    assert_eq!(a_item["was_already_running"], true);
}

#[tokio::test]
async fn scan_all_emits_audit_log_row() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let _lib = create_library_with_root(&app, "/tmp/scan-all-audit").await;

    let (status, _) = post_scan_all(&app, &auth, serde_json::json!({"force": true})).await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // The action name matches the audit.rs template (line 92) and is
    // what the bulk-action audit-check expects. Untargeted (no
    // target_type / target_id) because the action scopes "every
    // library at once."
    use entity::audit_log::{Column as AuditCol, Entity as AuditEntity};
    let row = AuditEntity::find()
        .filter(AuditCol::Action.eq("admin.libraries.scan_all"))
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("audit_log row for admin.libraries.scan_all");
    assert!(row.target_type.is_none(), "scan-all is untargeted");
    assert!(row.target_id.is_none());
    assert_eq!(row.payload["force"], true);
    assert_eq!(row.payload["total"], 1);
    assert_eq!(row.payload["newly_enqueued"], 1);
}

#[tokio::test]
async fn scan_all_requires_admin() {
    let app = TestApp::spawn().await;
    // Register a non-admin (the first registrant becomes admin, so we
    // need TWO accounts to get a non-admin).
    let _admin = register_admin(&app).await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"non-admin@example.com","password":"correctly-horse-battery"}"#,
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
    let session = cookies
        .iter()
        .find(|c| c.starts_with("__Host-comic_session="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("__Host-comic_session=")
        .to_owned();
    let csrf = cookies
        .iter()
        .find(|c| c.starts_with("__Host-comic_csrf="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("__Host-comic_csrf=")
        .to_owned();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/admin/libraries/scan-all")
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={session}; __Host-comic_csrf={csrf}"),
                )
                .header("X-CSRF-Token", &csrf)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "non-admin must get 403 from scan-all",
    );
}

#[tokio::test]
async fn returned_scan_id_is_used_by_worker_scan_run() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Dispatch Series (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    write_minimal_cbz(&folder.join("Dispatch 001.cbz"), 77);
    let lib_id = create_library_with_root(&app, &tmp.path().to_string_lossy()).await;

    let (status, body) = post_scan(&app, &auth, &lib_id).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let scan_id = Uuid::parse_str(body["scan_id"].as_str().unwrap()).unwrap();
    let library_id = Uuid::parse_str(&lib_id).unwrap();

    server::jobs::scan::handle(
        server::jobs::scan::Job {
            library_id,
            scan_run_id: scan_id,
            force: false,
        },
        apalis::prelude::Data::new(app.state()),
    )
    .await
    .expect("scan job should complete");

    let row = ScanRunEntity::find_by_id(scan_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("worker should persist scan_runs row with returned id");
    assert_eq!(row.id, scan_id);
    assert_eq!(row.library_id, library_id);
    assert_eq!(row.state, "complete");
}
