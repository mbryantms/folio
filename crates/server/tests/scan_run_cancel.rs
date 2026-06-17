//! Integration coverage for the manual scan-cancel endpoint
//! (`POST /libraries/{slug}/scan-runs/{id}/cancel`). The endpoint is the
//! operator's escape hatch for scans that have lost their worker —
//! typical cause: "Clear queues" mid-flight purges pending Redis jobs
//! but leaves the `scan_runs` row sitting at `state='running'`
//! forever because nothing transitions it to a terminal state.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{library, scan_run};
use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, Set};
use std::io::Write;
use std::path::Path;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

async fn register(app: &TestApp, email: &str) -> Authed {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
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
    let json: serde_json::Value = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id,
    }
}

async fn promote_to_admin(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = entity::user::Entity::find_by_id(user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::user::ActiveModel = user.into();
    am.role = Set("admin".into());
    am.update(&db).await.unwrap();
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: &Authed,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf)
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn seed_library_and_scan_run(app: &TestApp, state: &str) -> (Uuid, String, Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let slug = lib_id.to_string();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Test Library".into()),
        root_path: Set(format!("/tmp/scan-cancel-{lib_id}")),
        default_language: Set("en".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(slug.clone()),
        scan_schedule_cron: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_scan_at: Set(None),
        ignore_globs: Set(serde_json::json!([])),
        report_missing_comicinfo: Set(false),
        file_watch_enabled: Set(true),
        soft_delete_days: Set(30),
        thumbnails_enabled: Set(true),
        thumbnail_format: Set("webp".into()),
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
    let scan_id = Uuid::now_v7();
    scan_run::ActiveModel {
        id: Set(scan_id),
        library_id: Set(lib_id),
        state: Set(state.into()),
        started_at: Set(now),
        ended_at: Set(None),
        stats: Set(serde_json::json!({})),
        error: Set(None),
        kind: Set("library".into()),
        series_id: Set(None),
        issue_id: Set(None),
        batch_id: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    (lib_id, slug, scan_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_flips_running_scan_to_cancelled() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin-cancel@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (lib_id, slug, scan_id) = seed_library_and_scan_run(&app, "running").await;

    let (status, body) = http(
        &app,
        Method::POST,
        &format!("/api/libraries/{slug}/scan-runs/{scan_id}/cancel"),
        &auth,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body:#?}");
    assert_eq!(body["state"], "cancelled");
    assert_eq!(body["error"], "Cancelled by admin");
    assert!(body["ended_at"].is_string());

    // DB row reflects the change.
    let db = Database::connect(&app.db_url).await.unwrap();
    let row = scan_run::Entity::find_by_id(scan_id)
        .one(&db)
        .await
        .unwrap()
        .expect("row");
    assert_eq!(row.state, "cancelled");
    assert!(row.ended_at.is_some());
    assert_eq!(row.error.as_deref(), Some("Cancelled by admin"));
    assert_eq!(row.library_id, lib_id);

    // Audit row landed.
    let audit_rows = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.scan_run.cancel"))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(audit_rows.len(), 1, "exactly one cancel audit row");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_terminal_scan_returns_409() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin-409@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib_id, slug, scan_id) = seed_library_and_scan_run(&app, "complete").await;

    let (status, body) = http(
        &app,
        Method::POST,
        &format!("/api/libraries/{slug}/scan-runs/{scan_id}/cancel"),
        &auth,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body:#?}");
    assert_eq!(body["error"]["code"], "already_terminal");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_requires_admin() {
    let app = TestApp::spawn().await;
    // First user auto-admin; second user is a regular reader.
    let _admin = register(&app, "first@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let (_lib_id, slug, scan_id) = seed_library_and_scan_run(&app, "running").await;

    let (status, _body) = http(
        &app,
        Method::POST,
        &format!("/api/libraries/{slug}/scan-runs/{scan_id}/cancel"),
        &reader,
    )
    .await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED),
        "non-admin should be rejected; got {status}",
    );
}

// ─────────────────────────────────────────────────────────────
// Cross-library scan-run admin endpoints (M2 of the cross-library
// findings plan). Same dispatcher uses seed_library_and_scan_run as
// the cancel tests above — seeds a library + a single scan_run row,
// then asserts the admin views surface it correctly.
// ─────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_cross_library_scan_runs_aggregates_with_library_enrichment() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin-x-scan-runs@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    // Seed two libraries, each with one scan_run.
    let (lib_a, _slug_a, scan_a) = seed_library_and_scan_run(&app, "complete").await;
    let (lib_b, _slug_b, scan_b) = seed_library_and_scan_run(&app, "failed").await;

    // Default: both libraries' runs surface in one response with
    // library_name + library_slug carried per row.
    let (status, body) = http(&app, Method::GET, "/api/admin/scan-runs", &auth).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().expect("items");
    let scan_ids: std::collections::HashSet<&str> =
        items.iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert!(scan_ids.contains(scan_a.to_string().as_str()));
    assert!(scan_ids.contains(scan_b.to_string().as_str()));
    for item in items {
        assert!(item["library_name"].as_str().is_some());
        assert!(item["library_slug"].as_str().is_some());
    }

    // Filter by state=failed: only lib_b's row.
    let (status, body) = http(
        &app,
        Method::GET,
        "/api/admin/scan-runs?state=failed",
        &auth,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], scan_b.to_string());
    assert_eq!(items[0]["library_id"], lib_b.to_string());

    // Filter by library_id: scoped result.
    let (status, body) = http(
        &app,
        Method::GET,
        &format!("/api/admin/scan-runs?library_id={lib_a}"),
        &auth,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().expect("items");
    assert!(items.iter().all(|v| v["library_id"] == lib_a.to_string()));

    // Invalid state filter → 422.
    let (status, _) = http(&app, Method::GET, "/api/admin/scan-runs?state=bogus", &auth).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_latest_per_library_returns_one_row_per_library() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin-latest@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    // Seed lib_a with TWO scans, lib_b with one. Latest-per-library
    // should return exactly two rows (one per library), and lib_a's
    // entry should be the newer of its two scans.
    let (lib_a, _, scan_a_old) = seed_library_and_scan_run(&app, "complete").await;

    // Insert a newer scan for lib_a.
    let db = Database::connect(&app.db_url).await.unwrap();
    let scan_a_new = Uuid::now_v7();
    let later = chrono::Utc::now().fixed_offset() + chrono::Duration::seconds(10);
    scan_run::ActiveModel {
        id: Set(scan_a_new),
        library_id: Set(lib_a),
        state: Set("complete".into()),
        started_at: Set(later),
        ended_at: Set(Some(later)),
        stats: Set(serde_json::json!({})),
        error: Set(None),
        kind: Set("library".into()),
        series_id: Set(None),
        issue_id: Set(None),
        batch_id: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    let (lib_b, _, scan_b) = seed_library_and_scan_run(&app, "complete").await;

    let (status, body) = http(
        &app,
        Method::GET,
        "/api/admin/scan-runs/latest-per-library",
        &auth,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = body.as_array().expect("array");
    assert_eq!(items.len(), 2, "expected 1 row per library");

    let by_lib: std::collections::HashMap<&str, &str> = items
        .iter()
        .map(|v| (v["library_id"].as_str().unwrap(), v["id"].as_str().unwrap()))
        .collect();
    assert_eq!(
        by_lib.get(lib_a.to_string().as_str()),
        Some(&scan_a_new.to_string().as_str()),
        "lib_a entry must be the NEWER scan, not the older one ({scan_a_old})",
    );
    assert_eq!(
        by_lib.get(lib_b.to_string().as_str()),
        Some(&scan_b.to_string().as_str()),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_unknown_scan_returns_404() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin-404@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib_id, slug, _scan_id) = seed_library_and_scan_run(&app, "running").await;

    let bogus = Uuid::now_v7();
    let (status, _body) = http(
        &app,
        Method::POST,
        &format!("/api/libraries/{slug}/scan-runs/{bogus}/cancel"),
        &auth,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// D5: the per-library scan-runs list is cursor-paginated — walking the
/// `next_cursor` returns every run with no silent cap and no duplicates.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scan_runs_list_paginates_past_the_first_page() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin-page@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    // Seeds the library + 1 run; add 5 more for 6 total.
    let (lib_id, slug, _first) = seed_library_and_scan_run(&app, "complete").await;

    let db = Database::connect(&app.db_url).await.unwrap();
    let base = Utc::now().fixed_offset();
    for i in 1i64..=5 {
        scan_run::ActiveModel {
            id: Set(Uuid::now_v7()),
            library_id: Set(lib_id),
            state: Set("complete".into()),
            started_at: Set(base - chrono::Duration::seconds(i)),
            ended_at: Set(None),
            stats: Set(serde_json::json!({})),
            error: Set(None),
            kind: Set("library".into()),
            series_id: Set(None),
            issue_id: Set(None),
            batch_id: Set(None),
        }
        .insert(&db)
        .await
        .unwrap();
    }

    // Walk pages of 2 until exhausted; collect ids.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let uri = match &cursor {
            Some(c) => format!("/api/libraries/{slug}/scan-runs?limit=2&cursor={c}"),
            None => format!("/api/libraries/{slug}/scan-runs?limit=2"),
        };
        let (status, body) = http(&app, Method::GET, &uri, &auth).await;
        assert_eq!(status, StatusCode::OK, "body: {body:#?}");
        let items = body["items"].as_array().expect("items array");
        assert!(items.len() <= 2, "page exceeded the requested limit");
        for it in items {
            let id = it["id"].as_str().unwrap().to_owned();
            assert!(seen.insert(id), "duplicate row across pages");
        }
        pages += 1;
        assert!(pages < 10, "pagination failed to terminate");
        match body["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    assert_eq!(
        seen.len(),
        6,
        "every run should be reachable across pages (no cap)"
    );
    assert!(pages >= 3, "6 rows at limit=2 should span at least 3 pages");
}

// ─────────────────────────────────────────────────────────────
// D8a: worker-side cooperative cancellation. The endpoint flips the
// `scan_runs` row to `state='cancelled'`; the scanner polls for that and
// drains without running the Phase-4 reconcile. The reconcile-skip is the
// load-bearing invariant: a cancelled scan walked only part of the library,
// so its `seen_paths` is incomplete — reconciling on it would soft-delete
// every series the cancelled pass never reached.
// ─────────────────────────────────────────────────────────────

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

async fn create_on_disk_library(app: &TestApp, root: &Path) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(id),
        name: Set("Cancel Lib".into()),
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
        soft_delete_days: Set(30),
        thumbnails_enabled: Set(false),
        thumbnail_format: Set("webp".into()),
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

/// A scan that observes `state='cancelled'` before it processes any folder
/// must (a) finalize as `cancelled`, (b) NOT bump `last_scan_at`, and
/// (c) NOT soft-delete the series a prior full scan created — because the
/// Phase-4 reconcile is skipped on a partial pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_scan_skips_reconcile_and_preserves_series() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    // Three distinct series folders.
    for (i, name) in ["Alpha (2025)", "Beta (2025)", "Gamma (2025)"]
        .into_iter()
        .enumerate()
    {
        let folder = tmp.path().join(name);
        std::fs::create_dir_all(&folder).unwrap();
        write_cbz(&folder.join(format!("{name} 001.cbz")), i as u32 + 1);
    }

    let lib_id = create_on_disk_library(&app, tmp.path()).await;
    let state = app.state();

    // Full scan: all three series land, none removed, last_scan_at bumped.
    let s1 = server::library::scanner::scan_library(&state, lib_id)
        .await
        .unwrap();
    assert_eq!(s1.files_added, 3, "first scan adds three issues: {s1:?}");
    let live_before = entity::series::Entity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .filter(entity::series::Column::RemovedAt.is_null())
        .all(&state.db)
        .await
        .unwrap()
        .len();
    assert_eq!(live_before, 3, "three live series after the full scan");

    let db = Database::connect(&app.db_url).await.unwrap();
    let last_scan_after_full = library::Entity::find_by_id(lib_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .last_scan_at;
    assert!(
        last_scan_after_full.is_some(),
        "full scan bumps last_scan_at"
    );

    // Pre-insert a `cancelled` run row, then ask the scanner to use that id.
    // `open_scan_run` leaves a non-`queued` row untouched, so the scanner
    // picks up the row already in `state='cancelled'` and the pre-loop poll
    // trips the cooperative-cancel path deterministically.
    let cancelled_id = Uuid::now_v7();
    scan_run::ActiveModel {
        id: Set(cancelled_id),
        library_id: Set(lib_id),
        state: Set("cancelled".into()),
        started_at: Set(Utc::now().fixed_offset()),
        ended_at: Set(None),
        stats: Set(serde_json::json!({})),
        error: Set(Some("Cancelled by admin".into())),
        kind: Set("library".into()),
        series_id: Set(None),
        issue_id: Set(None),
        batch_id: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    let s2 = server::library::scanner::scan_library_with_run_id(
        &state,
        lib_id,
        false,
        Some(cancelled_id),
    )
    .await
    .expect("cancelled scan returns Ok (graceful drain)");
    assert_eq!(
        s2.issues_removed, 0,
        "cancelled scan removes nothing: {s2:?}"
    );

    // (a) the run stays cancelled.
    let row = scan_run::Entity::find_by_id(cancelled_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "cancelled");
    assert!(row.ended_at.is_some(), "finalize stamps ended_at");

    // (b) last_scan_at is unchanged — a cancelled pass is not a completed scan.
    let last_scan_after_cancel = library::Entity::find_by_id(lib_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .last_scan_at;
    assert_eq!(
        last_scan_after_cancel, last_scan_after_full,
        "cancelled scan must not bump last_scan_at",
    );

    // (c) the load-bearing invariant: all three series survive. A reconcile
    // over the cancelled pass's empty seen-set would have soft-deleted them.
    let live_after = entity::series::Entity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .filter(entity::series::Column::RemovedAt.is_null())
        .all(&state.db)
        .await
        .unwrap()
        .len();
    assert_eq!(
        live_after, 3,
        "cancelled scan skipped reconcile — no series soft-deleted",
    );
}
