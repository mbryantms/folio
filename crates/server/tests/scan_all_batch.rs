//! Observability split M6 — "Scan all" batch grouping + finalize state machine.
//!
//! Drives the per-run finalize path (`scanner::finalize_run` →
//! `maybe_finalize_batch`) against a real DB to assert:
//!   - member runs carry `batch_id`
//!   - the batch stays `running` while any member is still queued
//!   - once every member is terminal, the batch rolls up to the derived
//!     state (`partial_failed` here: 2 complete + 1 pre-failed member)

mod common;

use common::TestApp;
use common::seed::LibrarySeed;
use entity::scan_batch::{ActiveModel as BatchAM, Entity as BatchEntity};
use entity::scan_run::{ActiveModel as ScanRunAM, Entity as ScanRunEntity};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use server::library::scanner;
use std::io::Write;
use std::path::Path;
use uuid::Uuid;

/// `validate_library` rejects an empty root, so give each library a single
/// series folder with one minimal CBZ to scan.
fn seed_one_issue(root: &Path, marker: u32) {
    let folder = root.join("Series (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    let f = std::fs::File::create(folder.join("Series 001.cbz")).unwrap();
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

async fn insert_run(
    db: &sea_orm::DatabaseConnection,
    id: Uuid,
    library_id: Uuid,
    batch_id: Uuid,
    state: &str,
) {
    ScanRunAM {
        id: Set(id),
        library_id: Set(library_id),
        state: Set(state.to_owned()),
        started_at: Set(chrono::Utc::now().fixed_offset()),
        ended_at: Set(None),
        stats: Set(serde_json::json!({})),
        error: Set(None),
        kind: Set("library".into()),
        series_id: Set(None),
        issue_id: Set(None),
        batch_id: Set(Some(batch_id)),
    }
    .insert(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn scan_all_batch_finalizes_to_partial_failed() {
    let app = TestApp::spawn().await;
    let state = app.state();
    let db = state.db.clone();

    // Two scannable (empty) libraries + one extra member that's pre-failed.
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    seed_one_issue(dir_a.path(), 1);
    seed_one_issue(dir_b.path(), 2);
    let lib_a = LibrarySeed::new(dir_a.path()).insert(&db).await;
    let lib_b = LibrarySeed::new(dir_b.path()).insert(&db).await;

    let batch_id = Uuid::now_v7();
    BatchAM {
        id: Set(batch_id),
        kind: Set("scan_all".into()),
        actor_id: Set(None),
        force: Set(false),
        started_at: Set(chrono::Utc::now().fixed_offset()),
        ended_at: Set(None),
        library_count: Set(3),
        state: Set("running".into()),
    }
    .insert(&db)
    .await
    .unwrap();

    // A member that already failed (e.g. validation error) — never scanned.
    insert_run(&db, Uuid::now_v7(), lib_a, batch_id, "failed").await;

    // Two queued members we actually scan. open_scan_run flips queued→running
    // and preserves batch_id.
    let run_a = Uuid::now_v7();
    let run_b = Uuid::now_v7();
    insert_run(&db, run_a, lib_a, batch_id, "queued").await;
    insert_run(&db, run_b, lib_b, batch_id, "queued").await;

    // Scan the first member. The batch must stay `running` — run_b is still
    // queued.
    scanner::scan_library_with_run_id(&state, lib_a, false, Some(run_a))
        .await
        .unwrap();
    let batch = BatchEntity::find_by_id(batch_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        batch.state, "running",
        "batch open while a member is queued"
    );
    assert!(batch.ended_at.is_none());

    // Scan the last member — now every member is terminal (2 complete + 1
    // failed) → partial_failed.
    scanner::scan_library_with_run_id(&state, lib_b, false, Some(run_b))
        .await
        .unwrap();
    let batch = BatchEntity::find_by_id(batch_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(batch.state, "partial_failed");
    assert!(batch.ended_at.is_some());

    // The scanned runs carried the batch link through to completion.
    let run = ScanRunEntity::find_by_id(run_a)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.batch_id, Some(batch_id));
    assert_eq!(run.state, "complete");
}
