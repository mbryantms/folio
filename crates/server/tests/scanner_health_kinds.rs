//! WP-3.4 (audit R23) — the formerly-stub health kinds, end to end.
//!
//! Each test builds a small on-disk library with generated CBZ fixtures
//! (real PNG signatures — every reader content-sniffs page candidates),
//! runs a full library scan, and asserts on the persisted
//! `library_health_issues` rows:
//!
//! - `FolderNameMismatch` — folder name vs ComicInfo `<Series>`, with the
//!   scanner-style normalization keeping `Saga (2012)` / `Saga` quiet.
//! - `MixedSeriesInFolder` — one folder, several `<Series>`; specials
//!   ignored; resolves once the stray file is removed.
//! - `OrphanedSeriesJson` — `series.json` with no archives beside it.
//! - `AmbiguousFolder` — the row carries a bounded preview of the skipped
//!   subtree.

mod common;

use common::TestApp;
use common::seed::LibrarySeed;
use entity::library_health_issue::{self, Entity as HealthEntity};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use server::library::scanner;
use std::io::Write;
use std::path::Path;
use uuid::Uuid;

fn comic_info(series: &str, number: u32) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<ComicInfo><Series>{series}</Series><Number>{number}</Number><Year>2012</Year></ComicInfo>"#
    )
}

/// CBZ with one real-signature PNG page (made unique by `marker` so content
/// dedupe doesn't collapse fixtures) and an optional ComicInfo.xml.
fn write_cbz(path: &Path, comic_info: Option<&str>, marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    if let Some(xml) = comic_info {
        zw.start_file("ComicInfo.xml", opts).unwrap();
        zw.write_all(xml.as_bytes()).unwrap();
    }
    zw.finish().unwrap();
}

async fn rows_of(app: &TestApp, lib_id: Uuid, kind: &str) -> Vec<library_health_issue::Model> {
    HealthEntity::find()
        .filter(library_health_issue::Column::LibraryId.eq(lib_id))
        .filter(library_health_issue::Column::Kind.eq(kind))
        .all(&app.state().db)
        .await
        .unwrap()
}

fn data(row: &library_health_issue::Model) -> &serde_json::Value {
    row.payload.get("data").expect("adjacently-tagged payload")
}

fn open(rows: &[library_health_issue::Model]) -> Vec<&library_health_issue::Model> {
    rows.iter().filter(|r| r.resolved_at.is_none()).collect()
}

#[tokio::test]
async fn folder_name_mismatch_emitted_with_low_noise() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Spec §7.1 example: folder says Batman (2016), ComicInfo says
    // Batman: Rebirth → mismatch.
    let batman = root.join("Batman (2016)");
    std::fs::create_dir(&batman).unwrap();
    write_cbz(
        &batman.join("Batman 001.cbz"),
        Some(&comic_info("Batman: Rebirth", 1)),
        1,
    );
    write_cbz(
        &batman.join("Batman 002.cbz"),
        Some(&comic_info("Batman: Rebirth", 2)),
        2,
    );

    // Quiet: year group + article + volume token fold away.
    let saga = root.join("The Saga v1 (2012) [cv-4050-99999]");
    std::fs::create_dir(&saga).unwrap();
    write_cbz(&saga.join("Saga 001.cbz"), Some(&comic_info("Saga", 1)), 3);

    // Quiet: Layout B (nested by publisher) with an issue-folder name.
    let image = root.join("Image");
    let paper = image.join("Paper Girls 001");
    std::fs::create_dir_all(&paper).unwrap();
    write_cbz(
        &paper.join("Paper Girls 001.cbz"),
        Some(&comic_info("Paper Girls", 1)),
        4,
    );

    // Quiet: series.json confirms the ComicInfo name.
    let sidecar = root.join("Spawn Collection");
    std::fs::create_dir(&sidecar).unwrap();
    write_cbz(
        &sidecar.join("Spawn 001.cbz"),
        Some(&comic_info("Spawn", 1)),
        5,
    );
    std::fs::write(
        sidecar.join("series.json"),
        br#"{"metadata":{"type":"comicSeries","name":"Spawn","publisher":"Image","year_began":1992}}"#,
    )
    .unwrap();

    // Quiet: no ComicInfo at all — nothing to disagree with.
    let bare = root.join("Totally Different Name");
    std::fs::create_dir(&bare).unwrap();
    write_cbz(&bare.join("Something 001.cbz"), None, 6);

    let lib_id = LibrarySeed::new(root).insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib_id).await.unwrap();

    let rows = rows_of(&app, lib_id, "FolderNameMismatch").await;
    assert_eq!(
        rows.len(),
        1,
        "only the Batman folder should mismatch; got {:#?}",
        rows.iter().map(|r| &r.payload).collect::<Vec<_>>()
    );
    let d = data(&rows[0]);
    assert_eq!(rows[0].severity, "warning");
    assert_eq!(d["folder"], batman.to_string_lossy().as_ref());
    assert_eq!(d["comic_info_series"], "Batman: Rebirth");
    assert_eq!(d["files"], 2);
    assert!(
        d["series_id"]
            .as_str()
            .is_some_and(|s| s.parse::<Uuid>().is_ok()),
        "payload links the series"
    );

    // Unchanged rescan keeps the row open (folder fast path touches it).
    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    let rows = rows_of(&app, lib_id, "FolderNameMismatch").await;
    assert_eq!(open(&rows).len(), 1, "row survives an unchanged rescan");
}

#[tokio::test]
async fn mixed_series_in_folder_emitted_and_auto_resolves() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let saga = root.join("Saga (2012)");
    std::fs::create_dir(&saga).unwrap();
    write_cbz(&saga.join("Saga 001.cbz"), Some(&comic_info("Saga", 1)), 11);
    write_cbz(&saga.join("Saga 002.cbz"), Some(&comic_info("Saga", 2)), 12);
    let stray = saga.join("Paper Girls 001.cbz");
    write_cbz(&stray, Some(&comic_info("Paper Girls", 1)), 13);
    // A special in the allowlisted subfolder carries its own <Series>;
    // that's normal and must not count.
    let specials = saga.join("Specials");
    std::fs::create_dir(&specials).unwrap();
    write_cbz(
        &specials.join("Saga Deluxe Special.cbz"),
        Some(&comic_info("Saga Deluxe Special", 1)),
        14,
    );

    let lib_id = LibrarySeed::new(root).insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib_id).await.unwrap();

    let rows = rows_of(&app, lib_id, "MixedSeriesInFolder").await;
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let d = data(&rows[0]);
    assert_eq!(d["folder"], saga.to_string_lossy().as_ref());
    assert_eq!(d["distinct_values"], 2);
    let values = d["series_values"].as_array().unwrap();
    assert_eq!(values.len(), 2);
    assert_eq!(values[0]["series"], "Saga");
    assert_eq!(values[0]["files"], 2);
    assert_eq!(values[1]["series"], "Paper Girls");
    assert_eq!(values[1]["example"], "Paper Girls 001.cbz");
    // Folder name agrees with the dominant value → no mismatch row.
    assert!(rows_of(&app, lib_id, "FolderNameMismatch").await.is_empty());

    // Remove the stray: nothing is re-ingested, but the verdict changes
    // and the full scan's auto-resolve closes the row. The sleep keeps the
    // unlink's directory mtime strictly past last_scanned_at so the
    // folder fast path doesn't skip the folder.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::remove_file(&stray).unwrap();
    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    let rows = rows_of(&app, lib_id, "MixedSeriesInFolder").await;
    assert!(
        open(&rows).is_empty(),
        "fixing the folder auto-resolves the row: {rows:#?}"
    );
}

#[tokio::test]
async fn orphaned_series_json_replaces_empty_folder() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let live = root.join("Saga");
    std::fs::create_dir(&live).unwrap();
    write_cbz(&live.join("Saga 001.cbz"), Some(&comic_info("Saga", 1)), 21);

    let orphan = root.join("Moved Series (2019)");
    std::fs::create_dir(&orphan).unwrap();
    std::fs::write(
        orphan.join("series.json"),
        br#"{"metadata":{"type":"comicSeries","name":"Moved Series"}}"#,
    )
    .unwrap();

    let lib_id = LibrarySeed::new(root).insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib_id).await.unwrap();

    let rows = rows_of(&app, lib_id, "OrphanedSeriesJson").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].severity, "warning");
    assert_eq!(data(&rows[0])["folder"], orphan.to_string_lossy().as_ref());
    assert!(
        rows_of(&app, lib_id, "EmptyFolder").await.is_empty(),
        "an orphaned sidecar folder is not also an EmptyFolder"
    );
}

#[tokio::test]
async fn ambiguous_folder_lists_skipped_subtree() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // DC/Vertigo/{Sandman,Preacher}/*.cbz — third nesting level.
    let vertigo = root.join("DC").join("Vertigo");
    for (series, n, base) in [("Sandman", 25u32, 100u32), ("Preacher", 3, 200)] {
        let dir = vertigo.join(series);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..n {
            write_cbz(
                &dir.join(format!("{series} {i:03}.cbz")),
                Some(&comic_info(series, i)),
                base + i,
            );
        }
    }

    let lib_id = LibrarySeed::new(root).insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib_id).await.unwrap();

    let rows = rows_of(&app, lib_id, "AmbiguousFolder").await;
    assert_eq!(rows.len(), 1);
    let d = data(&rows[0]);
    assert_eq!(d["path"], vertigo.to_string_lossy().as_ref());
    assert_eq!(d["skipped_archive_count"], 28);
    let preview = d["skipped_archives"].as_array().unwrap();
    assert_eq!(
        preview.len(),
        scanner::enumerate::AMBIGUOUS_PREVIEW_LIMIT,
        "preview is bounded"
    );
    // Sorted relative paths: Preacher sorts first.
    assert_eq!(
        preview[0],
        Path::new("Preacher")
            .join("Preacher 000.cbz")
            .to_string_lossy()
            .as_ref()
    );
}
