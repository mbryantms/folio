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
        auto_convert_cbr_on_scan: Set(false),
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
                .uri(format!("/api/libraries/{lib_id}/removed"))
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
                .uri(format!(
                    "/api/series/{series_slug}/issues/{issue_slug}/restore"
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
                .uri(format!(
                    "/api/series/{series_slug}/issues/{issue_slug}/restore"
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

/// UX-11: `POST /series/{slug}/restore` — 409 while the folder is missing;
/// once the folder is back it restores the series row plus the child issues
/// whose files exist, leaving still-missing files soft-deleted.
#[tokio::test]
async fn restore_series_endpoint_restores_on_disk_issues() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Pi (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    let p1 = folder.join("Pi 001.cbz");
    let p2 = folder.join("Pi 002.cbz");
    write_cbz(&p1, 1);
    write_cbz(&p2, 2);
    // A second series keeps the library root non-empty after Pi's folder is
    // parked — the scanner refuses to scan an empty root (unmounted-storage
    // guard) and we need the removal rescan to actually run.
    let decoy = tmp.path().join("Series Tau (2025)");
    std::fs::create_dir_all(&decoy).unwrap();
    write_cbz(&decoy.join("Tau 001.cbz"), 9);

    let lib_id = create_library(&app, tmp.path(), 30).await;
    let state = app.state();
    let s = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(s.files_added, 3);
    let series_row = SeriesEntity::find()
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.name.contains("Pi"))
        .unwrap();
    let series_id = series_row.id;
    let series_slug = series_row.slug.clone();

    // Whole folder disappears → both issues + the series soft-delete. Park
    // it OUTSIDE the library root, or the scanner ingests the parked copy
    // as a new series and re-homes the content-hash-stable issues into it.
    let park = tempfile::tempdir().unwrap();
    let moved = park.path().join("Series Pi (2025)");
    std::fs::rename(&folder, &moved).unwrap();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let series_row = SeriesEntity::find_by_id(series_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(series_row.removed_at.is_some(), "series should soft-delete");

    let post_restore = |app: &TestApp, auth: &Authed, slug: String| {
        let router = app.router.clone();
        let cookie = format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            auth.session, auth.csrf
        );
        let csrf = auth.csrf.clone();
        async move {
            router
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/api/series/{slug}/restore"))
                        .header(header::COOKIE, cookie)
                        .header("X-CSRF-Token", csrf)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        }
    };

    // Folder still missing → 409.
    let resp = post_restore(&app, &auth, series_slug.clone()).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "conflict.folder_missing");

    // Folder returns, but only issue 1's file is back.
    std::fs::rename(&moved, &folder).unwrap();
    std::fs::remove_file(&p2).unwrap();

    let resp = post_restore(&app, &auth, series_slug).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let series_row = SeriesEntity::find_by_id(series_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(series_row.removed_at.is_none(), "series restored");
    let issues = IssueEntity::find().all(&state.db).await.unwrap();
    let pi_issues: Vec<_> = issues.iter().filter(|i| i.series_id == series_id).collect();
    let restored: Vec<_> = pi_issues
        .iter()
        .filter(|i| i.removed_at.is_none())
        .collect();
    assert_eq!(restored.len(), 1, "only the on-disk issue restores");
    assert_eq!(restored[0].file_path, p1.to_string_lossy());
}

/// UX-11: the removed-items list paginates by cursor without dropping rows;
/// series + `total_issues` ride on the first page only.
#[tokio::test]
async fn removed_list_paginates_by_cursor() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Rho (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    for n in 1..=3u32 {
        write_cbz(&folder.join(format!("Rho 00{n}.cbz")), n);
    }

    let lib_id = create_library(&app, tmp.path(), 30).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    for n in 1..=3u32 {
        std::fs::remove_file(folder.join(format!("Rho 00{n}.cbz"))).unwrap();
    }
    scanner::scan_library(&state, lib_id).await.unwrap();

    let get = |app: &TestApp, auth: &Authed, uri: String| {
        let router = app.router.clone();
        let cookie = format!("__Host-comic_session={}", auth.session);
        async move {
            router
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(uri)
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        }
    };

    let resp = get(
        &app,
        &auth,
        format!("/api/libraries/{lib_id}/removed?limit=2"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let page1 = body_json(resp.into_body()).await;
    assert_eq!(page1["issues"].as_array().unwrap().len(), 2);
    assert_eq!(page1["total_issues"], 3);
    // The all-issues-removed series soft-deletes and rides the first page.
    assert_eq!(page1["series"].as_array().unwrap().len(), 1);
    let cursor = page1["next_cursor"]
        .as_str()
        .expect("next_cursor")
        .to_owned();

    let resp = get(
        &app,
        &auth,
        format!("/api/libraries/{lib_id}/removed?limit=2&cursor={cursor}"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let page2 = body_json(resp.into_body()).await;
    assert_eq!(page2["issues"].as_array().unwrap().len(), 1);
    assert!(page2["next_cursor"].is_null());
    assert!(page2["total_issues"].is_null(), "total is first-page only");
    assert_eq!(page2["series"].as_array().unwrap().len(), 0);

    // No overlap between pages — cursor is exclusive.
    let id_of = |v: &serde_json::Value| v["id"].as_str().unwrap().to_owned();
    let mut seen: Vec<String> = page1["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(id_of)
        .collect();
    seen.extend(page2["issues"].as_array().unwrap().iter().map(id_of));
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 3, "pages must partition the removed set");
}

/// UX-3: `GET /issues/{id}` (bare, no /api prefix) 303-redirects to the
/// canonical slug URL so admin surfaces can link issues they only hold an
/// id for.
#[tokio::test]
async fn issue_permalink_redirects_to_canonical_url() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Sigma (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Sigma 001.cbz"), 1);

    let lib_id = create_library(&app, tmp.path(), 30).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let issue_row = IssueEntity::find().one(&state.db).await.unwrap().unwrap();
    let series_row = SeriesEntity::find_by_id(issue_row.series_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/issues/{}", issue_row.id))
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", auth.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .expect("Location header");
    assert_eq!(
        location,
        format!("/series/{}/issues/{}", series_row.slug, issue_row.slug)
    );

    // Unknown id → 404, not a proxy fallthrough.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/issues/does-not-exist")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", auth.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
