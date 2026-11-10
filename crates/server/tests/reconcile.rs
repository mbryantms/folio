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
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
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

/// First registration becomes the admin; later ones are plain readers.
async fn register_admin(app: &TestApp) -> Authed {
    register(app, "rec@example.com").await
}

async fn register(app: &TestApp, email: &str) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#
                )))
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
        auto_convert_cb7_on_scan: Set(false),
        trust_fingerprint_on_first_import: Set(false),
        auto_search_new_series: Set(false),
        show_in_sidebar_by_default: Set(true),
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
    // Mu's only file is gone, so the series itself soft-deletes and stands
    // for its issue in the listing: one series row carrying the tally, no
    // loose issue row.
    assert_eq!(body["issues"].as_array().unwrap().len(), 0, "{body}");
    let series = body["series"].as_array().unwrap();
    assert_eq!(series.len(), 1, "{body}");
    assert_eq!(series[0]["issue_count"], 1);
    assert_eq!(series[0]["pending_issue_count"], 1);
    assert_eq!(body["pending_issues"], 1);
    assert_eq!(body["pending_series"], 1);
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
    // Four files; three go missing while the fourth keeps the series alive,
    // so the removed three are *loose* issues and paginate as issue rows (a
    // removed series folds its issues into its own row instead).
    for n in 1..=4u32 {
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
    assert_eq!(page1["pending_issues"], 3);
    // The series still has a file on disk, so it is not removed itself.
    assert_eq!(page1["series"].as_array().unwrap().len(), 0);
    assert_eq!(page1["pending_series"], 0);
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

/// Soft-deleted series stay out of every list surface — the library grid /
/// search endpoint, the saved-view rails (home "Recently added"), and the
/// OPDS series catalog — for admins too. The detail endpoint 404s for
/// readers and hands admins the `removed_at` stamp (the page renders a
/// banner pointing at the Removed tab). Restoring brings the series back
/// everywhere. Prod 2026-10-22: a deleted folder's series kept rendering as
/// a blank card on "Recently added" because only the issues were filtered.
#[tokio::test]
async fn removed_series_hidden_from_lists_rails_and_opds() {
    let app = TestApp::spawn().await;
    let admin = register_admin(&app).await;
    let reader = register(&app, "reader@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Rho (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Rho 001.cbz"), 1);
    // Keeps the root non-empty once Rho's folder is parked (empty-root guard).
    let decoy = tmp.path().join("Series Sigma (2025)");
    std::fs::create_dir_all(&decoy).unwrap();
    write_cbz(&decoy.join("Sigma 001.cbz"), 9);

    let lib_id = create_library(&app, tmp.path(), 30).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    // The reader needs a grant to see the library at all.
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let reader_row = entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq("reader@example.com"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let now = Utc::now().fixed_offset();
    entity::library_user_access::ActiveModel {
        user_id: Set(reader_row.id),
        library_id: Set(lib_id),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();

    let rho = SeriesEntity::find()
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.name.contains("Rho"))
        .unwrap();
    let rho_slug = rho.slug.clone();

    let get = |auth: &Authed, uri: String| {
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
    let list_names = |body: serde_json::Value| -> Vec<String> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let series_url = format!("/api/series?library={lib_id}");
    // Built-in "Recently added series" system view (m20261205).
    let rail_url = "/api/me/saved-views/00000000-0000-0000-0000-000000000001/results".to_owned();
    let opds_url = "/opds/v1/series".to_owned();

    // Baseline: visible on all three surfaces.
    let names = list_names(body_json(get(&admin, series_url.clone()).await.into_body()).await);
    assert!(
        names.iter().any(|n| n.contains("Rho")),
        "baseline list: {names:?}"
    );
    let names = list_names(body_json(get(&admin, rail_url.clone()).await.into_body()).await);
    assert!(
        names.iter().any(|n| n.contains("Rho")),
        "baseline rail: {names:?}"
    );
    let resp = get(&admin, opds_url.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let xml = String::from_utf8(
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(xml.contains("<title>Rho</title>"), "baseline opds: {xml}");

    // Folder disappears → the series soft-deletes.
    let park = tempfile::tempdir().unwrap();
    std::fs::rename(&folder, park.path().join("Series Rho (2025)")).unwrap();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let row = SeriesEntity::find_by_id(rho.id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.removed_at.is_some(), "series should soft-delete");

    // Gone from the list, the rail and the OPDS catalog — for the admin too.
    let names = list_names(body_json(get(&admin, series_url.clone()).await.into_body()).await);
    assert!(
        !names.iter().any(|n| n.contains("Rho")) && names.iter().any(|n| n.contains("Sigma")),
        "removed series leaked into /series: {names:?}"
    );
    let names = list_names(body_json(get(&admin, rail_url.clone()).await.into_body()).await);
    assert!(
        !names.iter().any(|n| n.contains("Rho")),
        "removed series leaked into the Recently added rail: {names:?}"
    );
    let resp = get(&admin, opds_url.clone()).await;
    let xml = String::from_utf8(
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(
        !xml.contains("<title>Rho</title>"),
        "removed series leaked into OPDS: {xml}"
    );
    let names = list_names(body_json(get(&reader, series_url.clone()).await.into_body()).await);
    assert!(
        !names.iter().any(|n| n.contains("Rho")),
        "reader list: {names:?}"
    );

    // Detail: admins get the row with its removal stamp; readers get 404.
    let resp = get(&admin, format!("/api/series/{rho_slug}")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert!(
        body["removed_at"].is_string(),
        "admin detail carries removed_at: {body}"
    );
    let resp = get(&reader, format!("/api/series/{rho_slug}")).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Folder comes back + admin restores → visible again, stamp cleared.
    std::fs::rename(park.path().join("Series Rho (2025)"), &folder).unwrap();
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/series/{rho_slug}/restore"))
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        admin.session, admin.csrf
                    ),
                )
                .header("X-CSRF-Token", admin.csrf.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let names = list_names(body_json(get(&reader, series_url.clone()).await.into_body()).await);
    assert!(
        names.iter().any(|n| n.contains("Rho")),
        "restored series back in list: {names:?}"
    );
    let resp = get(&reader, format!("/api/series/{rho_slug}")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert!(
        body["removed_at"].is_null(),
        "restored detail has no stamp: {body}"
    );
}

/// Series-level confirm: one call stamps the series row and every pending
/// issue under it (the prod case was nine clicks, one per issue). Issues of
/// a removed series are folded into the series row in the listing.
#[tokio::test]
async fn confirm_series_cascades_to_its_issues() {
    let app = TestApp::spawn().await;
    let admin = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Phi (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Phi 001.cbz"), 1);
    write_cbz(&folder.join("Phi 002.cbz"), 2);
    write_cbz(&folder.join("Phi 003.cbz"), 3);
    let decoy = tmp.path().join("Series Chi (2025)");
    std::fs::create_dir_all(&decoy).unwrap();
    let loose = decoy.join("Chi 001.cbz");
    write_cbz(&loose, 9);
    write_cbz(&decoy.join("Chi 002.cbz"), 10);

    let lib_id = create_library(&app, tmp.path(), 30).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let phi = SeriesEntity::find()
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.name.contains("Phi"))
        .unwrap();

    // Phi's folder goes; one loose Chi file goes too.
    let park = tempfile::tempdir().unwrap();
    std::fs::rename(&folder, park.path().join("Series Phi (2025)")).unwrap();
    std::fs::remove_file(&loose).unwrap();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let get = |uri: String| {
        let router = app.router.clone();
        let cookie = format!("__Host-comic_session={}", admin.session);
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
    let post = |uri: String, body: Option<String>| {
        let router = app.router.clone();
        let cookie = format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            admin.session, admin.csrf
        );
        let csrf = admin.csrf.clone();
        async move {
            let mut b = Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header("X-CSRF-Token", csrf);
            let body = match body {
                Some(json) => {
                    b = b.header(header::CONTENT_TYPE, "application/json");
                    Body::from(json)
                }
                None => Body::empty(),
            };
            router.oneshot(b.body(body).unwrap()).await.unwrap()
        }
    };

    // Listing: the series row carries its issue tally; only the loose Chi
    // file appears as an issue row; pending counts span both.
    let body = body_json(
        get(format!("/api/libraries/{lib_id}/removed"))
            .await
            .into_body(),
    )
    .await;
    let series = body["series"].as_array().unwrap();
    assert_eq!(series.len(), 1, "{body}");
    assert_eq!(series[0]["issue_count"], 3);
    assert_eq!(series[0]["pending_issue_count"], 3);
    let issues = body["issues"].as_array().unwrap();
    assert_eq!(
        issues.len(),
        1,
        "Phi's issues fold into the series row: {body}"
    );
    assert!(
        issues[0]["file_path"]
            .as_str()
            .unwrap()
            .ends_with("Chi 001.cbz")
    );
    assert_eq!(body["total_issues"], 1);
    assert_eq!(body["pending_issues"], 4);
    assert_eq!(body["pending_series"], 1);

    // Not removed → 409.
    let chi_slug = SeriesEntity::find()
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.name.contains("Chi"))
        .unwrap()
        .slug;
    let resp = post(format!("/api/series/{chi_slug}/confirm-removal"), None).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // Confirm Phi: series + 3 issues in one call.
    let resp = post(format!("/api/series/{}/confirm-removal", phi.slug), None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["issues_confirmed"], 3, "{body}");
    assert_eq!(body["series_confirmed"], 1, "{body}");
    let phi_issues = IssueEntity::find()
        .filter(entity::issue::Column::SeriesId.eq(phi.id))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(phi_issues.len(), 3);
    assert!(
        phi_issues.iter().all(|i| i.removal_confirmed_at.is_some()),
        "every Phi issue confirmed"
    );
    let phi_row = SeriesEntity::find_by_id(phi.id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(phi_row.removal_confirmed_at.is_some());
    // The loose Chi file is untouched.
    let body = body_json(
        get(format!("/api/libraries/{lib_id}/removed"))
            .await
            .into_body(),
    )
    .await;
    assert_eq!(body["pending_issues"], 1, "{body}");
    assert_eq!(body["pending_series"], 0);
    assert_eq!(body["series"][0]["pending_issue_count"], 0);

    // Second call is a no-op, not an error.
    let resp = post(format!("/api/series/{}/confirm-removal", phi.slug), None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["issues_confirmed"], 0);
    assert_eq!(body["series_confirmed"], 0);

    // Audit row.
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let n = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.series.confirm_removal"))
        .all(&db)
        .await
        .unwrap()
        .len();
    assert_eq!(n, 2, "one audit row per confirm call");
}

/// Library-wide bulk confirm: a picked set (series ids cascade to their
/// issues, issue ids hit loose files), then `all` sweeps the rest. Readers
/// get 403; an empty selection is 422.
#[tokio::test]
async fn bulk_confirm_selected_then_all() {
    let app = TestApp::spawn().await;
    let admin = register_admin(&app).await;
    let reader = register(&app, "reader2@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let f1 = tmp.path().join("Series Alpha (2025)");
    let f2 = tmp.path().join("Series Beta (2025)");
    let f3 = tmp.path().join("Series Gamma (2025)");
    for f in [&f1, &f2, &f3] {
        std::fs::create_dir_all(f).unwrap();
    }
    write_cbz(&f1.join("Alpha 001.cbz"), 1);
    write_cbz(&f1.join("Alpha 002.cbz"), 2);
    write_cbz(&f2.join("Beta 001.cbz"), 3);
    let g1 = f3.join("Gamma 001.cbz");
    let g2 = f3.join("Gamma 002.cbz");
    write_cbz(&g1, 4);
    write_cbz(&g2, 5);
    write_cbz(&f3.join("Gamma 003.cbz"), 6);

    let lib_id = create_library(&app, tmp.path(), 30).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let all_series = SeriesEntity::find().all(&state.db).await.unwrap();
    let id_of = |needle: &str| {
        all_series
            .iter()
            .find(|s| s.name.contains(needle))
            .unwrap()
            .id
    };
    let (alpha, beta, gamma) = (id_of("Alpha"), id_of("Beta"), id_of("Gamma"));

    // Alpha + Beta folders vanish; two loose Gamma files vanish.
    let park = tempfile::tempdir().unwrap();
    std::fs::rename(&f1, park.path().join("a")).unwrap();
    std::fs::rename(&f2, park.path().join("b")).unwrap();
    std::fs::remove_file(&g1).unwrap();
    std::fs::remove_file(&g2).unwrap();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let gamma_removed: Vec<String> = IssueEntity::find()
        .filter(entity::issue::Column::SeriesId.eq(gamma))
        .filter(entity::issue::Column::RemovedAt.is_not_null())
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(gamma_removed.len(), 2);

    let post = |auth: &Authed, body: String| {
        let router = app.router.clone();
        let cookie = format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            auth.session, auth.csrf
        );
        let csrf = auth.csrf.clone();
        let uri = format!("/api/libraries/{lib_id}/removed/confirm");
        async move {
            router
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(uri)
                        .header(header::COOKIE, cookie)
                        .header("X-CSRF-Token", csrf)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap()
        }
    };

    // Reader → 403 before anything happens.
    let resp = post(&reader, r#"{"all":true}"#.to_owned()).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    // Nothing picked → 422.
    let resp = post(&admin, r#"{"series_ids":[],"issue_ids":[]}"#.to_owned()).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Picked: Alpha (cascades to its 2 issues) + one loose Gamma file.
    let body = serde_json::json!({
        "series_ids": [alpha],
        "issue_ids": [gamma_removed[0]],
    });
    let resp = post(&admin, body.to_string()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["issues_confirmed"], 3, "{body}");
    assert_eq!(body["series_confirmed"], 1, "{body}");
    let beta_row = SeriesEntity::find_by_id(beta)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(beta_row.removed_at.is_some() && beta_row.removal_confirmed_at.is_none());
    let g_other = IssueEntity::find_by_id(gamma_removed[1].clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(
        g_other.removal_confirmed_at.is_none(),
        "unpicked loose file stays pending"
    );

    // All: Beta (+1 issue) and the other Gamma file.
    let resp = post(&admin, r#"{"all":true}"#.to_owned()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["issues_confirmed"], 2, "{body}");
    assert_eq!(body["series_confirmed"], 1, "{body}");
    let pending = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .filter(entity::issue::Column::RemovedAt.is_not_null())
        .filter(entity::issue::Column::RemovalConfirmedAt.is_null())
        .all(&state.db)
        .await
        .unwrap();
    assert!(pending.is_empty(), "nothing pending after `all`");
    // Live Gamma 003 was never touched.
    let live = IssueEntity::find()
        .filter(entity::issue::Column::SeriesId.eq(gamma))
        .filter(entity::issue::Column::RemovedAt.is_null())
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(live.len(), 1);

    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let n = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.library.confirm_removals"))
        .all(&db)
        .await
        .unwrap()
        .len();
    assert_eq!(n, 2, "the reader's 403 and the 422 write no audit rows");
}

// ───── A returning folder restores its issues and its series ─────
//
// Moving a series folder away and back keeps its mtime, so the plan's
// unchanged-folder skip used to leave the returned issues soft-removed
// forever, and even once they were restored the series kept `removed_at`
// (hidden from every list until an admin restored it by hand).

/// A library whose "Series Tau (2025)" folder (4 issues) was scanned, read
/// by a granted reader, then parked outside the root and scanned away.
struct ParkedSeries {
    app: TestApp,
    _tmp: tempfile::TempDir,
    park: tempfile::TempDir,
    root: std::path::PathBuf,
    folder: std::path::PathBuf,
    lib_id: Uuid,
    series_id: Uuid,
    series_slug: String,
    issue_ids: std::collections::BTreeSet<String>,
    admin: Authed,
    reader: Authed,
    reader_id: Uuid,
    read_issue: String,
    old_mtime: filetime::FileTime,
}

impl ParkedSeries {
    const NAME: &'static str = "Series Tau (2025)";

    async fn new() -> Self {
        let app = TestApp::spawn().await;
        let admin = register_admin(&app).await;
        let reader = register(&app, "tau-reader@example.com").await;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let folder = root.join(Self::NAME);
        std::fs::create_dir_all(&folder).unwrap();
        // Everything an hour old, so the folder is unambiguously "older than
        // the last scan" regardless of filesystem timestamp granularity.
        let old_mtime = filetime::FileTime::from_unix_time(Utc::now().timestamp() - 3600, 0);
        for n in 1..=4u32 {
            let p = folder.join(format!("Tau {n:03}.cbz"));
            write_cbz(&p, 100 + n);
            filetime::set_file_mtime(&p, old_mtime).unwrap();
        }
        filetime::set_file_mtime(&folder, old_mtime).unwrap();
        // Keeps the root non-empty once Tau's folder is parked.
        let decoy = root.join("Series Upsilon (2025)");
        std::fs::create_dir_all(&decoy).unwrap();
        write_cbz(&decoy.join("Upsilon 001.cbz"), 9);

        let lib_id = create_library(&app, &root, 30).await;
        let state = app.state();
        scanner::scan_library(&state, lib_id).await.unwrap();

        let db = &state.db;
        let reader_id = entity::user::Entity::find()
            .filter(entity::user::Column::Email.eq("tau-reader@example.com"))
            .one(db)
            .await
            .unwrap()
            .unwrap()
            .id;
        let now = Utc::now().fixed_offset();
        entity::library_user_access::ActiveModel {
            user_id: Set(reader_id),
            library_id: Set(lib_id),
            age_rating_max: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();

        let series = SeriesEntity::find()
            .filter(entity::series::Column::Name.contains("Tau"))
            .one(db)
            .await
            .unwrap()
            .unwrap();
        let issue_ids: std::collections::BTreeSet<String> = IssueEntity::find()
            .filter(entity::issue::Column::SeriesId.eq(series.id))
            .all(db)
            .await
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(issue_ids.len(), 4);
        let read_issue = issue_ids.iter().next().unwrap().clone();
        entity::progress_record::ActiveModel {
            user_id: Set(reader_id),
            issue_id: Set(read_issue.clone()),
            last_page: Set(1),
            percent: Set(0.5),
            finished: Set(false),
            finished_at: Set(None),
            updated_at: Set(now),
            device: Set(None),
            is_backfill: Set(false),
            run: Set(0),
            page_hash: Set(None),
        }
        .insert(db)
        .await
        .unwrap();

        // The folder disappears for one scan.
        let park = tempfile::tempdir().unwrap();
        std::fs::rename(&folder, park.path().join(Self::NAME)).unwrap();
        let s = scanner::scan_library(&state, lib_id).await.unwrap();
        assert_eq!(s.issues_removed, 4, "{s:?}");
        assert_eq!(s.series_removed, 1, "{s:?}");

        let fixture = Self {
            app,
            _tmp: tmp,
            park,
            root,
            folder,
            lib_id,
            series_id: series.id,
            series_slug: series.slug,
            issue_ids,
            admin,
            reader,
            reader_id,
            read_issue,
            old_mtime,
        };
        let sr = fixture.series_row().await;
        assert!(sr.removed_at.is_some(), "series soft-removed");
        for i in fixture.series_issues().await {
            assert!(i.removed_at.is_some(), "issue {} soft-removed", i.id);
        }
        fixture
    }

    async fn series_row(&self) -> entity::series::Model {
        SeriesEntity::find_by_id(self.series_id)
            .one(&self.app.state().db)
            .await
            .unwrap()
            .unwrap()
    }

    async fn series_issues(&self) -> Vec<entity::issue::Model> {
        IssueEntity::find()
            .filter(entity::issue::Column::SeriesId.eq(self.series_id))
            .all(&self.app.state().db)
            .await
            .unwrap()
    }

    /// Put the folder back where it was, with its old mtime (a rename keeps
    /// it on most filesystems; set it explicitly so the test doesn't care).
    fn move_back(&self) {
        std::fs::rename(self.park.path().join(Self::NAME), &self.folder).unwrap();
        filetime::set_file_mtime(&self.folder, self.old_mtime).unwrap();
    }

    async fn reader_get(&self, uri: String) -> axum::response::Response {
        self.app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .header(
                        header::COOKIE,
                        format!("__Host-comic_session={}", self.reader.session),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// Issues back with the same ids, series un-removed, visible to the
    /// reader, read state intact, and a `series restored` event recorded.
    async fn assert_fully_restored(&self) {
        let issues = self.series_issues().await;
        let ids: std::collections::BTreeSet<String> = issues.iter().map(|i| i.id.clone()).collect();
        assert_eq!(ids, self.issue_ids, "same issue ids after the return");
        for i in &issues {
            assert!(i.removed_at.is_none(), "issue {} restored", i.id);
            assert!(
                i.removal_confirmed_at.is_none(),
                "issue {} unconfirmed",
                i.id
            );
        }
        let sr = self.series_row().await;
        assert!(sr.removed_at.is_none(), "series removed_at cleared");
        assert!(
            sr.removal_confirmed_at.is_none(),
            "series removal_confirmed_at cleared"
        );

        let resp = self
            .reader_get(format!("/api/series?library={}", self.lib_id))
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let names: Vec<&str> = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert!(
            names.iter().any(|n| n.contains("Tau")),
            "reader sees the returned series: {names:?}"
        );
        let resp = self
            .reader_get(format!("/api/series/{}", self.series_slug))
            .await;
        assert_eq!(resp.status(), StatusCode::OK, "reader opens the series");

        let progress =
            entity::progress_record::Entity::find_by_id((self.reader_id, self.read_issue.clone()))
                .one(&self.app.state().db)
                .await
                .unwrap();
        assert!(
            progress.is_some(),
            "reading progress survives the round trip"
        );

        let restored_events = entity::library_event::Entity::find()
            .filter(entity::library_event::Column::LibraryId.eq(self.lib_id))
            .filter(entity::library_event::Column::Category.eq("series"))
            .filter(entity::library_event::Column::Action.eq("restored"))
            .filter(entity::library_event::Column::EntityId.eq(self.series_id.to_string()))
            .all(&self.app.state().db)
            .await
            .unwrap();
        assert_eq!(restored_events.len(), 1, "one series-restored event");
    }
}

#[tokio::test]
async fn returning_folder_restores_issues_and_series_on_normal_scan() {
    let f = ParkedSeries::new().await;
    f.move_back();
    let s = scanner::scan_library(&f.app.state(), f.lib_id)
        .await
        .unwrap();
    assert_eq!(s.issues_restored, 4, "{s:?}");
    f.assert_fully_restored().await;
}

#[tokio::test]
async fn returning_folder_restores_through_watcher_scoped_scan() {
    let f = ParkedSeries::new().await;
    f.move_back();
    // The watcher reports the root (an entry appeared) and the folder.
    let s = scanner::scan_library_scoped(
        &f.app.state(),
        f.lib_id,
        &[f.root.clone(), f.folder.clone()],
        None,
    )
    .await
    .unwrap();
    assert_eq!(s.issues_restored, 4, "{s:?}");
    f.assert_fully_restored().await;
}

#[tokio::test]
async fn returning_folder_restores_through_series_scan() {
    let f = ParkedSeries::new().await;
    f.move_back();
    let s = scanner::scan_series_folder(
        &f.app.state(),
        f.lib_id,
        f.series_id,
        &f.folder,
        scanner::ScanKind::Series,
        None,
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(s.issues_restored, 4, "{s:?}");
    f.assert_fully_restored().await;
}

#[tokio::test]
async fn returning_folder_clears_a_confirmed_series_removal() {
    let f = ParkedSeries::new().await;
    let db = &f.app.state().db;
    let confirmed = Utc::now().fixed_offset();
    let mut am: entity::series::ActiveModel = f.series_row().await.into();
    am.removal_confirmed_at = Set(Some(confirmed));
    am.update(db).await.unwrap();
    for i in f.series_issues().await {
        let mut am: IssueAM = i.into();
        am.removal_confirmed_at = Set(Some(confirmed));
        am.update(db).await.unwrap();
    }

    f.move_back();
    scanner::scan_library(&f.app.state(), f.lib_id)
        .await
        .unwrap();
    f.assert_fully_restored().await;
}

/// A Duplicates-page removal is sticky (WP-3.3): when it is the only issue
/// whose file comes back, it neither restores itself nor its series.
#[tokio::test]
async fn returning_duplicate_removed_issue_keeps_series_removed() {
    let f = ParkedSeries::new().await;
    let db = &f.app.state().db;
    let pinned = f.read_issue.clone();
    entity::issue_duplicate_decision::ActiveModel {
        issue_id: Set(pinned.clone()),
        library_id: Set(f.lib_id),
        decision: Set("remove".to_owned()),
        decided_by: Set(None),
        decided_at: Set(Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .unwrap();

    // Only the pinned issue's file returns.
    f.move_back();
    for i in f.series_issues().await {
        if i.id != pinned {
            std::fs::remove_file(&i.file_path).unwrap();
        }
    }
    filetime::set_file_mtime(&f.folder, f.old_mtime).unwrap();

    let s = scanner::scan_library(&f.app.state(), f.lib_id)
        .await
        .unwrap();
    assert_eq!(s.issues_restored, 0, "{s:?}");
    for i in f.series_issues().await {
        assert!(i.removed_at.is_some(), "issue {} stays removed", i.id);
    }
    assert!(
        f.series_row().await.removed_at.is_some(),
        "series stays removed"
    );
    let resp = f.reader_get(format!("/api/series/{}", f.series_slug)).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // An admin restore of that issue drops the pin, so the series has a
    // live issue again and comes back with it.
    let issue_slug = f
        .series_issues()
        .await
        .into_iter()
        .find(|i| i.id == pinned)
        .unwrap()
        .slug;
    let resp = f
        .app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/series/{}/issues/{issue_slug}/restore",
                    f.series_slug
                ))
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        f.admin.session, f.admin.csrf
                    ),
                )
                .header("X-CSRF-Token", f.admin.csrf.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let sr = f.series_row().await;
    assert!(sr.removed_at.is_none() && sr.removal_confirmed_at.is_none());
    let resp = f.reader_get(format!("/api/series/{}", f.series_slug)).await;
    assert_eq!(resp.status(), StatusCode::OK);
}
