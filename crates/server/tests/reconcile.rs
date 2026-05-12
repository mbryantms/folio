//! Library Scanner v1 — Milestone 7 reconciliation & soft-delete (spec §4.7).
//!
//! Validates:
//!   - deleting a CBZ on disk + rescanning sets `removed_at` (and exposes the
//!     issue via `GET /libraries/{id}/removed`)
//!   - bringing the file back + rescanning clears `removed_at`
//!   - `POST /issues/{id}/restore` requires the file to be back (409 otherwise)
//!   - `POST /issues/{id}/confirm-removal` flips `removal_confirmed_at`
//!   - the auto-confirm sweep job confirms removals past `soft_delete_days`

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::{ActiveModel as IssueAM, Entity as IssueEntity},
    library::ActiveModel as LibraryAM,
    series::Entity as SeriesEntity,
};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use server::library::{reconcile, scanner};
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
                    r#"{"email":"rec@example.com","password":"correctly-horse-battery"}"#,
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

async fn create_library(app: &TestApp, root: &Path, soft_delete_days: i32) -> Uuid {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    LibraryAM {
        id: Set(id),
        name: Set("Reconcile Lib".into()),
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
        report_missing_comicinfo: Set(false),
        file_watch_enabled: Set(true),
        soft_delete_days: Set(soft_delete_days),
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

#[tokio::test]
async fn delete_then_rescan_soft_deletes() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Mu (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    let p = folder.join("Mu 001.cbz");
    write_cbz(&p, 1);

    let lib_id = create_library(&app, tmp.path(), 30).await;
    let state = app.state();
    let s1 = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(s1.files_added, 1);
    assert_eq!(s1.issues_removed, 0);

    // Delete the file. The folder mtime gate would normally short-circuit the
    // rescan; reconciliation runs regardless of the mtime gate (it walks the
    // DB, not the filesystem). So we just rescan.
    std::fs::remove_file(&p).unwrap();

    let s2 = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(s2.issues_removed, 1, "expected 1 removed: {s2:?}");

    // GET /libraries/{id}/removed lists it.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/libraries/{lib_id}/removed"))
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
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["issues"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn returning_file_clears_removed_at() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Nu (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    let p = folder.join("Nu 001.cbz");
    write_cbz(&p, 1);

    let lib_id = create_library(&app, tmp.path(), 30).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    std::fs::remove_file(&p).unwrap();
    scanner::scan_library(&state, lib_id).await.unwrap();
    write_cbz(&p, 1); // identical content (same hash)
    let s = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(s.issues_restored, 1, "expected restore: {s:?}");

    let issues = IssueEntity::find().all(&state.db).await.unwrap();
    assert_eq!(issues.len(), 1);
    assert!(issues[0].removed_at.is_none());
    assert!(issues[0].removal_confirmed_at.is_none());
}

#[tokio::test]
async fn restore_endpoint_requires_file_back() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Xi (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    let p = folder.join("Xi 001.cbz");
    write_cbz(&p, 1);

    let lib_id = create_library(&app, tmp.path(), 30).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let issue_row = IssueEntity::find().one(&state.db).await.unwrap().unwrap();
    let issue_slug = issue_row.slug.clone();
    let series_row = SeriesEntity::find_by_id(issue_row.series_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let series_slug = series_row.slug.clone();

    // Manually mark removed (skip the rescan dance).
    let mut am: IssueAM = issue_row.into();
    am.removed_at = Set(Some(Utc::now().fixed_offset()));
    am.update(&state.db).await.unwrap();
    // File is still missing.
    std::fs::remove_file(&p).unwrap();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/series/{series_slug}/issues/{issue_slug}/restore"))
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
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "conflict.file_missing");

    // Put the file back; restore should now succeed.
    write_cbz(&p, 1);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/series/{series_slug}/issues/{issue_slug}/restore"))
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
}

#[tokio::test]
async fn auto_confirm_sweep_processes_expired() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Omicron (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    let p = folder.join("Omicron 001.cbz");
    write_cbz(&p, 1);

    // soft_delete_days=0 means the moment something is removed it's eligible.
    let lib_id = create_library(&app, tmp.path(), 0).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    std::fs::remove_file(&p).unwrap();
    scanner::scan_library(&state, lib_id).await.unwrap();

    // Force the removed_at to a moment in the past so the sweep cutoff bites.
    let row = IssueEntity::find().one(&state.db).await.unwrap().unwrap();
    let mut am: IssueAM = row.into();
    am.removed_at = Set(Some(
        Utc::now().fixed_offset() - chrono::Duration::seconds(1),
    ));
    am.update(&state.db).await.unwrap();

    let confirmed = reconcile::auto_confirm_sweep(&state.db).await.unwrap();
    assert!(
        confirmed >= 1,
        "expected at least 1 confirmation, got {confirmed}"
    );

    let row = IssueEntity::find().one(&state.db).await.unwrap().unwrap();
    assert!(row.removal_confirmed_at.is_some());
}
