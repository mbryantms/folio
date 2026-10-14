//! Post-audit T1.1 — fail-safe reconciliation. A folder the scan cannot
//! read (permission denied, dead mount) is *unknown*, not absent: nothing
//! at or under it is marked removed, an `UnreadableFolder` health row is
//! raised, and the row resolves on the next scan that reads the folder.
//!
//! Permission tests are skipped when the process can read a mode-000
//! directory anyway (root).

mod common;

use common::TestApp;
use common::seed::LibrarySeed;
use entity::{issue, library_health_issue, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use server::library::scanner;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use uuid::Uuid;

fn write_cbz(path: &Path, series_name: &str, number: u32, marker: u32) {
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
            r#"<?xml version="1.0"?><ComicInfo><Series>{series_name}</Series><Number>{number}</Number><Year>2012</Year></ComicInfo>"#
        )
        .as_bytes(),
    )
    .unwrap();
    zw.finish().unwrap();
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Visible marker for a test that could not exercise permissions (root).
#[allow(clippy::print_stderr)]
fn skip_note() {
    eprintln!("skipping: running as a user that can read mode-000 directories");
}

/// `true` when mode 000 actually blocks reads here (not root).
fn unreadable_takes_effect(path: &Path) -> bool {
    chmod(path, 0o000);
    let blocked = std::fs::read_dir(path).is_err();
    if !blocked {
        chmod(path, 0o755);
    }
    blocked
}

async fn active_issue_paths(app: &TestApp, lib_id: Uuid) -> Vec<String> {
    let mut v: Vec<String> = issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .filter(issue::Column::RemovedAt.is_null())
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.file_path)
        .collect();
    v.sort();
    v
}

async fn active_series_folders(app: &TestApp, lib_id: Uuid) -> Vec<String> {
    let mut v: Vec<String> = series::Entity::find()
        .filter(series::Column::LibraryId.eq(lib_id))
        .filter(series::Column::RemovedAt.is_null())
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|s| s.folder_path)
        .collect();
    v.sort();
    v
}

async fn unreadable_rows(app: &TestApp, lib_id: Uuid) -> Vec<library_health_issue::Model> {
    library_health_issue::Entity::find()
        .filter(library_health_issue::Column::LibraryId.eq(lib_id))
        .filter(library_health_issue::Column::Kind.eq("UnreadableFolder"))
        .all(&app.state().db)
        .await
        .unwrap()
}

fn row_path(row: &library_health_issue::Model) -> String {
    row.payload["data"]["path"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn unreadable_series_folder_keeps_its_issues_and_raises_then_resolves() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let alpha = root.join("Alpha");
    let beta = root.join("Beta");
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::create_dir_all(&beta).unwrap();
    write_cbz(&alpha.join("Alpha 001.cbz"), "Alpha", 1, 1);
    write_cbz(&alpha.join("Alpha 002.cbz"), "Alpha", 2, 2);
    write_cbz(&beta.join("Beta 001.cbz"), "Beta", 1, 3);
    write_cbz(&beta.join("Beta 002.cbz"), "Beta", 2, 4);
    let lib_id = LibrarySeed::new(root).insert(&app.state().db).await;

    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(active_issue_paths(&app, lib_id).await.len(), 4);
    assert_eq!(active_series_folders(&app, lib_id).await.len(), 2);

    if !unreadable_takes_effect(&beta) {
        skip_note();
        return;
    }

    // Beta is unreadable: nothing under it is removed, Alpha still works,
    // and a health row names the folder.
    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(stats.issues_removed, 0, "nothing may be removed: {stats:?}");
    assert_eq!(stats.series_removed, 0);
    assert_eq!(active_issue_paths(&app, lib_id).await.len(), 4);
    assert_eq!(active_series_folders(&app, lib_id).await.len(), 2);
    let rows = unreadable_rows(&app, lib_id).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(row_path(&rows[0]), beta.to_string_lossy());
    assert!(rows[0].resolved_at.is_none());
    assert_eq!(rows[0].severity, "error");
    // Never reported as "empty" either.
    let empties = library_health_issue::Entity::find()
        .filter(library_health_issue::Column::LibraryId.eq(lib_id))
        .filter(library_health_issue::Column::Kind.eq("EmptyFolder"))
        .filter(library_health_issue::Column::ResolvedAt.is_null())
        .all(&app.state().db)
        .await
        .unwrap();
    assert!(empties.is_empty(), "{empties:?}");

    // Readable again: the row resolves, still nothing removed.
    chmod(&beta, 0o755);
    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(stats.issues_removed, 0);
    assert_eq!(active_issue_paths(&app, lib_id).await.len(), 4);
    let rows = unreadable_rows(&app, lib_id).await;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].resolved_at.is_some(), "{rows:?}");

    // Control: a folder that is genuinely gone is still reconciled away.
    std::fs::remove_dir_all(&beta).unwrap();
    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(stats.issues_removed, 2, "{stats:?}");
    assert_eq!(active_series_folders(&app, lib_id).await.len(), 1);
}

#[tokio::test]
async fn unreadable_subfolder_inside_a_series_keeps_its_files() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let gamma = root.join("Gamma");
    let annuals = gamma.join("Annuals");
    std::fs::create_dir_all(&annuals).unwrap();
    write_cbz(&gamma.join("Gamma 001.cbz"), "Gamma", 1, 11);
    write_cbz(&gamma.join("Gamma 002.cbz"), "Gamma", 2, 12);
    write_cbz(&annuals.join("Gamma Annual 001.cbz"), "Gamma", 1, 13);
    let lib_id = LibrarySeed::new(root).insert(&app.state().db).await;

    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(active_issue_paths(&app, lib_id).await.len(), 3);

    if !unreadable_takes_effect(&annuals) {
        skip_note();
        return;
    }
    // Touch the main run so the folder is not fast-path skipped on mtime.
    write_cbz(&gamma.join("Gamma 003.cbz"), "Gamma", 3, 14);

    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(stats.issues_removed, 0, "{stats:?}");
    let active = active_issue_paths(&app, lib_id).await;
    assert_eq!(active.len(), 4, "{active:?}");
    assert!(
        active
            .iter()
            .any(|p| p.ends_with("Annuals/Gamma Annual 001.cbz")),
        "{active:?}"
    );
    let rows = unreadable_rows(&app, lib_id).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(row_path(&rows[0]), annuals.to_string_lossy());

    // Readable again and the annual truly deleted: now it is removed.
    chmod(&annuals, 0o755);
    std::fs::remove_file(annuals.join("Gamma Annual 001.cbz")).unwrap();
    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(stats.issues_removed, 1, "{stats:?}");
    assert!(unreadable_rows(&app, lib_id).await[0].resolved_at.is_some());
}

#[tokio::test]
async fn unreadable_publisher_container_keeps_every_series_under_it() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let publisher = root.join("Image");
    let saga = publisher.join("Saga");
    let paper = publisher.join("Paper Girls");
    std::fs::create_dir_all(&saga).unwrap();
    std::fs::create_dir_all(&paper).unwrap();
    write_cbz(&saga.join("Saga 001.cbz"), "Saga", 1, 21);
    write_cbz(&paper.join("Paper Girls 001.cbz"), "Paper Girls", 1, 22);
    let lib_id = LibrarySeed::new(root).insert(&app.state().db).await;

    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(active_series_folders(&app, lib_id).await.len(), 2);

    if !unreadable_takes_effect(&publisher) {
        skip_note();
        return;
    }
    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(stats.series_removed, 0, "{stats:?}");
    assert_eq!(stats.issues_removed, 0);
    assert_eq!(active_series_folders(&app, lib_id).await.len(), 2);
    let rows = unreadable_rows(&app, lib_id).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(row_path(&rows[0]), publisher.to_string_lossy());
    chmod(&publisher, 0o755);
}
