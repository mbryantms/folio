//! Library Scanner v1 — Milestone 7 reconciliation & soft-delete (spec §4.7).
//!
//! Validates:
//!   - deleting a CBZ on disk + rescanning sets `removed_at` (and exposes the
//!     issue via `GET /libraries/{id}/removed`)
//!   - bringing the file back + rescanning clears `removed_at`
//!   - `POST /issues/{id}/restore` requires the file to be back (409 otherwise)
//!   - `POST /issues/{id}/confirm-removal` flips `removal_confirmed_at`
//!   - the auto-confirm sweep job confirms removals past `soft_delete_days`,
//!     but never a row whose file is back on disk (it queues a series rescan
//!     instead) nor anything in a library whose root fails validation
//!   - the library reconcile chunks its series-id lists

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
        name: Set(format!("Reconcile Lib {id}")),
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

    let confirmed = reconcile::auto_confirm_sweep(&state)
        .await
        .unwrap()
        .confirmed;
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
        hide_unrated: Set(false),
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
            hide_unrated: Set(false),
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

// ───── Unchanged-folder skip vs. removed state ─────
//
// The planner keeps the mtime fast path and turns it off only when the walk
// finds something to restore or ingest; the series heal runs as the
// reconcile's last step, so a hidden series with live issues comes back
// without its folder being walked.

/// Write `count` archives `"{prefix} 00N.cbz"` into `folder` and pin the
/// files and the folder an hour in the past, so the folder is unambiguously
/// "older than the last scan" whatever the filesystem timestamp granularity.
fn write_old_folder(folder: &Path, prefix: &str, count: u32, marker: u32) {
    std::fs::create_dir_all(folder).unwrap();
    let old = filetime::FileTime::from_unix_time(Utc::now().timestamp() - 3600, 0);
    for n in 1..=count {
        let p = folder.join(format!("{prefix} {n:03}.cbz"));
        write_cbz(&p, marker + n);
        filetime::set_file_mtime(&p, old).unwrap();
    }
    filetime::set_file_mtime(folder, old).unwrap();
}

async fn series_named(app: &TestApp, needle: &str) -> entity::series::Model {
    SeriesEntity::find()
        .filter(entity::series::Column::Name.contains(needle))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
}

async fn issues_of(app: &TestApp, series_id: Uuid) -> Vec<entity::issue::Model> {
    IssueEntity::find()
        .filter(entity::issue::Column::SeriesId.eq(series_id))
        .all(&app.state().db)
        .await
        .unwrap()
}

async fn series_restored_events(app: &TestApp, lib_id: Uuid, series_id: Uuid) -> usize {
    entity::library_event::Entity::find()
        .filter(entity::library_event::Column::LibraryId.eq(lib_id))
        .filter(entity::library_event::Column::Category.eq("series"))
        .filter(entity::library_event::Column::Action.eq("restored"))
        .filter(entity::library_event::Column::EntityId.eq(series_id.to_string()))
        .all(&app.state().db)
        .await
        .unwrap()
        .len()
}

async fn set_series_removed(app: &TestApp, series_id: Uuid) {
    let row = SeriesEntity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::series::ActiveModel = row.into();
    am.removed_at = Set(Some(Utc::now().fixed_offset()));
    am.update(&app.state().db).await.unwrap();
}

/// A series already hidden while its issues are live (what the old scanner
/// left behind after a forced or issue scan, or an admin per-issue restore)
/// heals on the next normal scan, and its unchanged folder is still skipped.
#[tokio::test]
async fn hidden_series_with_live_issues_heals_and_keeps_the_skip() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    write_old_folder(&root.join("Series Chi (2025)"), "Chi", 3, 300);
    let lib_id = create_library(&app, &root, 30).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let series = series_named(&app, "Chi").await;
    set_series_removed(&app, series.id).await;

    let s = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(s.series_skipped_unchanged, 1, "folder still skipped: {s:?}");
    assert_eq!(s.issues_restored, 0, "{s:?}");
    let sr = SeriesEntity::find_by_id(series.id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(sr.removed_at.is_none(), "series healed");
    for i in issues_of(&app, series.id).await {
        assert!(i.removed_at.is_none(), "issue {} still live", i.id);
    }
    assert_eq!(series_restored_events(&app, lib_id, series.id).await, 1);
}

/// A permanently deleted file leaves a soft-removed row behind until hard
/// purge. It must not turn the unchanged-folder skip off for good.
#[tokio::test]
async fn permanently_deleted_file_keeps_the_unchanged_folder_skip() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let folder = root.join("Series Psi (2025)");
    write_old_folder(&folder, "Psi", 3, 400);
    let lib_id = create_library(&app, &root, 30).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let series = series_named(&app, "Psi").await;

    // The delete moves the folder mtime past the last scan.
    let gone = folder.join("Psi 002.cbz");
    std::fs::remove_file(&gone).unwrap();
    let s = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(s.issues_removed, 1, "{s:?}");
    assert_eq!(s.series_skipped_unchanged, 0, "{s:?}");

    let s = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(s.series_skipped_unchanged, 1, "folder skipped again: {s:?}");
    assert_eq!(s.issues_restored + s.issues_removed, 0, "{s:?}");
    let gone = gone.to_string_lossy().into_owned();
    for i in issues_of(&app, series.id).await {
        assert_eq!(
            i.removed_at.is_some(),
            i.file_path == gone,
            "issue {} ({})",
            i.id,
            i.file_path
        );
    }
    let sr = SeriesEntity::find_by_id(series.id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(sr.removed_at.is_none(), "series stays live");
}

/// A removed series whose only issue is a sticky Duplicates-page removal
/// stays removed, and its unchanged folder is skipped.
#[tokio::test]
async fn duplicate_removed_only_series_stays_removed_and_skipped() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    write_old_folder(&root.join("Series Omega (2025)"), "Omega", 1, 500);
    let lib_id = create_library(&app, &root, 30).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let series = series_named(&app, "Omega").await;
    let issue = issues_of(&app, series.id).await.remove(0);
    let now = Utc::now().fixed_offset();
    entity::issue_duplicate_decision::ActiveModel {
        issue_id: Set(issue.id.clone()),
        library_id: Set(lib_id),
        decision: Set("remove".to_owned()),
        decided_by: Set(None),
        decided_at: Set(now),
    }
    .insert(&state.db)
    .await
    .unwrap();
    let mut am: IssueAM = issue.into();
    am.removed_at = Set(Some(now));
    am.update(&state.db).await.unwrap();
    set_series_removed(&app, series.id).await;

    let s = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(s.series_skipped_unchanged, 1, "{s:?}");
    assert_eq!(s.issues_restored, 0, "{s:?}");
    for i in issues_of(&app, series.id).await {
        assert!(i.removed_at.is_some(), "issue {} stays removed", i.id);
    }
    let sr = SeriesEntity::find_by_id(series.id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(sr.removed_at.is_some(), "series stays removed");
    assert_eq!(series_restored_events(&app, lib_id, series.id).await, 0);
}

/// A removed series whose folder holds archives the database has no row
/// for (here: the rows were purged) is not skipped: the files are ingested
/// again and the series comes back.
#[tokio::test]
async fn removed_series_with_unknown_archives_is_processed() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    write_old_folder(&root.join("Series Sigma (2025)"), "Sigma", 2, 600);
    let lib_id = create_library(&app, &root, 30).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let series = series_named(&app, "Sigma").await;
    IssueEntity::delete_many()
        .filter(entity::issue::Column::SeriesId.eq(series.id))
        .exec(&state.db)
        .await
        .unwrap();
    set_series_removed(&app, series.id).await;

    let s = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(s.series_skipped_unchanged, 0, "{s:?}");
    assert_eq!(s.files_added, 2, "{s:?}");
    let live = issues_of(&app, series.id).await;
    assert_eq!(live.len(), 2);
    assert!(live.iter().all(|i| i.removed_at.is_none()));
    let sr = SeriesEntity::find_by_id(series.id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(sr.removed_at.is_none(), "series back with its files");
}

/// No flap: a removed series whose recorded folder is missing while one of
/// its removed issues sits in a scanned, present folder. The reconcile
/// restores that issue, the folder-missing sweep removes it again, and the
/// series heal runs after the sweep, so the series is never restored (and
/// removed again) inside the one reconcile. Drives `reconcile_library_seen`
/// directly: a full scan re-homes the series onto the present folder first.
#[tokio::test]
async fn series_with_missing_folder_does_not_flap_in_one_reconcile() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let folder = root.join("Series Rho (2025)");
    write_old_folder(&folder, "Rho", 2, 700);
    let lib_id = create_library(&app, &root, 30).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let series = series_named(&app, "Rho").await;
    let issues = issues_of(&app, series.id).await;

    // The series' recorded folder is gone and the last scan removed it; one
    // of its issues (file present in the scanned folder) is removed too.
    let mut am: entity::series::ActiveModel = series.clone().into();
    am.folder_path = Set(Some(
        root.join("Gone Rho (2025)").to_string_lossy().into_owned(),
    ));
    am.removed_at = Set(Some(Utc::now().fixed_offset()));
    am.update(&state.db).await.unwrap();
    let mut am: IssueAM = issues[0].clone().into();
    am.removed_at = Set(Some(Utc::now().fixed_offset()));
    am.update(&state.db).await.unwrap();

    let present = folder.to_string_lossy().into_owned();
    let folders: std::collections::HashSet<String> = [present].into_iter().collect();
    let seen: std::collections::HashSet<String> =
        issues.iter().map(|i| i.file_path.clone()).collect();
    let scanned: std::collections::HashSet<Uuid> = [series.id].into_iter().collect();
    let mut stats = scanner::ScanStats::default();
    let mut events = server::library::event_log::EventCollector::new(lib_id, None);
    reconcile::reconcile_library_seen(
        &state.db,
        lib_id,
        &scanned,
        &seen,
        &folders,
        &folders,
        None,
        &std::collections::HashSet::new(),
        &mut stats,
        &mut events,
    )
    .await
    .unwrap();
    events.flush(&state.db).await;

    let sr = SeriesEntity::find_by_id(series.id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(sr.removed_at.is_some(), "series stays removed");
    assert_eq!(
        stats.series_removed, 0,
        "never restored, so never re-removed: {stats:?}"
    );
    assert_eq!(series_restored_events(&app, lib_id, series.id).await, 0);
}

// ───── Removal guards: confirm only what is really gone ─────

/// Stamp `removed_at` `days` ago (unconfirmed) on the issue at `path`.
async fn mark_removed_days_ago(app: &TestApp, path: &Path, days: i64) -> entity::issue::Model {
    let db = &app.state().db;
    let row = IssueEntity::find()
        .filter(entity::issue::Column::FilePath.eq(path.to_string_lossy().into_owned()))
        .one(db)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("no issue at {}", path.display()));
    let mut am: IssueAM = row.into();
    am.removed_at = Set(Some(
        Utc::now().fixed_offset() - chrono::Duration::days(days),
    ));
    am.removal_confirmed_at = Set(None);
    am.update(db).await.unwrap()
}

async fn issue_row(app: &TestApp, id: &str) -> entity::issue::Model {
    IssueEntity::find_by_id(id.to_owned())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
}

/// Series-scoped scan runs queued (or run) for `series_id`.
async fn series_scan_runs(app: &TestApp, series_id: Uuid) -> usize {
    entity::scan_run::Entity::find()
        .filter(entity::scan_run::Column::SeriesId.eq(series_id))
        .filter(entity::scan_run::Column::Kind.eq(entity::scan_run::ScanRunKind::Series))
        .all(&app.state().db)
        .await
        .unwrap()
        .len()
}

/// A soft-removed issue past the window whose file is back on disk is not
/// confirmed and its series gets one rescan; a file that is really gone is
/// confirmed; a symlink at the stored path is not "present" (a scan would
/// never index it) so it is confirmed too.
#[tokio::test]
async fn auto_confirm_skips_files_back_on_disk_and_queues_a_rescan() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Sigma (2025)");
    write_old_folder(&folder, "Sigma", 3, 900);
    let lib_id = create_library(&app, tmp.path(), 1).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let series = series_named(&app, "Sigma").await;

    // 001: removed by a scan that could not see it, file present again.
    let back = mark_removed_days_ago(&app, &folder.join("Sigma 001.cbz"), 5).await;
    // 002: really gone.
    let gone_path = folder.join("Sigma 002.cbz");
    std::fs::remove_file(&gone_path).unwrap();
    let gone = mark_removed_days_ago(&app, &gone_path, 5).await;
    // 003: replaced by a symlink to a real archive outside the library.
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("elsewhere.cbz");
    write_cbz(&target, 999);
    let link_path = folder.join("Sigma 003.cbz");
    std::fs::remove_file(&link_path).unwrap();
    std::os::unix::fs::symlink(&target, &link_path).unwrap();
    let linked = mark_removed_days_ago(&app, &link_path, 5).await;
    age_series_scan(&app, series.id).await;
    assert_eq!(series_scan_runs(&app, series.id).await, 0);

    let stats = reconcile::auto_confirm_sweep(&state).await.unwrap();
    assert_eq!(stats.confirmed, 2, "{stats:?}");
    assert_eq!(stats.kept_present, 1, "{stats:?}");
    assert_eq!(stats.rescans_queued, 1, "{stats:?}");
    assert_eq!(stats.libraries_skipped, 0, "{stats:?}");

    let back = issue_row(&app, &back.id).await;
    assert!(back.removed_at.is_some(), "still removed until the rescan");
    assert!(
        back.removal_confirmed_at.is_none(),
        "file on disk: not confirmed"
    );
    assert!(
        issue_row(&app, &gone.id)
            .await
            .removal_confirmed_at
            .is_some(),
        "absent file: confirmed"
    );
    assert!(
        issue_row(&app, &linked.id)
            .await
            .removal_confirmed_at
            .is_some(),
        "a symlink is never indexed, so it is not present"
    );
    assert_eq!(
        series_scan_runs(&app, series.id).await,
        1,
        "one scoped rescan queued for the series"
    );

    // A second run before the rescan ran coalesces into the queued one.
    let stats = reconcile::auto_confirm_sweep(&state).await.unwrap();
    assert_eq!(stats.confirmed, 0, "{stats:?}");
    assert_eq!(stats.kept_present, 1, "{stats:?}");
    assert_eq!(series_scan_runs(&app, series.id).await, 1, "coalesced");

    // The normal restore path brings the issue back.
    scanner::scan_library(&state, lib_id).await.unwrap();
    let back = issue_row(&app, &back.id).await;
    assert!(back.removed_at.is_none(), "restored by the rescan");
}

/// A library whose root is unavailable is skipped whole (nothing confirmed,
/// one skipped event); another library in the same run is swept normally.
#[tokio::test]
async fn auto_confirm_skips_a_library_whose_root_is_missing() {
    let app = TestApp::spawn().await;
    let state = app.state();

    let parent = tempfile::tempdir().unwrap();
    let root_a = parent.path().join("lib-a");
    let folder_a = root_a.join("Series Upsilon (2025)");
    write_old_folder(&folder_a, "Upsilon", 2, 910);
    let lib_a = create_library(&app, &root_a, 1).await;
    scanner::scan_library(&state, lib_a).await.unwrap();

    let tmp_b = tempfile::tempdir().unwrap();
    let folder_b = tmp_b.path().join("Series Phi (2025)");
    write_old_folder(&folder_b, "Phi", 2, 920);
    let lib_b = create_library(&app, tmp_b.path(), 1).await;
    scanner::scan_library(&state, lib_b).await.unwrap();

    // Both libraries hold a removal past the window whose file is gone.
    let a1 = folder_a.join("Upsilon 001.cbz");
    std::fs::remove_file(&a1).unwrap();
    let a1 = mark_removed_days_ago(&app, &a1, 5).await;
    let b1 = folder_b.join("Phi 001.cbz");
    std::fs::remove_file(&b1).unwrap();
    let b1 = mark_removed_days_ago(&app, &b1, 5).await;

    // Library A's root disappears (an unmounted share), and so every one
    // of its files looks absent.
    std::fs::rename(&root_a, parent.path().join("lib-a-unmounted")).unwrap();
    let a2 = mark_removed_days_ago(&app, &folder_a.join("Upsilon 002.cbz"), 5).await;

    let stats = reconcile::auto_confirm_sweep(&state).await.unwrap();
    assert_eq!(stats.libraries_skipped, 1, "{stats:?}");
    assert_eq!(stats.confirmed, 1, "only library B's row: {stats:?}");
    for id in [&a1.id, &a2.id] {
        assert!(
            issue_row(&app, id).await.removal_confirmed_at.is_none(),
            "library A skipped: nothing confirmed"
        );
    }
    assert!(issue_row(&app, &b1.id).await.removal_confirmed_at.is_some());

    let skipped = entity::library_event::Entity::find()
        .filter(entity::library_event::Column::LibraryId.eq(lib_a))
        .filter(entity::library_event::Column::Action.eq("skipped"))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(skipped.len(), 1, "one skipped event: {skipped:?}");
    assert!(
        skipped[0].summary.contains("library root unavailable"),
        "{}",
        skipped[0].summary
    );
}

/// Snapshot `(issue removed?, series removed?)` keyed by file name /
/// series name, for comparing two libraries with the same layout.
async fn removal_snapshot(
    app: &TestApp,
    lib_id: Uuid,
) -> (Vec<(String, bool)>, Vec<(String, bool)>) {
    let db = &app.state().db;
    let mut issues: Vec<(String, bool)> = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|i| {
            let name = Path::new(&i.file_path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (name, i.removed_at.is_some())
        })
        .collect();
    issues.sort();
    let mut series: Vec<(String, bool)> = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|s| (s.name, s.removed_at.is_some()))
        .collect();
    series.sort();
    (issues, series)
}

/// The library reconcile binds its series-id lists in chunks. With a chunk
/// of 2 over five series (three chunks) it reaches the same state as the
/// default chunk: a missing file removed, an emptied series removed, and a
/// hidden series holding live issues healed.
#[tokio::test]
async fn library_reconcile_chunks_series_id_lists() {
    let app = TestApp::spawn().await;

    async fn run(
        app: &TestApp,
        chunk: Option<usize>,
    ) -> (Vec<(String, bool)>, Vec<(String, bool)>) {
        let state = app.state();
        let tmp = tempfile::tempdir().unwrap();
        let names = ["Alef", "Bet", "Gimel", "Dalet", "He"];
        for (n, name) in names.iter().enumerate() {
            write_old_folder(
                &tmp.path().join(format!("Series {name} (2025)")),
                name,
                2,
                1000 + 10 * n as u32,
            );
        }
        let lib_id = create_library(app, tmp.path(), 30).await;
        scanner::scan_library(&state, lib_id).await.unwrap();

        // Alef loses one file; Bet loses both; Gimel is hidden with live
        // issues (heal); Dalet and He are untouched.
        std::fs::remove_file(tmp.path().join("Series Alef (2025)/Alef 001.cbz")).unwrap();
        std::fs::remove_file(tmp.path().join("Series Bet (2025)/Bet 001.cbz")).unwrap();
        std::fs::remove_file(tmp.path().join("Series Bet (2025)/Bet 002.cbz")).unwrap();
        let gimel = SeriesEntity::find()
            .filter(entity::series::Column::LibraryId.eq(lib_id))
            .filter(entity::series::Column::Name.contains("Gimel"))
            .one(&state.db)
            .await
            .unwrap()
            .unwrap();
        set_series_removed(app, gimel.id).await;

        let series_rows = SeriesEntity::find()
            .filter(entity::series::Column::LibraryId.eq(lib_id))
            .all(&state.db)
            .await
            .unwrap();
        assert_eq!(series_rows.len(), 5);
        let scanned: std::collections::HashSet<Uuid> = series_rows.iter().map(|s| s.id).collect();
        let folders: std::collections::HashSet<String> = series_rows
            .iter()
            .filter_map(|s| s.folder_path.clone())
            .collect();
        let seen: std::collections::HashSet<String> = IssueEntity::find()
            .filter(entity::issue::Column::LibraryId.eq(lib_id))
            .all(&state.db)
            .await
            .unwrap()
            .into_iter()
            .map(|i| i.file_path)
            .filter(|p| Path::new(p).exists())
            .collect();
        let mut stats = scanner::ScanStats::default();
        let mut events = server::library::event_log::EventCollector::new(lib_id, None);
        let unreadable = std::collections::HashSet::new();
        let restored = match chunk {
            Some(c) => reconcile::reconcile_library_seen_with_chunk(
                &state.db,
                lib_id,
                &scanned,
                &seen,
                &folders,
                &folders,
                None,
                &unreadable,
                &mut stats,
                &mut events,
                c,
            )
            .await
            .unwrap(),
            None => reconcile::reconcile_library_seen(
                &state.db,
                lib_id,
                &scanned,
                &seen,
                &folders,
                &folders,
                None,
                &unreadable,
                &mut stats,
                &mut events,
            )
            .await
            .unwrap(),
        };
        events.flush(&state.db).await;
        assert_eq!(restored, 1, "Gimel healed (chunk {chunk:?})");
        assert_eq!(stats.issues_removed, 3, "{stats:?}");
        assert_eq!(stats.series_removed, 1, "{stats:?}");
        removal_snapshot(app, lib_id).await
    }

    let chunked = run(&app, Some(2)).await;
    let unchunked = run(&app, None).await;
    assert_eq!(chunked, unchunked);
    let (issues, series) = chunked;
    assert_eq!(
        issues.iter().filter(|(_, removed)| *removed).count(),
        3,
        "{issues:?}"
    );
    let removed: Vec<&str> = series
        .iter()
        .filter(|(_, removed)| *removed)
        .map(|(n, _)| n.as_str())
        .collect();
    assert!(
        removed.len() == 1 && removed[0].contains("Bet"),
        "only the emptied series: {series:?}"
    );
}

// ───── Unknown is not gone, bounds, throttle ─────

async fn sql(app: &TestApp, q: &str, values: Vec<sea_orm::Value>) {
    use sea_orm::ConnectionTrait;
    app.state()
        .db
        .execute_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            q,
            values,
        ))
        .await
        .unwrap_or_else(|e| panic!("{q}: {e}"));
}

/// Pretend the series was last scanned a week ago, past the sweep's
/// rescan throttle.
async fn age_series_scan(app: &TestApp, series_id: Uuid) {
    sql(
        app,
        "UPDATE series SET last_scanned_at = now() - interval '7 days' WHERE id = $1",
        vec![series_id.into()],
    )
    .await;
}

async fn library_scan_runs(app: &TestApp, lib_id: Uuid) -> usize {
    entity::scan_run::Entity::find()
        .filter(entity::scan_run::Column::LibraryId.eq(lib_id))
        .filter(entity::scan_run::Column::Kind.eq(entity::scan_run::ScanRunKind::Library))
        .all(&app.state().db)
        .await
        .unwrap()
        .len()
}

/// Restores a directory's mode when dropped, so a failed assertion still
/// lets the tempdir clean up.
struct ModeGuard(std::path::PathBuf);
impl Drop for ModeGuard {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

/// Make `dir` unreadable. Returns `None` (the caller skips the test) when
/// the process can still read it, i.e. when running as root.
fn lock_dir(dir: &Path) -> Option<ModeGuard> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    let guard = ModeGuard(dir.to_path_buf());
    if std::fs::read_dir(dir).is_ok() {
        // Skipped: running with privileges (root) that ignore directory
        // modes, so the folder cannot be made unreadable.
        return None;
    }
    Some(guard)
}

/// An issue whose series folder cannot be read is unknown, not gone: the
/// confirm sweep keeps it unconfirmed and queues no rescan. A file under a
/// readable folder that is really gone is still confirmed.
#[tokio::test]
async fn auto_confirm_keeps_rows_it_cannot_read() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let locked = tmp.path().join("Series Chi (2025)");
    let open = tmp.path().join("Series Psi (2025)");
    write_old_folder(&locked, "Chi", 2, 1100);
    write_old_folder(&open, "Psi", 2, 1110);
    let lib_id = create_library(&app, tmp.path(), 1).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let chi = series_named(&app, "Chi").await;
    age_series_scan(&app, chi.id).await;

    let unreadable = mark_removed_days_ago(&app, &locked.join("Chi 001.cbz"), 5).await;
    let gone_path = open.join("Psi 001.cbz");
    std::fs::remove_file(&gone_path).unwrap();
    let gone = mark_removed_days_ago(&app, &gone_path, 5).await;
    let Some(_guard) = lock_dir(&locked) else {
        return;
    };

    let stats = reconcile::auto_confirm_sweep(&state).await.unwrap();
    assert_eq!(stats.kept_unknown, 1, "{stats:?}");
    assert_eq!(stats.kept_present, 0, "{stats:?}");
    assert_eq!(stats.confirmed, 1, "{stats:?}");
    assert_eq!(stats.rescans_queued, 0, "unknown is never rescanned");
    assert!(
        issue_row(&app, &unreadable.id)
            .await
            .removal_confirmed_at
            .is_none()
    );
    assert!(
        issue_row(&app, &gone.id)
            .await
            .removal_confirmed_at
            .is_some()
    );
    assert_eq!(series_scan_runs(&app, chi.id).await, 0);
}

/// The batch helper refuses a root that is empty (an unmounted share's
/// mount point): nothing from the batch may be acted on. With the root
/// populated it answers per path.
#[tokio::test]
async fn probe_batch_refuses_an_empty_root() {
    let ignore = server::library::ignore::IgnoreRules::default();
    let empty = tempfile::tempdir().unwrap();
    let ghost = empty
        .path()
        .join("Series Gone (2025)/Gone 001.cbz")
        .to_string_lossy()
        .into_owned();
    let err = reconcile::probe_files_blocking(empty.path(), &ignore, &[ghost]).unwrap_err();
    assert!(
        err.downcast_ref::<reconcile::RootVanished>().is_some(),
        "{err}"
    );
    let err = reconcile::probe_folders_blocking(
        empty.path(),
        &ignore,
        &[empty
            .path()
            .join("Series Gone (2025)")
            .to_string_lossy()
            .into_owned()],
        &std::collections::HashSet::new(),
    )
    .unwrap_err();
    assert!(err.downcast_ref::<reconcile::RootVanished>().is_some());

    let full = tempfile::tempdir().unwrap();
    let folder = full.path().join("Series Here (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    let here = folder.join("Here 001.cbz");
    write_cbz(&here, 1);
    let paths = [
        here.to_string_lossy().into_owned(),
        folder.join("Here 002.cbz").to_string_lossy().into_owned(),
        full.path()
            .join("Series Nope (2025)/Nope 001.cbz")
            .to_string_lossy()
            .into_owned(),
    ];
    let got = reconcile::probe_files_blocking(full.path(), &ignore, &paths).unwrap();
    assert_eq!(
        got,
        vec![
            reconcile::Presence::Present,
            reconcile::Presence::Absent,
            reconcile::Presence::Absent,
        ]
    );
}

/// The series branch: a removed series (no issue rows left) whose folder
/// holds an archive again is not confirmed and gets a rescan.
#[tokio::test]
async fn auto_confirm_keeps_a_series_whose_folder_holds_archives() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Lambda (2025)");
    write_old_folder(&folder, "Lambda", 1, 1200);
    let lib_id = create_library(&app, tmp.path(), 1).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let lambda = series_named(&app, "Lambda").await;
    sql(
        &app,
        "DELETE FROM issues WHERE series_id = $1",
        vec![lambda.id.into()],
    )
    .await;
    sql(
        &app,
        "UPDATE series SET removed_at = now() - interval '5 days', \
         last_scanned_at = now() - interval '7 days' WHERE id = $1",
        vec![lambda.id.into()],
    )
    .await;

    let stats = reconcile::auto_confirm_sweep(&state).await.unwrap();
    assert_eq!(stats.confirmed, 0, "{stats:?}");
    assert_eq!(stats.kept_present, 1, "{stats:?}");
    assert_eq!(stats.rescans_queued, 1, "{stats:?}");
    let row = SeriesEntity::find_by_id(lambda.id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.removal_confirmed_at.is_none());
    assert_eq!(series_scan_runs(&app, lambda.id).await, 1);
}

/// A Duplicates-page removal is deliberate: both sweeps act on it without
/// a stat although its file is on disk.
#[tokio::test]
async fn duplicate_removal_is_confirmed_and_purged_without_a_stat() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Omega (2025)");
    write_old_folder(&folder, "Omega", 2, 1300);
    let lib_id = create_library(&app, tmp.path(), 1).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let dup = mark_removed_days_ago(&app, &folder.join("Omega 001.cbz"), 5).await;
    sql(
        &app,
        "INSERT INTO issue_duplicate_decision (issue_id, library_id, decision, decided_at) \
         VALUES ($1, $2, 'remove', now())",
        vec![dup.id.clone().into(), lib_id.into()],
    )
    .await;

    let stats = reconcile::auto_confirm_sweep(&state).await.unwrap();
    assert_eq!(stats.confirmed, 1, "{stats:?}");
    assert_eq!(stats.kept_present, 0, "{stats:?}");
    assert!(
        issue_row(&app, &dup.id)
            .await
            .removal_confirmed_at
            .is_some()
    );

    // Purge window: 1 day × 2. Age the confirmation past it.
    sql(
        &app,
        "UPDATE issues SET removal_confirmed_at = now() - interval '10 days' WHERE id = $1",
        vec![dup.id.clone().into()],
    )
    .await;
    let stats = server::jobs::hard_purge::run_with(&state, 2, Utc::now().fixed_offset())
        .await
        .unwrap();
    assert_eq!(stats.issues, 1, "{stats:?}");
    assert_eq!(stats.kept_present, 0, "{stats:?}");
    assert!(
        IssueEntity::find_by_id(dup.id.clone())
            .one(&state.db)
            .await
            .unwrap()
            .is_none()
    );
}

/// Keyset paging: rows kept at the head of the queue do not block the rest.
/// Page 2, at most 2 confirmations a run: the two oldest (present) rows are
/// kept at no cost to the budget, the next two (gone) are confirmed across
/// the page boundary, the fifth waits for the next run, which confirms it.
#[tokio::test]
async fn auto_confirm_pages_past_kept_rows() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Kappa (2025)");
    write_old_folder(&folder, "Kappa", 5, 1400);
    let lib_id = create_library(&app, tmp.path(), 1).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let kappa = series_named(&app, "Kappa").await;
    age_series_scan(&app, kappa.id).await;

    let mut rows = Vec::new();
    for n in 1..=5u32 {
        let p = folder.join(format!("Kappa {n:03}.cbz"));
        if n > 2 {
            std::fs::remove_file(&p).unwrap();
        }
        rows.push(mark_removed_days_ago(&app, &p, 20 - i64::from(n)).await);
    }
    let limits = reconcile::SweepLimits {
        page: 2,
        max_issue_confirms: 2,
        ..reconcile::SweepLimits::default()
    };

    let stats = reconcile::auto_confirm_sweep_with(&state, limits)
        .await
        .unwrap();
    assert_eq!(stats.kept_present, 2, "{stats:?}");
    assert_eq!(stats.confirmed, 2, "{stats:?}");
    let confirmed = |m: &entity::issue::Model| m.removal_confirmed_at.is_some();
    let now: Vec<bool> = {
        let mut v = Vec::new();
        for r in &rows {
            v.push(confirmed(&issue_row(&app, &r.id).await));
        }
        v
    };
    assert_eq!(now, vec![false, false, true, true, false]);

    let stats = reconcile::auto_confirm_sweep_with(&state, limits)
        .await
        .unwrap();
    assert_eq!(stats.confirmed, 1, "the fifth row next run: {stats:?}");
    assert!(confirmed(&issue_row(&app, &rows[4].id).await));
}

// ───── Confirm sweep paging: rows it cannot confirm do not starve the rest ─────

/// A library for the confirm-paging tests: one live folder (so the root
/// validates) holding the live series `Holder`, and a folder link `Loop`
/// that points at itself. Every stat through `Loop` fails with `ELOOP`,
/// which is not the not-found class, so a row stored under it is unknown
/// (never confirmed); unlike a chmod-ed folder this holds when the tests run
/// as root. Returns `(library id, Holder's id)`.
async fn confirm_paging_fixture(app: &TestApp, root: &Path) -> (Uuid, Uuid) {
    let holder = root.join("Holder (2020)");
    write_old_folder(&holder, "Holder", 1, 1700);
    let lib_id = create_library(app, root, 1).await;
    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    std::os::unix::fs::symlink("Loop", root.join("Loop")).unwrap();
    let holder = series_named(app, "Holder").await;
    (lib_id, holder.id)
}

/// `n` fresh ids in ascending order, so a test can place rows that share a
/// `removed_at` on either side of a page boundary.
fn sorted_ids(n: usize) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = (0..n).map(|_| Uuid::new_v4()).collect();
    ids.sort();
    ids
}

/// Insert an issue of `series_id` stored at `path`, soft-removed at `at`
/// and unconfirmed. Returns its id.
async fn insert_removed_issue(
    app: &TestApp,
    lib_id: Uuid,
    series_id: Uuid,
    id: Uuid,
    path: &Path,
    at: chrono::DateTime<chrono::FixedOffset>,
) -> String {
    let id = id.to_string();
    sql(
        app,
        "INSERT INTO issues (id, library_id, series_id, file_path, file_size, file_mtime, \
                             content_hash, slug, removed_at) \
         VALUES ($1, $2, $3, $4, 1, now(), $1, $1, $5)",
        vec![
            id.clone().into(),
            lib_id.into(),
            series_id.into(),
            path.to_string_lossy().into_owned().into(),
            at.into(),
        ],
    )
    .await;
    id
}

/// Insert a series with folder `folder`, soft-removed at `at` and
/// unconfirmed.
async fn insert_removed_series(
    app: &TestApp,
    lib_id: Uuid,
    id: Uuid,
    folder: &Path,
    at: chrono::DateTime<chrono::FixedOffset>,
) -> Uuid {
    sql(
        app,
        "INSERT INTO series (id, library_id, name, normalized_name, slug, folder_path, removed_at) \
         VALUES ($1, $2, $3, $3, $3, $4, $5)",
        vec![
            id.into(),
            lib_id.into(),
            format!("Paging {id}").into(),
            folder.to_string_lossy().into_owned().into(),
            at.into(),
        ],
    )
    .await;
    id
}

/// Issues for the paging tests: `unknown` rows (stored under `Loop`)
/// removed 100, 99, … days ago, ahead of `gone` rows (their folder does not
/// exist) removed 50, 49, … days ago; or, with `same_time`, all removed at
/// one instant, unknown rows first in `id` order. Returns `(unknown, gone)`.
async fn paging_issues(
    app: &TestApp,
    root: &Path,
    unknown: usize,
    gone: usize,
    same_time: bool,
) -> (Vec<String>, Vec<String>) {
    let (lib_id, holder) = confirm_paging_fixture(app, root).await;
    let now = Utc::now().fixed_offset();
    let at = |days: usize| {
        if same_time {
            now - chrono::Duration::days(40)
        } else {
            now - chrono::Duration::days(days as i64)
        }
    };
    let ids = sorted_ids(unknown + gone);
    let mut unknown_ids = Vec::new();
    for (k, id) in ids[..unknown].iter().enumerate() {
        let path = root.join("Loop").join(format!("Unknown {k:03}.cbz"));
        unknown_ids.push(insert_removed_issue(app, lib_id, holder, *id, &path, at(100 - k)).await);
    }
    let mut gone_ids = Vec::new();
    for (k, id) in ids[unknown..].iter().enumerate() {
        let path = root.join("Gone (2021)").join(format!("Gone {k:03}.cbz"));
        gone_ids.push(insert_removed_issue(app, lib_id, holder, *id, &path, at(50 - k)).await);
    }
    (unknown_ids, gone_ids)
}

/// The same for series: unknown series have their folder under `Loop`,
/// gone series a folder that does not exist.
async fn paging_series(
    app: &TestApp,
    root: &Path,
    unknown: usize,
    gone: usize,
    same_time: bool,
) -> (Vec<Uuid>, Vec<Uuid>) {
    let (lib_id, _) = confirm_paging_fixture(app, root).await;
    let now = Utc::now().fixed_offset();
    let at = |days: usize| {
        if same_time {
            now - chrono::Duration::days(40)
        } else {
            now - chrono::Duration::days(days as i64)
        }
    };
    let ids = sorted_ids(unknown + gone);
    let mut unknown_ids = Vec::new();
    for (k, id) in ids[..unknown].iter().enumerate() {
        let folder = root.join("Loop").join(format!("Unknown Series {k:03}"));
        unknown_ids.push(insert_removed_series(app, lib_id, *id, &folder, at(100 - k)).await);
    }
    let mut gone_ids = Vec::new();
    for (k, id) in ids[unknown..].iter().enumerate() {
        let folder = root.join(format!("Gone Series {k:03}"));
        gone_ids.push(insert_removed_series(app, lib_id, *id, &folder, at(50 - k)).await);
    }
    (unknown_ids, gone_ids)
}

async fn issue_confirmed(app: &TestApp, id: &str) -> bool {
    let row = issue_row(app, id).await;
    assert!(row.removed_at.is_some(), "{id}: still removed");
    row.removal_confirmed_at.is_some()
}

async fn series_confirmed(app: &TestApp, id: Uuid) -> bool {
    let row = SeriesEntity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.removed_at.is_some(), "{id}: still removed");
    row.removal_confirmed_at.is_some()
}

/// More rows the sweep cannot confirm (unknown) at the front of the order
/// than fit in one page, and more than the per-run cap, must not hide the
/// confirmable rows behind them: one run pages past them, confirms every
/// gone row, and leaves the unknown rows untouched.
#[tokio::test]
async fn auto_confirm_pages_past_unknown_rows_at_the_front() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (unknown, gone) = paging_issues(&app, tmp.path(), 7, 4, false).await;
    let state = app.state();
    let limits = reconcile::SweepLimits {
        page: 3,
        max_issue_confirms: 5,
        ..reconcile::SweepLimits::default()
    };

    let stats = reconcile::auto_confirm_sweep_with(&state, limits)
        .await
        .unwrap();
    assert_eq!(stats.confirmed, 4, "every gone row behind: {stats:?}");
    assert_eq!(stats.kept_unknown, 7, "{stats:?}");
    assert_eq!(stats.libraries_aborted, 0, "{stats:?}");
    for id in &gone {
        assert!(issue_confirmed(&app, id).await, "{id}: gone → confirmed");
    }
    for id in &unknown {
        assert!(
            !issue_confirmed(&app, id).await,
            "{id}: unknown → untouched"
        );
    }

    // The next run only re-examines the unknown rows.
    let stats = reconcile::auto_confirm_sweep_with(&state, limits)
        .await
        .unwrap();
    assert_eq!(stats.confirmed, 0, "{stats:?}");
    assert_eq!(stats.kept_unknown, 7, "{stats:?}");
}

/// The per-run cap counts confirmations, oldest removal first, and holds
/// when it falls inside a page; unknown rows do not count against it, and
/// the rest go on the next run.
#[tokio::test]
async fn auto_confirm_confirms_at_most_the_cap_per_run() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (unknown, gone) = paging_issues(&app, tmp.path(), 2, 5, false).await;
    let state = app.state();
    // Page 1: two unknown + gone[0..2]; page 2: gone[2..5], cut after one.
    let limits = reconcile::SweepLimits {
        page: 4,
        max_issue_confirms: 3,
        ..reconcile::SweepLimits::default()
    };

    let before = Utc::now().fixed_offset();
    let stats = reconcile::auto_confirm_sweep_with(&state, limits)
        .await
        .unwrap();
    let after = Utc::now().fixed_offset();
    assert_eq!(stats.confirmed, 3, "the cap: {stats:?}");
    assert_eq!(stats.kept_unknown, 2, "{stats:?}");
    for id in &gone[..3] {
        assert!(issue_confirmed(&app, id).await, "{id}: oldest first");
        let stamp = issue_row(&app, id).await.removal_confirmed_at.unwrap();
        assert!(before <= stamp && stamp <= after, "{id}: stamped {stamp}");
    }
    for id in &gone[3..] {
        assert!(!issue_confirmed(&app, id).await, "{id}: past the cap");
    }

    let stats = reconcile::auto_confirm_sweep_with(&state, limits)
        .await
        .unwrap();
    assert_eq!(stats.confirmed, 2, "the rest: {stats:?}");
    for id in &gone {
        assert!(issue_confirmed(&app, id).await, "{id}: confirmed by now");
    }
    for id in &unknown {
        assert!(
            !issue_confirmed(&app, id).await,
            "{id}: unknown → untouched"
        );
    }
}

/// Rows with the same `removed_at` across a page boundary are told apart
/// by `id`: none is skipped and none examined twice. (A reconcile stamps
/// every row it removes in one run with the same instant.)
#[tokio::test]
async fn auto_confirm_pages_through_equal_removal_times() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (unknown, gone) = paging_issues(&app, tmp.path(), 1, 3, true).await;
    let state = app.state();
    // Four rows, one instant, pages of two.
    let limits = reconcile::SweepLimits {
        page: 2,
        ..reconcile::SweepLimits::default()
    };

    let stats = reconcile::auto_confirm_sweep_with(&state, limits)
        .await
        .unwrap();
    assert_eq!(stats.confirmed, 3, "{stats:?}");
    assert_eq!(stats.kept_unknown, 1, "examined once: {stats:?}");
    for id in &gone {
        assert!(issue_confirmed(&app, id).await, "{id}");
    }
    assert!(!issue_confirmed(&app, &unknown[0]).await);
}

/// [`auto_confirm_pages_past_unknown_rows_at_the_front`] for series.
#[tokio::test]
async fn auto_confirm_series_pages_past_unknown_series_at_the_front() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (unknown, gone) = paging_series(&app, tmp.path(), 7, 4, false).await;
    let state = app.state();
    let limits = reconcile::SweepLimits {
        page: 3,
        max_series_confirms: 5,
        ..reconcile::SweepLimits::default()
    };

    let stats = reconcile::auto_confirm_sweep_with(&state, limits)
        .await
        .unwrap();
    assert_eq!(stats.confirmed, 4, "every gone series behind: {stats:?}");
    assert_eq!(stats.kept_unknown, 7, "{stats:?}");
    for id in &gone {
        assert!(series_confirmed(&app, *id).await, "{id}: gone → confirmed");
    }
    for id in &unknown {
        assert!(
            !series_confirmed(&app, *id).await,
            "{id}: unknown → untouched"
        );
    }

    let stats = reconcile::auto_confirm_sweep_with(&state, limits)
        .await
        .unwrap();
    assert_eq!(stats.confirmed, 0, "{stats:?}");
    assert_eq!(stats.kept_unknown, 7, "{stats:?}");
}

/// [`auto_confirm_confirms_at_most_the_cap_per_run`] for series.
#[tokio::test]
async fn auto_confirm_series_confirms_at_most_the_cap_per_run() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (unknown, gone) = paging_series(&app, tmp.path(), 2, 5, false).await;
    let state = app.state();
    let limits = reconcile::SweepLimits {
        page: 4,
        max_series_confirms: 3,
        ..reconcile::SweepLimits::default()
    };

    let stats = reconcile::auto_confirm_sweep_with(&state, limits)
        .await
        .unwrap();
    assert_eq!(stats.confirmed, 3, "the cap: {stats:?}");
    assert_eq!(stats.kept_unknown, 2, "{stats:?}");
    for id in &gone[..3] {
        assert!(series_confirmed(&app, *id).await, "{id}: oldest first");
    }
    for id in &gone[3..] {
        assert!(!series_confirmed(&app, *id).await, "{id}: past the cap");
    }

    let stats = reconcile::auto_confirm_sweep_with(&state, limits)
        .await
        .unwrap();
    assert_eq!(stats.confirmed, 2, "the rest: {stats:?}");
    for id in &gone {
        assert!(series_confirmed(&app, *id).await, "{id}: confirmed by now");
    }
    for id in &unknown {
        assert!(
            !series_confirmed(&app, *id).await,
            "{id}: unknown → untouched"
        );
    }
}

/// [`auto_confirm_pages_through_equal_removal_times`] for series: none is
/// skipped. (Kept series are a set, so a second look at the unknown one
/// would not show in `kept_unknown`; the issue test covers that.)
#[tokio::test]
async fn auto_confirm_series_pages_through_equal_removal_times() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (unknown, gone) = paging_series(&app, tmp.path(), 1, 3, true).await;
    let state = app.state();
    let limits = reconcile::SweepLimits {
        page: 2,
        ..reconcile::SweepLimits::default()
    };

    let stats = reconcile::auto_confirm_sweep_with(&state, limits)
        .await
        .unwrap();
    assert_eq!(stats.confirmed, 3, "{stats:?}");
    assert_eq!(stats.kept_unknown, 1, "{stats:?}");
    for id in &gone {
        assert!(series_confirmed(&app, *id).await, "{id}");
    }
    assert!(!series_confirmed(&app, unknown[0]).await);
}

/// Over the per-run series cap, one library scan is queued instead of a
/// scan per series (cap injected as 2 with three series).
#[tokio::test]
async fn many_series_to_rescan_queue_one_library_scan() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let names = ["Nu", "Xi", "Pi"];
    for (n, name) in names.iter().enumerate() {
        write_old_folder(
            &tmp.path().join(format!("Series Many{name} (2025)")),
            &format!("Many{name}"),
            1,
            1500 + 10 * n as u32,
        );
    }
    let lib_id = create_library(&app, tmp.path(), 1).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let before = library_scan_runs(&app, lib_id).await;
    let mut ids = Vec::new();
    for name in names {
        let s = series_named(&app, &format!("Many{name}")).await;
        age_series_scan(&app, s.id).await;
        let folder = tmp.path().join(format!("Series Many{name} (2025)"));
        mark_removed_days_ago(&app, &folder.join(format!("Many{name} 001.cbz")), 5).await;
        ids.push(s.id);
    }
    let limits = reconcile::SweepLimits {
        max_series_rescans: 2,
        ..reconcile::SweepLimits::default()
    };

    let stats = reconcile::auto_confirm_sweep_with(&state, limits)
        .await
        .unwrap();
    assert_eq!(stats.kept_present, 3, "{stats:?}");
    assert_eq!(stats.rescans_queued, 0, "{stats:?}");
    assert_eq!(stats.library_scans_queued, 1, "{stats:?}");
    for id in ids {
        assert_eq!(series_scan_runs(&app, id).await, 0);
    }
    assert_eq!(library_scan_runs(&app, lib_id).await, before + 1);
}

/// A series scanned within the throttle window is not rescanned again; its
/// present row is still kept.
#[tokio::test]
async fn recently_scanned_series_is_not_rescanned() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series TauTwo (2025)");
    write_old_folder(&folder, "TauTwo", 1, 1600);
    let lib_id = create_library(&app, tmp.path(), 1).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let series = series_named(&app, "TauTwo").await;
    let row = mark_removed_days_ago(&app, &folder.join("TauTwo 001.cbz"), 5).await;
    // `last_scanned_at` is the scan above: inside the window.

    let stats = reconcile::auto_confirm_sweep(&state).await.unwrap();
    assert_eq!(stats.kept_present, 1, "{stats:?}");
    assert_eq!(stats.rescans_throttled, 1, "{stats:?}");
    assert_eq!(stats.rescans_queued, 0, "{stats:?}");
    assert!(
        issue_row(&app, &row.id)
            .await
            .removal_confirmed_at
            .is_none()
    );
    assert_eq!(series_scan_runs(&app, series.id).await, 0);

    // Past the window, the next run queues one.
    age_series_scan(&app, series.id).await;
    let stats = reconcile::auto_confirm_sweep(&state).await.unwrap();
    assert_eq!(stats.rescans_queued, 1, "{stats:?}");
    assert_eq!(series_scan_runs(&app, series.id).await, 1);
}

// ───── Reconcile stopped by a lost scan lease (R-03) ─────

/// Run `reconcile_library_seen_until` over every series of `lib_id` whose
/// folder is in `present`, stopping when `stop` says so.
async fn reconcile_until(
    app: &TestApp,
    lib_id: Uuid,
    scanned: &std::collections::HashSet<Uuid>,
    present: &std::collections::HashSet<String>,
    stop: &(dyn Fn() -> bool + Send + Sync),
) -> (anyhow::Result<u64>, scanner::ScanStats) {
    let state = app.state();
    let seen: std::collections::HashSet<String> = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.file_path)
        .filter(|p| Path::new(p).exists())
        .collect();
    let mut stats = scanner::ScanStats::default();
    let mut events = server::library::event_log::EventCollector::new(lib_id, None);
    let res = reconcile::reconcile_library_seen_until(
        &state.db,
        lib_id,
        scanned,
        &seen,
        present,
        present,
        None,
        &std::collections::HashSet::new(),
        &mut stats,
        &mut events,
        Some(stop),
    )
    .await;
    events.flush(&state.db).await;
    (res, stats)
}

/// A stop counter: the first `allowed` checks pass, every later one stops.
fn stop_after(allowed: usize) -> impl Fn() -> bool + Send + Sync {
    let calls = std::sync::atomic::AtomicUsize::new(0);
    move || calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= allowed
}

/// A stop requested while the reconcile works on the series the scan
/// itself walked does not interrupt that work: a series whose files were
/// all deleted is still marked removed, because a later non-forced scan
/// would skip its unchanged folder and never mark it.
#[tokio::test]
async fn stopped_reconcile_still_marks_the_scanned_emptied_series_removed() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Tav (2025)");
    write_old_folder(&folder, "Tav", 2, 900);
    let lib_id = create_library(&app, tmp.path(), 30).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let series = series_named(&app, "Tav").await;
    std::fs::remove_file(folder.join("Tav 001.cbz")).unwrap();
    std::fs::remove_file(folder.join("Tav 002.cbz")).unwrap();

    let scanned: std::collections::HashSet<Uuid> = [series.id].into_iter().collect();
    let present: std::collections::HashSet<String> = [folder.to_string_lossy().into_owned()]
        .into_iter()
        .collect();
    // Entry passes; the stop is requested from then on.
    let stop = stop_after(1);
    let (res, stats) = reconcile_until(&app, lib_id, &scanned, &present, &stop).await;
    assert!(res.is_err(), "the reconcile reports the stop");
    assert_eq!(stats.issues_removed, 2, "{stats:?}");
    assert_eq!(stats.series_removed, 1, "{stats:?}");
    assert!(
        issues_of(&app, series.id)
            .await
            .iter()
            .all(|i| i.removed_at.is_some())
    );
    let sr = SeriesEntity::find_by_id(series.id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(
        sr.removed_at.is_some(),
        "the emptied series is marked removed"
    );
}

/// A stop between series of the folder-missing sweep completes the series
/// already handled, leaves the rest, and records events only for rows that
/// were written.
#[tokio::test]
async fn stop_mid_sweep_records_only_what_was_written() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    write_old_folder(&tmp.path().join("Series Kaf (2025)"), "Kaf", 2, 910);
    write_old_folder(&tmp.path().join("Series Lamed (2025)"), "Lamed", 2, 920);
    let lib_id = create_library(&app, tmp.path(), 30).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    std::fs::remove_dir_all(tmp.path().join("Series Kaf (2025)")).unwrap();
    std::fs::remove_dir_all(tmp.path().join("Series Lamed (2025)")).unwrap();
    let removed_events_before = entity::library_event::Entity::find()
        .filter(entity::library_event::Column::LibraryId.eq(lib_id))
        .filter(entity::library_event::Column::Action.eq("removed"))
        .all(&state.db)
        .await
        .unwrap()
        .len();

    // Entry and the first missing series pass; the second is stopped.
    let stop = stop_after(2);
    let empty_scanned = std::collections::HashSet::new();
    let empty_present = std::collections::HashSet::new();
    let (res, stats) = reconcile_until(&app, lib_id, &empty_scanned, &empty_present, &stop).await;
    assert!(res.is_err());

    let rows = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    let removed: Vec<&entity::series::Model> =
        rows.iter().filter(|s| s.removed_at.is_some()).collect();
    assert_eq!(removed.len(), 1, "exactly the handled series is flagged");
    assert_eq!(stats.series_removed, 1, "{stats:?}");
    assert_eq!(stats.issues_removed, 2, "{stats:?}");
    let handled = removed[0].id;
    assert!(
        issues_of(&app, handled)
            .await
            .iter()
            .all(|i| i.removed_at.is_some())
    );
    let other = rows.iter().find(|s| s.id != handled).unwrap();
    assert!(other.removed_at.is_none());
    assert!(
        issues_of(&app, other.id)
            .await
            .iter()
            .all(|i| i.removed_at.is_none())
    );

    let events: Vec<entity::library_event::Model> = entity::library_event::Entity::find()
        .filter(entity::library_event::Column::LibraryId.eq(lib_id))
        .filter(entity::library_event::Column::Action.eq("removed"))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(
        events.len(),
        removed_events_before + 1,
        "one event, for the written series"
    );
    assert!(
        events
            .iter()
            .any(|e| e.entity_id.as_deref() == Some(handled.to_string().as_str())),
        "the event names the series that was written"
    );
    assert!(
        !events
            .iter()
            .any(|e| e.entity_id.as_deref() == Some(other.id.to_string().as_str())),
        "no event for the series left to the next reconcile"
    );
}
