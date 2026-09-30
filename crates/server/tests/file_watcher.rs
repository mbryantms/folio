//! File watcher (WP-3.1) integration coverage.
//!
//! - inotify on a local temp dir: a 1,000-file copy into one series folder
//!   collapses into exactly one trigger → one queued scoped scan.
//! - forced-poll mode: the directory-mtime poller only ever stats
//!   directories (asserted through a counting [`DirProbe`] shim) and turns a
//!   new archive into one scoped scan of its folder.
//! - the scoped scan itself ingests only the touched folders and reconciles
//!   only what it enumerated.
//! - the coalescer unions scoped follow-ups and lets a full trigger win.
//! - the supervisor honours `file_watch_enabled` flips.
//! - `GET /api/admin/server/watchers` reports mode + last trigger.
//!
//! The TestApp never starts the apalis monitor, so enqueued scans stay
//! queued: `scan:in_flight` stays claimed and the queued `scan_runs` rows
//! are exactly the scans the watcher asked for.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{library, scan_run, series};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::library::ignore::IgnoreRules;
use server::library::watcher::{
    self, DirChild, DirProbe, FsDirProbe, ModeChoice, WatchMode, WatchOptions,
};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tower::ServiceExt;
use uuid::Uuid;

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

async fn create_library(app: &TestApp, root: &Path, watch: bool) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(id),
        name: Set(format!("Watch Lib {id}")),
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
        file_watch_enabled: Set(watch),
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
    .insert(&app.state().db)
    .await
    .unwrap();
    id
}

async fn scan_runs_for(app: &TestApp, lib: Uuid) -> Vec<scan_run::Model> {
    scan_run::Entity::find()
        .filter(scan_run::Column::LibraryId.eq(lib))
        .all(&app.state().db)
        .await
        .unwrap()
}

/// Every `scan::Job` payload currently stored in the apalis `scan` queue,
/// as JSON (the args object carrying `library_id` / `scope`).
async fn queued_scan_payloads(app: &TestApp) -> Vec<serde_json::Value> {
    let state = app.state();
    let hash = state.jobs.scan_storage.get_config().job_data_hash();
    let mut conn = state.jobs.redis.clone();
    let raw: Vec<Vec<u8>> = redis::cmd("HVALS")
        .arg(&hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    fn find_job(v: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
        match v {
            serde_json::Value::Object(m) => {
                if m.contains_key("scan_run_id") && m.contains_key("library_id") {
                    out.push(v.clone());
                    return;
                }
                for child in m.values() {
                    find_job(child, out);
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|c| find_job(c, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for bytes in raw {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            find_job(&v, &mut out);
        }
    }
    out
}

fn scope_of(job: &serde_json::Value) -> Option<Vec<String>> {
    job.get("scope").and_then(|s| s.as_array()).map(|a| {
        a.iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    })
}

async fn wait_for(mut cond: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cond()
}

// ───────────────────────── inotify ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inotify_thousand_file_copy_triggers_exactly_one_scoped_scan() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mount = watcher::detect_mount(&root).unwrap();
    assert!(
        !mount.network,
        "test temp dir must be on a local filesystem, got {mount:?}"
    );
    let series_dir = root.join("Bulk Series");
    std::fs::create_dir(&series_dir).unwrap();
    let lib = create_library(&app, &root, true).await;
    let state = app.state();

    let debounce = Duration::from_millis(1500);
    let handle = watcher::start_library_watcher(
        &state,
        lib,
        root.clone(),
        IgnoreRules::default(),
        WatchOptions {
            mode: ModeChoice::Auto,
            debounce,
            max_wait: Duration::from_secs(60),
            poll_interval: Duration::from_secs(3600),
            probe: Arc::new(FsDirProbe),
        },
    )
    .await
    .expect("watcher starts");
    assert_eq!(handle.mode(), WatchMode::Inotify);
    assert_eq!(
        state.watchers.status(lib).unwrap().filesystem.as_deref(),
        Some(mount.filesystem.as_str())
    );

    // The "copy": 1,000 archives written into one series folder, plus the
    // temp-file/rename dance a copier often does (ignored noise).
    let copier = series_dir.clone();
    tokio::task::spawn_blocking(move || {
        for i in 0..1000_u32 {
            let tmp_name = copier.join(format!("Bulk {i:04}.cbz.part"));
            std::fs::write(&tmp_name, b"PK\x03\x04 not really a zip").unwrap();
            std::fs::rename(&tmp_name, copier.join(format!("Bulk {i:04}.cbz"))).unwrap();
        }
    })
    .await
    .unwrap();

    let reg = Arc::clone(&state.watchers);
    assert!(
        wait_for(
            || reg.status(lib).is_some_and(|s| s.triggers_total >= 1),
            Duration::from_secs(30)
        )
        .await,
        "watcher never triggered: {:?}",
        state.watchers.status(lib)
    );
    // Hold on past several more debounce windows: nothing else may fire.
    tokio::time::sleep(debounce * 3).await;

    let status = state.watchers.status(lib).unwrap();
    assert_eq!(status.triggers_total, 1, "exactly one trigger: {status:?}");
    assert_eq!(status.mode, WatchMode::Inotify);
    assert_eq!(status.last_trigger_dirs, 1, "only the series folder");
    assert!(!status.last_trigger_coalesced);
    assert!(status.last_event_at.is_some());

    let runs = scan_runs_for(&app, lib).await;
    assert_eq!(runs.len(), 1, "exactly one scan run: {runs:?}");
    assert_eq!(runs[0].state, "queued");
    assert_eq!(runs[0].kind, "library");
    assert_eq!(Some(runs[0].id), status.last_scan_id);

    let jobs = queued_scan_payloads(&app).await;
    assert_eq!(jobs.len(), 1, "one queued scan job: {jobs:?}");
    assert_eq!(
        scope_of(&jobs[0]),
        Some(vec![series_dir.to_string_lossy().into_owned()]),
        "the job is scoped to the touched folder only"
    );
    assert_eq!(jobs[0]["force"], serde_json::json!(false));

    handle.stop().await;
}

// ───────────────────────── forced poll ─────────────────────────

/// Counting shim over the real filesystem: records every path the poller
/// asks about so the test can prove no file is ever stat'ed.
#[derive(Default)]
struct CountingProbe {
    stats: Mutex<Vec<PathBuf>>,
    lists: Mutex<Vec<PathBuf>>,
}

impl DirProbe for CountingProbe {
    fn dir_mtime(&self, dir: &Path) -> std::io::Result<SystemTime> {
        self.stats.lock().unwrap().push(dir.to_path_buf());
        FsDirProbe.dir_mtime(dir)
    }
    fn list(&self, dir: &Path) -> std::io::Result<Vec<DirChild>> {
        self.lists.lock().unwrap().push(dir.to_path_buf());
        FsDirProbe.list(dir)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forced_poll_stats_directories_only_and_triggers_scoped_scan() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    // Layout A + Layout B + a category subfolder: 6 directories, 12 files.
    let a = root.join("Alpha");
    let b = root.join("Beta");
    let b_specials = b.join("Specials");
    let publisher = root.join("Publisher");
    let c = publisher.join("Gamma");
    for d in [&a, &b_specials, &c] {
        std::fs::create_dir_all(d).unwrap();
    }
    for i in 0..5 {
        write_cbz(&a.join(format!("Alpha {i:03}.cbz")), i);
    }
    for i in 0..3 {
        write_cbz(&b.join(format!("Beta {i:03}.cbz")), 10 + i);
    }
    for i in 0..2 {
        write_cbz(&b_specials.join(format!("Beta Special {i}.cbz")), 20 + i);
        write_cbz(&c.join(format!("Gamma {i:03}.cbz")), 30 + i);
    }
    let dirs: std::collections::HashSet<PathBuf> = [
        root.clone(),
        a.clone(),
        b.clone(),
        b_specials.clone(),
        publisher.clone(),
        c.clone(),
    ]
    .into_iter()
    .collect();

    let lib = create_library(&app, &root, true).await;
    let state = app.state();
    let probe = Arc::new(CountingProbe::default());
    let handle = watcher::start_library_watcher(
        &state,
        lib,
        root.clone(),
        IgnoreRules::default(),
        WatchOptions {
            mode: ModeChoice::ForcePoll,
            debounce: Duration::from_secs(30),
            max_wait: Duration::from_secs(300),
            poll_interval: Duration::from_millis(250),
            probe: probe.clone(),
        },
    )
    .await
    .expect("watcher starts");
    assert_eq!(handle.mode(), WatchMode::Poll);

    // Let the baseline + a few idle passes run: nothing changed, no trigger.
    let p = probe.clone();
    assert!(
        wait_for(
            || p.stats.lock().unwrap().len() >= dirs.len() * 4,
            Duration::from_secs(10)
        )
        .await
    );
    assert_eq!(state.watchers.status(lib).unwrap().triggers_total, 0);

    // A new archive lands in Beta (as a remote writer would).
    write_cbz(&b.join("Beta 003.cbz"), 99);
    let reg = Arc::clone(&state.watchers);
    assert!(
        wait_for(
            || reg.status(lib).is_some_and(|s| s.triggers_total >= 1),
            Duration::from_secs(10)
        )
        .await,
        "poller never triggered"
    );
    // A few more idle passes: the change is reported once.
    let seen = probe.stats.lock().unwrap().len();
    let p = probe.clone();
    assert!(
        wait_for(
            || p.stats.lock().unwrap().len() >= seen + dirs.len() * 3,
            Duration::from_secs(10)
        )
        .await
    );
    handle.stop().await;

    let status = state.watchers.status(lib).unwrap();
    assert_eq!(status.triggers_total, 1, "{status:?}");
    assert_eq!(status.mode, WatchMode::Poll);
    assert_eq!(status.last_trigger_dirs, 1);

    // The NAS invariant: every stat and every listing was of a directory —
    // not one of the 13 archives was ever stat'ed.
    let stats = probe.stats.lock().unwrap().clone();
    let lists = probe.lists.lock().unwrap().clone();
    assert!(!stats.is_empty());
    for p in stats.iter().chain(lists.iter()) {
        assert!(dirs.contains(p), "poller touched a non-directory: {p:?}");
    }
    // Each pass stats each directory exactly once.
    assert_eq!(stats.len() % dirs.len(), 0, "{} stats", stats.len());

    let runs = scan_runs_for(&app, lib).await;
    assert_eq!(runs.len(), 1, "{runs:?}");
    let jobs = queued_scan_payloads(&app).await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(
        scope_of(&jobs[0]),
        Some(vec![b.to_string_lossy().into_owned()])
    );
}

// ───────────────────────── scoped scan ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_scan_ingests_and_reconciles_only_its_scope() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let a = root.join("Alpha (2024)");
    let b = root.join("Beta (2024)");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    write_cbz(&a.join("Alpha (2024) 001.cbz"), 1);
    write_cbz(&b.join("Beta (2024) 001.cbz"), 2);
    let lib = create_library(&app, &root, true).await;
    let state = app.state();

    let full = server::library::scanner::scan_library(&state, lib)
        .await
        .unwrap();
    assert_eq!(full.files_added, 2);
    let last_full = library::Entity::find_by_id(lib)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap()
        .last_scan_at;

    let live_series = |state: server::state::AppState| async move {
        series::Entity::find()
            .filter(series::Column::LibraryId.eq(lib))
            .filter(series::Column::RemovedAt.is_null())
            .all(&state.db)
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.folder_path.unwrap())
            .collect::<std::collections::BTreeSet<_>>()
    };

    // A new issue in Alpha, and Beta deleted outright. A scan scoped to
    // Alpha picks up the new issue but does not judge Beta (out of scope).
    write_cbz(&a.join("Alpha (2024) 002.cbz"), 3);
    std::fs::remove_dir_all(&b).unwrap();
    let s =
        server::library::scanner::scan_library_scoped(&state, lib, std::slice::from_ref(&a), None)
            .await
            .unwrap();
    assert_eq!(s.files_added, 1, "{s:?}");
    assert_eq!(s.series_removed, 0, "Beta is out of scope: {s:?}");
    assert_eq!(live_series(state.clone()).await.len(), 2);

    // Scope = the root (a folder vanished from it): Beta's missing top is
    // reconciled away; Alpha is not re-walked.
    let s = server::library::scanner::scan_library_scoped(
        &state,
        lib,
        std::slice::from_ref(&root),
        None,
    )
    .await
    .unwrap();
    assert_eq!(s.files_added, 0, "{s:?}");
    assert_eq!(s.series_removed, 1, "{s:?}");
    assert_eq!(
        live_series(state.clone()).await,
        [a.to_string_lossy().into_owned()].into_iter().collect()
    );

    // A brand-new series folder: root + the new folder are touched.
    let c = root.join("Gamma (2024)");
    std::fs::create_dir_all(&c).unwrap();
    write_cbz(&c.join("Gamma (2024) 001.cbz"), 4);
    let s = server::library::scanner::scan_library_scoped(
        &state,
        lib,
        &[root.clone(), c.clone()],
        None,
    )
    .await
    .unwrap();
    assert_eq!(s.files_added, 1, "{s:?}");
    assert_eq!(live_series(state.clone()).await.len(), 2);

    // Scoped passes are recorded as library runs but never bump
    // `last_scan_at` (reserved for full passes).
    let after = library::Entity::find_by_id(lib)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap()
        .last_scan_at;
    assert_eq!(after, last_full);
}

// ───────────────────────── coalescing ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watch_triggers_coalesce_into_one_scoped_follow_up() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let lib = create_library(&app, tmp.path(), true).await;
    let state = app.state();
    let jobs = &state.jobs;

    let first = jobs
        .coalesce_watch_scan(lib, vec!["/lib/A".into()])
        .await
        .unwrap();
    assert!(!first.was_coalesced());
    // While it's "running", two more scoped triggers coalesce.
    let second = jobs
        .coalesce_watch_scan(lib, vec!["/lib/B".into()])
        .await
        .unwrap();
    let third = jobs
        .coalesce_watch_scan(lib, vec!["/lib/C".into(), "/lib/B".into()])
        .await
        .unwrap();
    assert!(second.was_coalesced() && third.was_coalesced());
    assert_eq!(second.scan_id(), first.scan_id());
    assert_eq!(scan_runs_for(&app, lib).await.len(), 1);

    // Finishing the first scan enqueues ONE follow-up scoped to the union.
    jobs.release_scan(lib).await.unwrap();
    assert_eq!(scan_runs_for(&app, lib).await.len(), 2);
    let payloads = queued_scan_payloads(&app).await;
    let mut scopes: Vec<Option<Vec<String>>> = payloads.iter().map(scope_of).collect();
    scopes.sort();
    assert_eq!(
        scopes,
        vec![
            Some(vec!["/lib/A".to_owned()]),
            Some(vec!["/lib/B".to_owned(), "/lib/C".to_owned()]),
        ]
    );

    // A full trigger while a scoped follow-up is queued wins: the next
    // follow-up covers the whole library.
    jobs.coalesce_watch_scan(lib, vec!["/lib/D".into()])
        .await
        .unwrap();
    jobs.coalesce_scan(lib, false).await.unwrap();
    jobs.coalesce_watch_scan(lib, vec!["/lib/E".into()])
        .await
        .unwrap();
    jobs.release_scan(lib).await.unwrap();
    let payloads = queued_scan_payloads(&app).await;
    assert_eq!(payloads.len(), 3);
    assert!(
        payloads.iter().any(|p| scope_of(p).is_none()),
        "full trigger made the follow-up a full scan: {payloads:?}"
    );
    assert_eq!(scan_runs_for(&app, lib).await.len(), 3);
}

// ───────────────────────── supervisor + API ─────────────────────────

async fn register_admin(app: &TestApp) -> (String, String) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"watch-admin@example.com","password":"correctly-horse-battery"}"#,
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
    (
        extract("__Host-comic_session="),
        extract("__Host-comic_csrf="),
    )
}

async fn get_watchers(app: &TestApp, session: &str, csrf: &str) -> serde_json::Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/admin/server/watchers")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={session}; __Host-comic_csrf={csrf}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervisor_follows_toggle_and_endpoint_reports_mode() {
    let app = TestApp::spawn().await;
    let (session, csrf) = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let watched = create_library(&app, tmp.path(), true).await;
    let other = tempfile::tempdir().unwrap();
    let unwatched = create_library(&app, other.path(), false).await;
    let state = app.state();

    // Before the supervisor has run, the endpoint still lists both.
    let body = get_watchers(&app, &session, &csrf).await;
    assert_eq!(body["libraries"].as_array().unwrap().len(), 2);
    assert_eq!(body["debounce_secs"], 30);
    assert_eq!(body["poll_interval_secs"], 300);

    watcher::sync(&state).await;
    let s = state.watchers.status(watched).unwrap();
    assert_eq!(s.mode, WatchMode::Inotify, "{s:?}");
    assert_eq!(
        state.watchers.status(unwatched).unwrap().mode,
        WatchMode::Disabled
    );

    let body = get_watchers(&app, &session, &csrf).await;
    let row = body["libraries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["library_id"] == watched.to_string())
        .unwrap()
        .clone();
    assert_eq!(row["watcher"]["mode"], "inotify");
    assert_eq!(row["file_watch_enabled"], true);
    assert!(row["watcher"]["started_at"].is_string());

    // Flip the toggle off: the next sync stops the watcher.
    let mut am: library::ActiveModel = library::Entity::find_by_id(watched)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.file_watch_enabled = Set(false);
    am.update(&state.db).await.unwrap();
    watcher::sync(&state).await;
    assert_eq!(
        state.watchers.status(watched).unwrap().mode,
        WatchMode::Disabled
    );

    // And back on, with a root that doesn't exist: disabled with a reason.
    let mut am: library::ActiveModel = library::Entity::find_by_id(watched)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.file_watch_enabled = Set(true);
    am.root_path = Set(tmp.path().join("gone").to_string_lossy().into_owned());
    am.update(&state.db).await.unwrap();
    watcher::sync(&state).await;
    let s = state.watchers.status(watched).unwrap();
    assert_eq!(s.mode, WatchMode::Disabled);
    assert!(
        s.detail
            .as_deref()
            .unwrap_or("")
            .contains("could not start"),
        "{s:?}"
    );
}

/// The scoped path inherits the behaviours main added to the shared scan
/// pipeline: a Duplicates-page soft-remove (WP-3.3) stays removed through a
/// scoped reconcile, and an archive-less folder that still holds a
/// `series.json` surfaces `OrphanedSeriesJson` (WP-3.4), not `EmptyFolder`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_scan_honours_duplicate_removals_and_orphaned_series_json() {
    use sea_orm::{ConnectionTrait, Statement};
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let a = root.join("Alpha (2024)");
    std::fs::create_dir_all(&a).unwrap();
    write_cbz(&a.join("Alpha (2024) 001.cbz"), 1);
    write_cbz(&a.join("Alpha (2024) 002.cbz"), 2);
    let lib = create_library(&app, &root, true).await;
    let state = app.state();
    server::library::scanner::scan_library(&state, lib)
        .await
        .unwrap();

    // Admin soft-removes issue 001 from the Duplicates page.
    let issue = entity::issue::Entity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib))
        .filter(entity::issue::Column::FilePath.contains("001"))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::issue::ActiveModel = issue.clone().into();
    am.removed_at = Set(Some(Utc::now().fixed_offset()));
    am.update(&state.db).await.unwrap();
    state
        .db
        .execute_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "INSERT INTO issue_duplicate_decision (issue_id, library_id, decision) \
             VALUES ($1, $2, 'remove')",
            [issue.id.clone().into(), lib.into()],
        ))
        .await
        .unwrap();

    // A new file in Alpha + an orphaned sidecar folder at the root.
    write_cbz(&a.join("Alpha (2024) 003.cbz"), 3);
    let orphan = root.join("Orphan (2020)");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(
        orphan.join("series.json"),
        br#"{"metadata":{"name":"Orphan"}}"#,
    )
    .unwrap();

    let s = server::library::scanner::scan_library_scoped(
        &state,
        lib,
        &[root.clone(), a.clone(), orphan.clone()],
        None,
    )
    .await
    .unwrap();
    assert_eq!(s.files_added, 1, "{s:?}");
    assert_eq!(
        s.issues_restored, 0,
        "duplicate soft-remove is sticky: {s:?}"
    );
    let still = entity::issue::Entity::find_by_id(issue.id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(still.removed_at.is_some());

    let kinds: Vec<String> = entity::library_health_issue::Entity::find()
        .filter(entity::library_health_issue::Column::LibraryId.eq(lib))
        .filter(entity::library_health_issue::Column::ResolvedAt.is_null())
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .map(|h| h.kind)
        .collect();
    assert!(
        kinds.iter().any(|k| k == "OrphanedSeriesJson"),
        "scoped enumeration reports the orphaned sidecar: {kinds:?}"
    );
    assert!(!kinds.iter().any(|k| k == "EmptyFolder"), "{kinds:?}");
}
