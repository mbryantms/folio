//! Observability split M2 — durable library-event writer surface.
//!
//! Validates the `event_log::{record, record_many}` write path against a real
//! DB:
//!   - a single `record` persists with all fields round-tripping
//!   - `record_many` bulk-inserts in one shot
//!   - empty `record_many` is a no-op (doesn't error)
//!   - the `library_id` FK is enforced
//!   - every `Severity` variant satisfies the DB CHECK constraint

mod common;

use common::TestApp;
use entity::library::ActiveModel as LibraryAM;
use entity::library_event::{ActiveModel as EventAM, Column as EventCol, Entity as EventEntity};
use entity::scan_run::ActiveModel as ScanRunAM;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};
use server::library::event_log::{self, Action, Category, NewEvent, Severity};
use uuid::Uuid;

/// Insert a minimal `scan_runs` row so events can reference it without
/// tripping the `library_events_scan_run_fk` constraint.
async fn create_scan_run(db: &DatabaseConnection, library_id: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    ScanRunAM {
        id: Set(id),
        library_id: Set(library_id),
        state: Set("complete".into()),
        started_at: Set(chrono::Utc::now().fixed_offset()),
        ended_at: Set(None),
        stats: Set(serde_json::json!({})),
        error: Set(None),
        kind: Set("library".into()),
        series_id: Set(None),
        issue_id: Set(None),
        batch_id: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn create_library(app: &TestApp) -> Uuid {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = chrono::Utc::now().fixed_offset();
    LibraryAM {
        id: Set(id),
        name: Set("Event Lib".into()),
        root_path: Set(format!("/tmp/event-lib-{id}")),
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
        auto_convert_cbr_on_scan: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn record_persists_a_single_event_with_all_fields() {
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let lib = create_library(&app).await;
    let scan = create_scan_run(&db, lib).await;

    event_log::record(
        &db,
        NewEvent::new(
            lib,
            Category::Issue,
            Action::Added,
            Severity::Info,
            "Added issue Saga #1",
        )
        .scan_run(scan)
        .entity("issue", "issue-abc", Some("Saga #1".to_owned()))
        .detail(serde_json::json!({"page_count": 24})),
    )
    .await;

    let rows = EventEntity::find()
        .filter(EventCol::LibraryId.eq(lib))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.scan_run_id, Some(scan));
    assert_eq!(row.batch_id, None);
    assert_eq!(row.category, "issue");
    assert_eq!(row.action, "added");
    assert_eq!(row.severity, "info");
    assert_eq!(row.entity_type.as_deref(), Some("issue"));
    assert_eq!(row.entity_id.as_deref(), Some("issue-abc"));
    assert_eq!(row.entity_label.as_deref(), Some("Saga #1"));
    assert_eq!(row.summary, "Added issue Saga #1");
    assert_eq!(
        row.detail.as_ref().and_then(|d| d.get("page_count")),
        Some(&serde_json::json!(24)),
    );
}

#[tokio::test]
async fn record_many_bulk_inserts_all_severities() {
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let lib = create_library(&app).await;

    let events = vec![
        NewEvent::new(lib, Category::Series, Action::Updated, Severity::Info, "s1"),
        NewEvent::new(
            lib,
            Category::Thumbnail,
            Action::Errored,
            Severity::Warning,
            "thumb failed",
        ),
        NewEvent::new(
            lib,
            Category::Archive,
            Action::Errored,
            Severity::Error,
            "rewrite failed",
        ),
    ];
    event_log::record_many(&db, events).await;

    let rows = EventEntity::find()
        .filter(EventCol::LibraryId.eq(lib))
        .order_by_asc(EventCol::Severity)
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    let severities: Vec<&str> = rows.iter().map(|r| r.severity.as_str()).collect();
    // All three CHECK-admitted values landed.
    assert!(severities.contains(&"info"));
    assert!(severities.contains(&"warning"));
    assert!(severities.contains(&"error"));
}

#[tokio::test]
async fn record_with_dangling_scan_run_is_dropped_silently() {
    // The scan_run FK is enforced. A caller that references a non-existent
    // scan run gets a logged error and a dropped row — never a panic or a
    // bubbled error (fire-and-forget). M3 call sites always emit after the
    // scan_run row exists, so this is a guard, not a normal path.
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let lib = create_library(&app).await;

    event_log::record(
        &db,
        NewEvent::new(lib, Category::Scan, Action::Started, Severity::Info, "x")
            .scan_run(Uuid::now_v7()),
    )
    .await;

    let count = EventEntity::find()
        .filter(EventCol::LibraryId.eq(lib))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn record_many_empty_is_a_noop() {
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let lib = create_library(&app).await;

    // Must not panic or error on empty input.
    event_log::record_many(&db, Vec::new()).await;

    let count = EventEntity::find()
        .filter(EventCol::LibraryId.eq(lib))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

/// Insert a `library_events` row with an explicit `created_at` so the prune
/// test can age rows past the retention window.
async fn insert_event_at(db: &DatabaseConnection, lib: Uuid, age_days: i64) {
    let ts = chrono::Utc::now().fixed_offset() - chrono::Duration::days(age_days);
    EventAM {
        id: Set(Uuid::now_v7()),
        library_id: Set(lib),
        scan_run_id: Set(None),
        batch_id: Set(None),
        category: Set("issue".into()),
        entity_type: Set(None),
        entity_id: Set(None),
        entity_label: Set(None),
        action: Set("added".into()),
        severity: Set("info".into()),
        summary: Set(format!("aged {age_days}d")),
        detail: Set(None),
        created_at: Set(ts),
    }
    .insert(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn prune_drops_aged_and_over_cap_rows() {
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let lib = create_library(&app).await;

    // 3 recent rows + 2 well past the retention window.
    for _ in 0..3 {
        insert_event_at(&db, lib, 0).await;
    }
    insert_event_at(&db, lib, 200).await;
    insert_event_at(&db, lib, 365).await;

    // Time-based: 90-day window removes the two aged rows, generous cap keeps
    // the recent ones.
    let deleted = event_log::prune(&db, 90, 1000).await.unwrap();
    assert_eq!(deleted, 2);
    let remaining = EventEntity::find()
        .filter(EventCol::LibraryId.eq(lib))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(remaining, 3);

    // Per-library cap: keep only the most recent 1, regardless of age.
    let deleted = event_log::prune(&db, 90, 1).await.unwrap();
    assert_eq!(deleted, 2);
    let remaining = EventEntity::find()
        .filter(EventCol::LibraryId.eq(lib))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(remaining, 1);
}
