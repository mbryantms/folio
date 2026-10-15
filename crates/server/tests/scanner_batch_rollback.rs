//! Post-audit T1.5 — a rolled-back ingest batch leaves the folder dirty.
//!
//! Before: a batch that failed mid-way rolled back its rows but the folder
//! was still stamped `last_scanned_at` (so the next plain scan mtime-skipped
//! it and the files were never retried), the completed event still counted
//! the rolled-back files as added, and the hash failure that caused it was
//! only a log line. Now the stamp is withheld, the counters and queued
//! events are reverted, the failing file gets an `UnreadableFile` health
//! row, and the folder is not treated as scanned by the reconcile.
//!
//! The failure is injected by making one archive unreadable (mode 000): the
//! full-file hash then errors in the prepare phase (post-audit T1.2), which
//! skips that file alone while the readable files persist — the folder still
//! stays dirty and the health row still names the file. Skipped when the
//! process can read mode-000 files (root).

mod common;

use common::TestApp;
use common::seed::LibrarySeed;
use entity::{issue, library_health_issue, series};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use server::library::scanner;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use uuid::Uuid;

fn write_cbz(path: &Path, number: u32, marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.start_file("ComicInfo.xml", opts).unwrap();
    zw.write_all(
        format!(
            r#"<?xml version="1.0"?><ComicInfo><Series>Batch</Series><Number>{number}</Number><Year>2012</Year></ComicInfo>"#
        )
        .as_bytes(),
    )
    .unwrap();
    zw.finish().unwrap();
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[allow(clippy::print_stderr)]
fn skip_note() {
    eprintln!("skipping: running as a user that can read mode-000 files");
}

async fn active_issues(app: &TestApp, lib_id: Uuid) -> usize {
    issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .filter(issue::Column::RemovedAt.is_null())
        .count(&app.state().db)
        .await
        .unwrap() as usize
}

async fn series_row(app: &TestApp, lib_id: Uuid) -> Option<series::Model> {
    series::Entity::find()
        .filter(series::Column::LibraryId.eq(lib_id))
        .one(&app.state().db)
        .await
        .unwrap()
}

#[tokio::test]
async fn rolled_back_batch_keeps_the_folder_dirty_and_reports_the_failure() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let folder = root.join("Batch");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Batch 001.cbz"), 1, 1);
    let bad = folder.join("Batch 002.cbz");
    write_cbz(&bad, 2, 2);
    write_cbz(&folder.join("Batch 003.cbz"), 3, 3);
    let lib_id = LibrarySeed::new(root).insert(&app.state().db).await;

    chmod(&bad, 0o000);
    if std::fs::File::open(&bad).is_ok() {
        skip_note();
        return;
    }

    // Scan 1: the unreadable file fails its hash in the prepare phase
    // (post-audit T1.2), before any transaction opens, so it alone is
    // skipped; the two readable files persist. The folder still stays dirty.
    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(
        stats.files_added, 2,
        "only the unreadable file is skipped: {stats:?}"
    );
    assert_eq!(stats.files_seen, 3, "{stats:?}");
    assert_eq!(active_issues(&app, lib_id).await, 2);
    let srow = series_row(&app, lib_id)
        .await
        .expect("series row from folder");
    assert!(
        srow.last_scanned_at.is_none(),
        "a failed batch must not stamp last_scanned_at: {srow:?}"
    );
    let rows: Vec<library_health_issue::Model> = library_health_issue::Entity::find()
        .filter(library_health_issue::Column::LibraryId.eq(lib_id))
        .filter(library_health_issue::Column::Kind.eq("UnreadableFile"))
        .filter(library_health_issue::Column::ResolvedAt.is_null())
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        rows[0].payload["data"]["path"],
        bad.to_string_lossy().as_ref()
    );
    assert!(
        rows[0].payload["data"]["error"]
            .as_str()
            .unwrap()
            .contains("could not read the file"),
        "{rows:?}"
    );

    // Scan 2 (plain, nothing touched on disk): the folder is re-walked, not
    // mtime-skipped; the two good files are unchanged, the bad one fails again.
    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(stats.series_skipped_unchanged, 0, "{stats:?}");
    assert_eq!(stats.files_seen, 3, "{stats:?}");
    assert_eq!(stats.files_added, 0);
    assert_eq!(stats.files_unchanged, 2, "{stats:?}");

    // Fixed: the last file ingests, the health row resolves, the stamp lands.
    chmod(&bad, 0o644);
    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(stats.files_added, 1, "{stats:?}");
    assert_eq!(active_issues(&app, lib_id).await, 3);
    assert!(
        series_row(&app, lib_id)
            .await
            .unwrap()
            .last_scanned_at
            .is_some()
    );
    let open = library_health_issue::Entity::find()
        .filter(library_health_issue::Column::LibraryId.eq(lib_id))
        .filter(library_health_issue::Column::Kind.eq("UnreadableFile"))
        .filter(library_health_issue::Column::ResolvedAt.is_null())
        .count(&app.state().db)
        .await
        .unwrap();
    assert_eq!(open, 0);
}

#[tokio::test]
async fn rolled_back_batch_never_removes_rows_the_scan_did_not_see() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let folder = root.join("Batch");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Batch 001.cbz"), 1, 11);
    write_cbz(&folder.join("Batch 002.cbz"), 2, 12);
    let lib_id = LibrarySeed::new(root).insert(&app.state().db).await;
    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(stats.files_added, 2);

    // A new file that cannot be hashed arrives; the batch that would have
    // ingested it rolls back. The two committed rows must stay active —
    // the folder was processed but not completed, so nothing is reconciled.
    let bad = folder.join("Batch 003.cbz");
    write_cbz(&bad, 3, 13);
    chmod(&bad, 0o000);
    if std::fs::File::open(&bad).is_ok() {
        skip_note();
        return;
    }
    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(stats.files_added, 0, "{stats:?}");
    assert_eq!(stats.issues_removed, 0, "{stats:?}");
    assert_eq!(active_issues(&app, lib_id).await, 2);
    chmod(&bad, 0o644);
}
