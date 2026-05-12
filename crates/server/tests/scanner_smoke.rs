//! Library Scanner v1 — Milestone 3 orchestration smoke test.
//!
//! Drives the new `scanner::scan_library` against synthetic CBZ fixtures and
//! asserts:
//!   - validation rejects a missing or empty root
//!   - well-formed series folders produce series + issue rows
//!   - file-at-root entries are detected and ignored (warning logged; full
//!     persistence lands in Milestone 5)
//!   - a second scan with no on-disk changes skips the folder via the
//!     `series.last_scanned_at` mtime gate (§4.4)

mod common;

use common::TestApp;
use entity::{
    issue::Entity as IssueEntity,
    library::{ActiveModel as LibraryAM, Entity as LibraryEntity},
    scan_run::Entity as ScanRunEntity,
    series::Entity as SeriesEntity,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, QueryFilter, Set,
    Statement,
};
use server::library::{events::ScanEvent, scanner};
use std::io::Write;
use std::path::Path;
use uuid::Uuid;

/// Write a CBZ whose contents include `unique_marker` bytes so two calls with
/// different markers produce different BLAKE3 hashes (and thus distinct issue
/// rows). Without this, content-deduplication makes every "test issue" collapse
/// into a single DB row.
fn write_minimal_cbz(path: &Path, comic_info: Option<&str>, unique_marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // Real PNG header so the cover-thumb best-effort step doesn't churn warnings,
    // followed by the marker bytes for content uniqueness.
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&unique_marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();

    if let Some(xml) = comic_info {
        zw.start_file("ComicInfo.xml", opts).unwrap();
        zw.write_all(xml.as_bytes()).unwrap();
    }
    zw.finish().unwrap();
}

async fn create_library(app: &TestApp, root: &Path) -> Uuid {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = chrono::Utc::now().fixed_offset();
    LibraryAM {
        id: Set(id),
        name: Set("Smoke Lib".into()),
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

fn drain_scan_events(rx: &mut tokio::sync::broadcast::Receiver<ScanEvent>) -> Vec<ScanEvent> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(evt) => out.push(evt),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            | Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
        }
    }
    out
}

#[tokio::test]
async fn scan_indexes_well_formed_series_folders() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    // Two series folders, two files each.
    let folder_a = tmp.path().join("Series Alpha (2020)");
    let folder_b = tmp.path().join("Series Beta (2021)");
    std::fs::create_dir_all(&folder_a).unwrap();
    std::fs::create_dir_all(&folder_b).unwrap();
    write_minimal_cbz(&folder_a.join("Series Alpha 001.cbz"), None, 1);
    write_minimal_cbz(&folder_a.join("Series Alpha 002.cbz"), None, 2);
    write_minimal_cbz(&folder_b.join("Series Beta 001.cbz"), None, 3);
    write_minimal_cbz(&folder_b.join("Series Beta 002.cbz"), None, 4);

    let lib_id = create_library(&app, tmp.path()).await;

    let state = app.state();
    let stats = scanner::scan_library(&state, lib_id).await.expect("scan");

    assert_eq!(stats.files_seen, 4, "expected 4 files seen, got {stats:?}");
    assert_eq!(stats.files_added, 4);
    assert_eq!(stats.series_created, 2);

    // Sanity-check rows landed in the DB.
    let series_count = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap()
        .len();
    let issue_count = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap()
        .len();
    assert_eq!(series_count, 2);
    assert_eq!(issue_count, 4);

    // library.last_scan_at should be populated after a successful run.
    let lib = LibraryEntity::find_by_id(lib_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(lib.last_scan_at.is_some());
}

#[tokio::test]
async fn full_scan_emits_planned_progress_before_file_processing() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let folder_a = tmp.path().join("Progress Alpha (2020)");
    let folder_b = tmp.path().join("Progress Beta (2021)");
    std::fs::create_dir_all(&folder_a).unwrap();
    std::fs::create_dir_all(&folder_b).unwrap();
    write_minimal_cbz(&folder_a.join("Alpha 001.cbz"), None, 1101);
    write_minimal_cbz(&folder_a.join("Alpha 002.cbz"), None, 1102);
    write_minimal_cbz(&folder_b.join("Beta 001.cbz"), None, 1103);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    let mut rx = state.events.subscribe();

    let stats = scanner::scan_library(&state, lib_id).await.expect("scan");
    assert_eq!(stats.files_added, 3);

    let events = drain_scan_events(&mut rx);
    let planning_idx = events
        .iter()
        .position(
            |e| matches!(e, ScanEvent::Progress { phase, .. } if *phase == "planning_complete"),
        )
        .expect("planning_complete progress event");
    let first_series_idx = events
        .iter()
        .position(|e| matches!(e, ScanEvent::SeriesUpdated { .. }))
        .expect("series activity event");
    assert!(
        planning_idx < first_series_idx,
        "planning totals should emit before series processing: {events:?}",
    );
    let planning = events
        .iter()
        .find_map(|e| match e {
            ScanEvent::Progress {
                library_id,
                kind,
                phase,
                total,
                series_total,
                files_total,
                ..
            } if *kind == "library" && *phase == "planning_complete" => {
                Some((*library_id, *total, *series_total, *files_total))
            }
            _ => None,
        })
        .expect("library progress event");
    assert_eq!(planning.0, lib_id);
    assert!(planning.1 > 1, "progress total should be determinate");
    assert_eq!(planning.2, 2);
    assert_eq!(planning.3, 3);

    let complete = events
        .iter()
        .rev()
        .find_map(|e| match e {
            ScanEvent::Progress {
                phase,
                completed,
                total,
                ..
            } if *phase == "complete" => Some((*completed, *total)),
            _ => None,
        })
        .expect("complete progress event");
    assert_eq!(complete.0, complete.1);
}

#[tokio::test]
async fn second_scan_skips_unchanged_folder() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let folder = tmp.path().join("Quiet Series (2022)");
    std::fs::create_dir_all(&folder).unwrap();
    write_minimal_cbz(&folder.join("Quiet 001.cbz"), None, 100);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    let first = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(first.files_added, 1);
    assert_eq!(first.series_skipped_unchanged, 0);

    let second = scanner::scan_library(&state, lib_id).await.unwrap();
    // Folder was untouched between scans → mtime gate trips, no re-walk.
    assert_eq!(
        second.series_skipped_unchanged, 1,
        "expected the folder to be skipped, got {second:?}",
    );
    assert_eq!(second.files_seen, 0);
    assert_eq!(second.files_added, 0);
}

#[tokio::test]
async fn force_library_scan_bypasses_folder_mtime_gate() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let folder = tmp.path().join("Force Series (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    write_minimal_cbz(&folder.join("Force 001.cbz"), None, 501);
    write_minimal_cbz(&folder.join("Force 002.cbz"), None, 502);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    let first = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(first.files_added, 2);

    let skipped = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(
        skipped.series_skipped_unchanged, 1,
        "control scan should hit the folder-level fast path: {skipped:?}",
    );
    assert_eq!(skipped.files_seen, 0);

    let forced = scanner::scan_library_with(&state, lib_id, true)
        .await
        .unwrap();
    assert_eq!(
        forced.series_skipped_unchanged, 0,
        "force=true must bypass folder-level mtime skipping: {forced:?}",
    );
    assert_eq!(forced.files_seen, 2, "forced scan should inspect files");
    // force=true now also bypasses the per-file fast path so unchanged files
    // are re-parsed (lets new parser fields like comicvine_id land without
    // touching mtimes). Both files get an `updated` write even though their
    // disk bytes didn't change.
    assert_eq!(
        forced.files_unchanged, 0,
        "force=true must bypass the per-file fast path: {forced:?}",
    );
    assert_eq!(
        forced.files_updated, 2,
        "forced scan must re-ingest: {forced:?}"
    );
}

/// User scenario: a previously-scanned library has issue rows missing the
/// new `comicvine_id` column because the parser didn't extract it at the
/// time. The fast-path check (size+mtime match) keeps a routine "Scan
/// issue" from re-reading the file. With `force=true`, the scan re-parses
/// and the ID lands on the row.
#[tokio::test]
async fn scan_issue_force_re_extracts_new_parser_fields() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let folder = tmp.path().join("Backfill (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("Backfill 001.cbz");
    let xml = r#"<?xml version="1.0"?><ComicInfo><Series>Backfill</Series><Number>1</Number><ComicVineID>4242</ComicVineID></ComicInfo>"#;
    write_minimal_cbz(&file, Some(xml), 1);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    // Initial scan — issue row is created with comicvine_id=4242 from the
    // parser. (Sanity check that the parser path is wired up.)
    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(stats.files_added, 1);

    let issue_row = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .expect("issue row");
    assert_eq!(issue_row.comicvine_id, Some(4242));

    // Simulate "scanned with an older parser that didn't extract IDs" by
    // clearing the column on the row. The on-disk file still has the
    // ComicVineID tag; only the row is stale.
    let mut am: entity::issue::ActiveModel = issue_row.clone().into();
    am.comicvine_id = Set(None);
    am.update(&state.db).await.unwrap();

    // force=false should hit the per-file fast path (size+mtime unchanged)
    // and leave the column null.
    let stats = scanner::scan_issue_file(&state, lib_id, &issue_row.id, false, None)
        .await
        .unwrap();
    assert_eq!(
        stats.files_unchanged, 1,
        "default scan should hit the fast path: {stats:?}",
    );
    let row_after_default = IssueEntity::find_by_id(issue_row.id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row_after_default.comicvine_id, None,
        "fast-path scan must not re-extract",
    );

    // force=true re-parses the file even though size+mtime match, and the
    // parser populates comicvine_id from the XML tag.
    let stats = scanner::scan_issue_file(&state, lib_id, &issue_row.id, true, None)
        .await
        .unwrap();
    assert_eq!(
        stats.files_updated, 1,
        "forced scan must re-ingest: {stats:?}"
    );
    assert_eq!(
        stats.files_unchanged, 0,
        "force must skip fast-path: {stats:?}"
    );
    let row_after_force = IssueEntity::find_by_id(issue_row.id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row_after_force.comicvine_id,
        Some(4242),
        "forced scan must repopulate comicvine_id",
    );
}

#[tokio::test]
async fn requested_scan_run_id_is_persisted() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let folder = tmp.path().join("Stable Id Series (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    write_minimal_cbz(&folder.join("Stable 001.cbz"), None, 601);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    let requested = Uuid::now_v7();

    let stats = scanner::scan_library_with_run_id(&state, lib_id, false, Some(requested))
        .await
        .expect("scan");
    assert_eq!(stats.files_added, 1);

    let row = ScanRunEntity::find_by_id(requested)
        .one(&state.db)
        .await
        .unwrap()
        .expect("scan run row should use requested id");
    assert_eq!(row.id, requested);
    assert_eq!(row.library_id, lib_id);
    assert_eq!(row.kind, "library");
    assert_eq!(row.state, "complete");
}

#[tokio::test]
async fn parallel_series_scan_merges_stats_across_folders() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let mut marker = 700;
    for series_idx in 0..4 {
        let folder = tmp
            .path()
            .join(format!("Parallel Series {series_idx} (2024)"));
        std::fs::create_dir_all(&folder).unwrap();
        for issue_idx in 0..3 {
            marker += 1;
            write_minimal_cbz(
                &folder.join(format!("Parallel {series_idx}-{issue_idx:03}.cbz")),
                None,
                marker,
            );
        }
    }

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    let stats = scanner::scan_library(&state, lib_id).await.expect("scan");

    assert_eq!(
        stats.files_seen, 12,
        "stats should merge all folders: {stats:?}"
    );
    assert_eq!(
        stats.files_added, 12,
        "stats should merge all inserts: {stats:?}"
    );
    assert_eq!(
        stats.series_created, 4,
        "stats should merge per-folder series creation: {stats:?}",
    );
    assert_eq!(stats.files_updated, 0);
    assert_eq!(stats.files_duplicate, 0);

    let issue_count = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap()
        .len();
    let series_count = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap()
        .len();
    assert_eq!(issue_count, 12);
    assert_eq!(series_count, 4);
}

#[tokio::test]
async fn validation_rejects_missing_root() {
    let app = TestApp::spawn().await;
    let lib_id = create_library(&app, Path::new("/tmp/folio-scanner-smoke-does-not-exist")).await;
    let state = app.state();

    let err = scanner::scan_library(&state, lib_id)
        .await
        .expect_err("expected validation error");
    let msg = err.to_string();
    assert!(
        msg.contains("does not exist") || msg.contains("not exist"),
        "unexpected error: {msg}",
    );
}

#[tokio::test]
async fn scan_series_folder_is_narrow_and_leaves_siblings_untouched() {
    // Library Scanner v1, Milestone 3: per-series narrow path.
    //
    // After a full library scan, the per-series scan path should:
    //   - re-process its own folder (force=true, so even an unchanged folder
    //     is re-walked when the user asks for a refresh)
    //   - NOT touch the sibling folder's series row
    //   - reconcile only its own issues (sibling's issues stay active)
    //   - NOT bump library.last_scan_at (which is the cron's "last full
    //     scan" indicator)
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let folder_a = tmp.path().join("Series Alpha (2020)");
    let folder_b = tmp.path().join("Series Beta (2021)");
    std::fs::create_dir_all(&folder_a).unwrap();
    std::fs::create_dir_all(&folder_b).unwrap();
    write_minimal_cbz(&folder_a.join("Alpha 001.cbz"), None, 11);
    write_minimal_cbz(&folder_a.join("Alpha 002.cbz"), None, 12);
    write_minimal_cbz(&folder_b.join("Beta 001.cbz"), None, 21);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    // Initial full scan populates two series.
    scanner::scan_library(&state, lib_id).await.unwrap();
    let lib_before = LibraryEntity::find_by_id(lib_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let last_scan_before = lib_before.last_scan_at;
    assert!(last_scan_before.is_some());

    let alpha = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .filter(entity::series::Column::FolderPath.eq(folder_a.to_string_lossy()))
        .one(&state.db)
        .await
        .unwrap()
        .expect("alpha series row");
    let beta = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .filter(entity::series::Column::FolderPath.eq(folder_b.to_string_lossy()))
        .one(&state.db)
        .await
        .unwrap()
        .expect("beta series row");

    // Drop one of Alpha's files on disk before the narrow scan to verify
    // reconcile_series soft-deletes only Alpha's missing row.
    std::fs::remove_file(folder_a.join("Alpha 002.cbz")).unwrap();

    let stats = scanner::scan_series_folder(
        &state,
        lib_id,
        alpha.id,
        &folder_a,
        scanner::ScanKind::Series,
        None,
        false,
        None,
    )
    .await
    .expect("narrow scan");

    // The remaining Alpha file is re-processed (force=true), the missing one
    // is soft-deleted. Beta is untouched.
    assert_eq!(
        stats.issues_removed, 1,
        "expected 1 issue soft-deleted for Alpha, got {stats:?}",
    );

    let alpha_active = IssueEntity::find()
        .filter(entity::issue::Column::SeriesId.eq(alpha.id))
        .filter(entity::issue::Column::RemovedAt.is_null())
        .all(&state.db)
        .await
        .unwrap()
        .len();
    let alpha_removed = IssueEntity::find()
        .filter(entity::issue::Column::SeriesId.eq(alpha.id))
        .filter(entity::issue::Column::RemovedAt.is_not_null())
        .all(&state.db)
        .await
        .unwrap()
        .len();
    let beta_active = IssueEntity::find()
        .filter(entity::issue::Column::SeriesId.eq(beta.id))
        .filter(entity::issue::Column::RemovedAt.is_null())
        .all(&state.db)
        .await
        .unwrap()
        .len();
    assert_eq!(alpha_active, 1, "alpha should have 1 active issue left");
    assert_eq!(
        alpha_removed, 1,
        "alpha's missing file should be soft-deleted"
    );
    assert_eq!(beta_active, 1, "beta must be untouched by the narrow scan");

    // library.last_scan_at must be unchanged — narrow scans don't mark the
    // library as fully scanned.
    let lib_after = LibraryEntity::find_by_id(lib_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        lib_after.last_scan_at, last_scan_before,
        "narrow scan should not bump library.last_scan_at",
    );
}

#[tokio::test]
async fn series_scan_progress_totals_are_scoped_to_one_series() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let folder_a = tmp.path().join("Scoped Alpha (2020)");
    let folder_b = tmp.path().join("Scoped Beta (2021)");
    std::fs::create_dir_all(&folder_a).unwrap();
    std::fs::create_dir_all(&folder_b).unwrap();
    write_minimal_cbz(&folder_a.join("Alpha 001.cbz"), None, 1201);
    write_minimal_cbz(&folder_a.join("Alpha 002.cbz"), None, 1202);
    write_minimal_cbz(&folder_b.join("Beta 001.cbz"), None, 1203);
    write_minimal_cbz(&folder_b.join("Beta 002.cbz"), None, 1204);
    write_minimal_cbz(&folder_b.join("Beta 003.cbz"), None, 1205);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let alpha = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .filter(entity::series::Column::FolderPath.eq(folder_a.to_string_lossy()))
        .one(&state.db)
        .await
        .unwrap()
        .expect("alpha series row");

    let mut rx = state.events.subscribe();
    scanner::scan_series_folder(
        &state,
        lib_id,
        alpha.id,
        &folder_a,
        scanner::ScanKind::Series,
        None,
        false,
        None,
    )
    .await
    .expect("series scan");

    let events = drain_scan_events(&mut rx);
    let scoped = events
        .iter()
        .find_map(|e| match e {
            ScanEvent::Progress {
                kind,
                phase,
                library_id,
                series_total,
                files_total,
                ..
            } if *kind == "series" && *phase == "planning_complete" => {
                Some((*library_id, *series_total, *files_total))
            }
            _ => None,
        })
        .expect("series planning progress");
    assert_eq!(scoped.0, lib_id);
    assert_eq!(scoped.1, 1);
    assert_eq!(scoped.2, 2);
}

#[tokio::test]
async fn issue_scan_progress_total_is_one() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Issue Progress (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    write_minimal_cbz(&folder.join("Issue Progress 001.cbz"), None, 1301);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let issue = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .expect("issue row");

    let mut rx = state.events.subscribe();
    scanner::scan_issue_file(&state, lib_id, &issue.id, false, None)
        .await
        .expect("issue scan");

    let events = drain_scan_events(&mut rx);
    let progress: Vec<(u64, u64)> = events
        .iter()
        .filter_map(|e| match e {
            ScanEvent::Progress {
                kind,
                completed,
                total,
                ..
            } if *kind == "issue" => Some((*completed, *total)),
            _ => None,
        })
        .collect();
    assert!(!progress.is_empty(), "expected issue progress events");
    assert!(
        progress.iter().all(|(_, total)| *total == 1),
        "issue scan progress should use total=1: {progress:?}",
    );
    assert_eq!(progress.last().copied(), Some((1, 1)));
}

#[tokio::test]
async fn scan_series_rescan_is_idempotent_for_unchanged_files() {
    // Regression test for the file_mtime precision bug: Postgres `timestamptz`
    // truncates to microseconds while Linux fs mtime is nanosecond-precision,
    // so without truncation on the Rust side the per-file fast path
    // (size+mtime match → skip) never fires on a second per-series scan.
    //
    // Symptoms of the bug were: files_updated=N (instead of files_unchanged=N)
    // every rescan, plus N cover thumbs regenerated on each pass even when
    // nothing on disk changed. Both reflect wasted work, so we assert the
    // counters explicitly.
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Idempotent Series (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    write_minimal_cbz(&folder.join("Idempotent 001.cbz"), None, 401);
    write_minimal_cbz(&folder.join("Idempotent 002.cbz"), None, 402);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    // Initial library scan to seed series + issues.
    let first = scanner::scan_library(&state, lib_id)
        .await
        .expect("first scan");
    assert_eq!(first.files_added, 2);

    let row = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .expect("series row");

    // Second pass: scan_series_folder forces past the folder-level mtime
    // gate, so each file falls back on the per-file fast path. With the
    // fix in place all files report `files_unchanged`, no thumbs regen.
    let second = scanner::scan_series_folder(
        &state,
        lib_id,
        row.id,
        &folder,
        scanner::ScanKind::Series,
        None,
        false,
        None,
    )
    .await
    .expect("rescan");

    assert_eq!(second.files_seen, 2, "stats: {second:?}");
    assert_eq!(
        second.files_unchanged, 2,
        "both files should hit the unchanged fast path: {second:?}",
    );
    assert_eq!(second.files_added, 0);
    assert_eq!(second.files_updated, 0);
    assert_eq!(
        second.thumbs_generated, 0,
        "no thumbs should regenerate on an unchanged rescan: {second:?}",
    );
}

#[tokio::test]
async fn duplicate_content_is_skipped_and_reported() {
    // Regression test for the silent-duplicate-rejection bug:
    //
    // Before the fix, copying an existing CBZ to a new filename inside the
    // same series folder caused the second file's INSERT to fail with a PK
    // violation on `id` (the content hash). The whole batch rolled back so
    // even genuine work on sibling files in the chunk was lost, and the
    // duplicate was completely invisible to the History tab.
    //
    // After the fix, the duplicate is detected by hash before INSERT, a
    // `DuplicateContent` health issue is emitted, `files_duplicate`
    // increments, and the chunk continues normally.
    use entity::library_health_issue::Entity as HealthEntity;
    use sea_orm::ColumnTrait;

    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Dupe Series (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    write_minimal_cbz(&folder.join("Dupe 001.cbz"), None, 901);
    write_minimal_cbz(&folder.join("Dupe 002.cbz"), None, 902);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    let first = scanner::scan_library(&state, lib_id)
        .await
        .expect("first scan");
    assert_eq!(first.files_added, 2);

    // Drop a content-identical copy of issue 001 next to the originals.
    std::fs::copy(
        folder.join("Dupe 001.cbz"),
        folder.join("Dupe 001 (copy).cbz"),
    )
    .unwrap();

    // Force the per-series narrow path so the per-folder mtime gate isn't
    // what skips the duplicate — it has to be the content-hash check.
    let row = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .expect("series row");
    let stats = scanner::scan_series_folder(
        &state,
        lib_id,
        row.id,
        &folder,
        scanner::ScanKind::Series,
        None,
        false,
        None,
    )
    .await
    .expect("rescan");

    assert_eq!(stats.files_seen, 3, "stats: {stats:?}");
    assert_eq!(stats.files_duplicate, 1, "stats: {stats:?}");
    assert_eq!(stats.files_unchanged, 2, "stats: {stats:?}");
    assert_eq!(stats.files_added, 0, "stats: {stats:?}");
    assert_eq!(stats.files_updated, 0, "stats: {stats:?}");

    // Issue rows still in place (the originals); the duplicate didn't
    // create or destroy anything.
    let issue_count = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap()
        .len();
    assert_eq!(issue_count, 2);

    // A DuplicateContent health row was persisted by the scan.
    let health_kinds: Vec<String> = HealthEntity::find()
        .filter(entity::library_health_issue::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .map(|h| h.kind)
        .collect();
    assert!(
        health_kinds.iter().any(|k| k == "DuplicateContent"),
        "expected DuplicateContent in {health_kinds:?}",
    );
}

#[tokio::test]
async fn renamed_issue_updates_primary_path_alias() {
    #[derive(Debug, FromQueryResult)]
    struct PathRow {
        file_path: String,
        is_primary: bool,
        missing: bool,
    }

    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Moved Series (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    let old_path = folder.join("Moved 001.cbz");
    let new_path = folder.join("Moved 001 renamed.cbz");
    let old_path_str = old_path.to_string_lossy().into_owned();
    let new_path_str = new_path.to_string_lossy().into_owned();
    write_minimal_cbz(&old_path, None, 931);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    let first = scanner::scan_library(&state, lib_id)
        .await
        .expect("first scan");
    assert_eq!(first.files_added, 1);

    std::fs::rename(&old_path, &new_path).unwrap();

    let row = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .expect("series row");
    let stats = scanner::scan_series_folder(
        &state,
        lib_id,
        row.id,
        &folder,
        scanner::ScanKind::Series,
        None,
        false,
        None,
    )
    .await
    .expect("rescan");

    assert_eq!(stats.files_seen, 1, "stats: {stats:?}");
    assert_eq!(stats.files_duplicate, 0, "stats: {stats:?}");
    assert_eq!(stats.files_added, 0, "stats: {stats:?}");
    assert_eq!(stats.files_updated, 1, "stats: {stats:?}");

    let issues = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].file_path, new_path_str);

    let rows = PathRow::find_by_statement(Statement::from_sql_and_values(
        state.db.get_database_backend(),
        r"SELECT file_path, is_primary, missing_at IS NOT NULL AS missing
            FROM issue_paths
            WHERE issue_id = $1
            ORDER BY file_path",
        [issues[0].id.clone().into()],
    ))
    .all(&state.db)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "issue_paths rows: {rows:?}");
    assert!(
        rows.iter()
            .any(|row| { row.file_path == old_path_str && !row.is_primary && row.missing })
    );
    assert!(
        rows.iter()
            .any(|row| { row.file_path == new_path_str && row.is_primary && !row.missing })
    );
}

#[tokio::test]
async fn scan_series_folder_rejects_outside_root() {
    // The narrow path validates that the folder lives under the library
    // root — a payload pointing outside (e.g. a stale folder_path after
    // someone moved the library) is rejected with a clear error rather
    // than scanning an unintended directory.
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();

    let folder = tmp.path().join("Series Foo (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    write_minimal_cbz(&folder.join("Foo 001.cbz"), None, 30);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let row = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();

    // Pretend the series folder was moved to a sibling tmp dir that
    // happens to exist. The narrow scan should refuse it.
    let bogus = other.path().join("Series Foo (2024)");
    std::fs::create_dir_all(&bogus).unwrap();
    let err = scanner::scan_series_folder(
        &state,
        lib_id,
        row.id,
        &bogus,
        scanner::ScanKind::Series,
        None,
        false,
        None,
    )
    .await
    .expect_err("expected outside-root rejection");
    let msg = err.to_string();
    assert!(
        msg.contains("not inside the library root"),
        "unexpected error: {msg}",
    );
}

#[tokio::test]
async fn files_at_root_are_ignored_not_indexed() {
    use entity::library_health_issue::Entity as HealthEntity;

    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    // Wrong layout: a CBZ directly at the library root.
    write_minimal_cbz(&tmp.path().join("orphan.cbz"), None, 200);
    // Plus one well-formed series folder so the root isn't empty.
    let folder = tmp.path().join("Series Gamma (2023)");
    std::fs::create_dir_all(&folder).unwrap();
    write_minimal_cbz(&folder.join("Gamma 001.cbz"), None, 201);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    let mut rx = state.events.subscribe();
    let stats = scanner::scan_library(&state, lib_id).await.unwrap();

    // Only the in-folder file is processed.
    assert_eq!(stats.files_added, 1);
    assert_eq!(stats.series_created, 1);

    let events = drain_scan_events(&mut rx);
    let health_evt = events
        .iter()
        .find_map(|e| match e {
            ScanEvent::HealthIssue {
                library_id,
                scan_id,
                kind,
                severity,
                path,
            } if kind == "FileAtRoot" => {
                Some((*library_id, *scan_id, severity.clone(), path.clone()))
            }
            _ => None,
        })
        .expect("live FileAtRoot health event");
    assert_eq!(health_evt.0, lib_id);
    assert_eq!(health_evt.2, "warning");
    assert!(
        health_evt
            .3
            .as_deref()
            .unwrap_or_default()
            .ends_with("orphan.cbz"),
        "unexpected event path: {health_evt:?}",
    );

    let persisted = HealthEntity::find()
        .filter(entity::library_health_issue::Column::LibraryId.eq(lib_id))
        .filter(entity::library_health_issue::Column::ScanId.eq(Some(health_evt.1)))
        .filter(entity::library_health_issue::Column::Kind.eq("FileAtRoot"))
        .one(&state.db)
        .await
        .unwrap();
    assert!(persisted.is_some(), "health event should still persist");
}

// ────────────── Phase A: dimension probe + double-page inference ──────────────

/// Encode a solid-black PNG of the given dimensions. Used by the inference
/// test to plant pages with known aspect ratios in a synthetic CBZ.
fn encode_solid_png(w: u32, h: u32) -> Vec<u8> {
    let img = image::ImageBuffer::<image::Rgb<u8>, _>::from_pixel(w, h, image::Rgb([0, 0, 0]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode png");
    out.into_inner()
}

fn write_cbz_with_pages(path: &Path, pages: &[(&str, Vec<u8>)], comic_info: Option<&str>) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in pages {
        zw.start_file(*name, opts).unwrap();
        zw.write_all(bytes).unwrap();
    }
    if let Some(xml) = comic_info {
        zw.start_file("ComicInfo.xml", opts).unwrap();
        zw.write_all(xml.as_bytes()).unwrap();
    }
    zw.finish().unwrap();
}

#[tokio::test]
async fn missing_comicinfo_synthesizes_pages_with_inferred_doubles() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    // Three pages: portrait single, landscape spread (2× width = clear
    // double), portrait single. No ComicInfo.xml, so the scanner relies
    // entirely on the dimension probe to populate `pages_json`.
    let pages: Vec<(&str, Vec<u8>)> = vec![
        ("page-001.png", encode_solid_png(1000, 1500)),
        ("page-002.png", encode_solid_png(2000, 1500)),
        ("page-003.png", encode_solid_png(1000, 1500)),
    ];
    let folder = tmp.path().join("No ComicInfo Series (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    let cbz = folder.join("No ComicInfo 001.cbz");
    write_cbz_with_pages(&cbz, &pages, None);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(stats.files_added, 1);

    // The synthesized pages JSON should have one entry per archive page,
    // with widths/heights populated and the spread flagged.
    let issue = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .expect("issue row");
    let pages: Vec<parsers::comicinfo::PageInfo> =
        serde_json::from_value(issue.pages.clone()).expect("pages array");
    assert_eq!(pages.len(), 3, "synthesized pages count");

    assert_eq!(pages[0].image_width, Some(1000));
    assert_eq!(pages[0].image_height, Some(1500));
    assert_eq!(pages[0].double_page, Some(false));
    assert_eq!(pages[0].double_page_inferred, None);

    assert_eq!(pages[1].image_width, Some(2000));
    assert_eq!(pages[1].image_height, Some(1500));
    assert_eq!(pages[1].double_page, Some(true));
    assert_eq!(pages[1].double_page_inferred, Some(true));

    assert_eq!(pages[2].image_width, Some(1000));
    assert_eq!(pages[2].double_page, Some(false));
    assert_eq!(pages[2].double_page_inferred, None);
}

#[tokio::test]
async fn comicinfo_without_doublepage_attr_gets_inferred_when_dims_show_spread() {
    // Mirrors the Geiger-004 case: ComicInfo.xml is present and lists each
    // page (so info.pages is populated), but the publisher omitted the
    // DoublePage attribute. The probe must backfill it from pixel dims.
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let comic_info = r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Geiger-Like</Series>
  <Number>4</Number>
  <PageCount>3</PageCount>
  <Pages>
    <Page Image="0" Type="FrontCover"/>
    <Page Image="1"/>
    <Page Image="2"/>
  </Pages>
</ComicInfo>"#;

    let pages: Vec<(&str, Vec<u8>)> = vec![
        ("page-001.png", encode_solid_png(1000, 1500)),
        ("page-002.png", encode_solid_png(2000, 1500)),
        ("page-003.png", encode_solid_png(1000, 1500)),
    ];
    let folder = tmp.path().join("Inferred Double Series (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    let cbz = folder.join("Inferred 001.cbz");
    write_cbz_with_pages(&cbz, &pages, Some(comic_info));

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let issue = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .expect("issue row");
    let pages: Vec<parsers::comicinfo::PageInfo> =
        serde_json::from_value(issue.pages.clone()).expect("pages array");

    // Existing ComicInfo entries are kept (Type=FrontCover survives), and
    // probe results are merged in by `image` index, so we end up with the
    // original Type metadata plus inferred double-page on page 1.
    assert_eq!(pages.len(), 3);
    assert_eq!(pages[0].kind.as_deref(), Some("FrontCover"));
    assert_eq!(pages[1].double_page, Some(true));
    assert_eq!(pages[1].double_page_inferred, Some(true));
    // Singles must NOT be marked as inferred — only the positive result
    // carries the flag, so admin tooling can distinguish "we made this up"
    // from "publisher said no".
    assert_eq!(pages[0].double_page_inferred, None);
    assert_eq!(pages[2].double_page_inferred, None);
}

#[tokio::test]
async fn declared_doublepage_is_not_overridden_by_probe() {
    // When ComicInfo explicitly declares `DoublePage="false"` on a page
    // that has spread-shaped dimensions, the publisher's call wins. The
    // probe only fills in null values.
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let comic_info = r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Declared Lies</Series>
  <Number>1</Number>
  <PageCount>1</PageCount>
  <Pages>
    <Page Image="0" DoublePage="false"/>
  </Pages>
</ComicInfo>"#;

    let pages: Vec<(&str, Vec<u8>)> = vec![("page-001.png", encode_solid_png(2400, 1500))];
    let folder = tmp.path().join("Declared (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    let cbz = folder.join("Declared 001.cbz");
    write_cbz_with_pages(&cbz, &pages, Some(comic_info));

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let issue = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .expect("issue row");
    let pages: Vec<parsers::comicinfo::PageInfo> =
        serde_json::from_value(issue.pages.clone()).expect("pages array");
    assert_eq!(
        pages[0].double_page,
        Some(false),
        "publisher's `false` wins"
    );
    assert_eq!(pages[0].double_page_inferred, None);
    // Width/height are still backfilled — the probe doesn't compete with
    // ComicInfo here.
    assert_eq!(pages[0].image_width, Some(2400));
}
