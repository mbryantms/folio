//! Observability split M3 — end-to-end: a real library scan writes a durable,
//! itemized manifest to `library_events`.
//!
//! Asserts the scan path emits:
//!   - scan lifecycle (started + completed)
//!   - one `issue/added` row per new file
//!   - one `series/added` row per new series
//!   - an `issue/removed` row when a file disappears on a re-scan

mod common;

use common::TestApp;
use entity::library::ActiveModel as LibraryAM;
use entity::library_event::{Column as EventCol, Entity as EventEntity, Model as EventModel};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::library::scanner;
use std::io::Write;
use std::path::Path;
use uuid::Uuid;

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

async fn create_library(app: &TestApp, root: &Path) -> Uuid {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = chrono::Utc::now().fixed_offset();
    LibraryAM {
        id: Set(id),
        name: Set("Event Scan Lib".into()),
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

async fn events_for(app: &TestApp, lib: Uuid) -> Vec<EventModel> {
    EventEntity::find()
        .filter(EventCol::LibraryId.eq(lib))
        .all(&app.state().db)
        .await
        .unwrap()
}

fn count(events: &[EventModel], category: &str, action: &str) -> usize {
    events
        .iter()
        .filter(|e| e.category == category && e.action == action)
        .count()
}

#[tokio::test]
async fn scan_writes_itemized_manifest() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder_a = tmp.path().join("Series Alpha (2020)");
    let folder_b = tmp.path().join("Series Beta (2021)");
    std::fs::create_dir_all(&folder_a).unwrap();
    std::fs::create_dir_all(&folder_b).unwrap();
    write_minimal_cbz(&folder_a.join("Series Alpha 001.cbz"), 1);
    write_minimal_cbz(&folder_a.join("Series Alpha 002.cbz"), 2);
    write_minimal_cbz(&folder_b.join("Series Beta 001.cbz"), 3);

    let lib = create_library(&app, tmp.path()).await;
    let state = app.state();
    let stats = scanner::scan_library(&state, lib).await.expect("scan");
    assert_eq!(stats.files_added, 3);
    assert_eq!(stats.series_created, 2);

    let events = events_for(&app, lib).await;
    // Lifecycle.
    assert_eq!(count(&events, "scan", "started"), 1, "{events:?}");
    assert_eq!(count(&events, "scan", "completed"), 1, "{events:?}");
    // One manifest row per added entity.
    assert_eq!(count(&events, "issue", "added"), 3, "{events:?}");
    assert_eq!(count(&events, "series", "added"), 2, "{events:?}");

    // Every issue/added row carries a resolvable entity + a scan link.
    let added: Vec<&EventModel> = events
        .iter()
        .filter(|e| e.category == "issue" && e.action == "added")
        .collect();
    for e in &added {
        assert_eq!(e.entity_type.as_deref(), Some("issue"));
        assert!(e.entity_id.is_some());
        assert!(e.scan_run_id.is_some());
        assert!(e.detail.is_some());
    }
}

#[tokio::test]
async fn rescan_after_delete_emits_removed_event() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Gamma (2022)");
    std::fs::create_dir_all(&folder).unwrap();
    let f1 = folder.join("Gamma 001.cbz");
    let f2 = folder.join("Gamma 002.cbz");
    write_minimal_cbz(&f1, 10);
    write_minimal_cbz(&f2, 11);

    let lib = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib)
        .await
        .expect("first scan");

    // Drop one file and re-scan: reconcile should soft-delete it and log a
    // removed event.
    std::fs::remove_file(&f2).unwrap();
    scanner::scan_library(&state, lib)
        .await
        .expect("second scan");

    let events = events_for(&app, lib).await;
    assert!(
        count(&events, "issue", "removed") >= 1,
        "expected an issue/removed manifest row, got {events:?}",
    );
}

/// Batch stamping regression (2026-07-02): `EventCollector::with_batch`
/// existed since M5 but had zero callers — every event landed with
/// `batch_id = NULL` and the scan-batch "Changes" manifest (M10) was
/// permanently empty. Mimic a "Scan all" (pre-inserted queued run with a
/// `batch_id`) and assert every event the scan writes carries the batch.
#[tokio::test]
async fn batch_scan_events_carry_batch_id() {
    use entity::{scan_batch, scan_run};

    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Batch (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    write_minimal_cbz(&folder.join("Batch 001.cbz"), 1);

    let lib = create_library(&app, tmp.path()).await;
    let state = app.state();
    let now = chrono::Utc::now().fixed_offset();

    // Pre-insert the batch + queued member run, exactly like the
    // "Scan all" endpoint (M6) does before enqueueing the jobs.
    let batch_id = Uuid::now_v7();
    scan_batch::ActiveModel {
        id: Set(batch_id),
        kind: Set("scan_all".into()),
        actor_id: Set(None),
        force: Set(false),
        started_at: Set(now),
        ended_at: Set(None),
        library_count: Set(1),
        state: Set("running".into()),
    }
    .insert(&state.db)
    .await
    .unwrap();
    let scan_id = Uuid::now_v7();
    scan_run::ActiveModel {
        id: Set(scan_id),
        library_id: Set(lib),
        state: Set("queued".into()),
        started_at: Set(now),
        ended_at: Set(None),
        stats: Set(serde_json::json!({})),
        error: Set(None),
        kind: Set("library".into()),
        series_id: Set(None),
        issue_id: Set(None),
        batch_id: Set(Some(batch_id)),
    }
    .insert(&state.db)
    .await
    .unwrap();

    scanner::scan_library_with_run_id(&state, lib, false, Some(scan_id))
        .await
        .expect("scan succeeds");

    let events = events_for(&app, lib).await;
    assert!(!events.is_empty(), "scan wrote events");
    let unstamped: Vec<_> = events
        .iter()
        .filter(|e| e.batch_id != Some(batch_id))
        .collect();
    assert!(
        unstamped.is_empty(),
        "every event must carry the batch id; unstamped: {unstamped:?}"
    );

    // Ordinary single-library scans stay batch-less.
    let tmp2 = tempfile::tempdir().unwrap();
    let folder2 = tmp2.path().join("Series Solo (2025)");
    std::fs::create_dir_all(&folder2).unwrap();
    write_minimal_cbz(&folder2.join("Solo 001.cbz"), 2);
    let lib2 = create_library(&app, tmp2.path()).await;
    scanner::scan_library(&state, lib2).await.expect("scan");
    let events2 = events_for(&app, lib2).await;
    assert!(!events2.is_empty());
    assert!(
        events2.iter().all(|e| e.batch_id.is_none()),
        "solo scans must not invent a batch"
    );
}
