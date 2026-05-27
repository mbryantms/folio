//! Layout B (nested-by-publisher) end-to-end smoke.
//!
//! Sister test to [`scanner_smoke`]. Exercises the full scan pipeline
//! against a `root/Publisher/Series/CBZ` tree and asserts that
//!
//!   1. The walker classifies depth-1 folders as publisher containers.
//!   2. The series rows root at depth-2 (the series folder), not at
//!      the publisher container.
//!   3. `series.publisher` is auto-promoted from the publisher folder
//!      name when ComicInfo and `series.json` are silent.
//!   4. Mixed roots (a flat series next to a publisher container) work
//!      transparently.
//!
//! See `~/.claude/plans/scanner-nested-folders-1.0.md` M4.

mod common;

use common::TestApp;
use entity::{
    issue::Entity as IssueEntity, library::ActiveModel as LibraryAM, series::Entity as SeriesEntity,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
use server::library::scanner;
use std::io::Write;
use std::path::Path;
use uuid::Uuid;

fn write_minimal_cbz(path: &Path, comic_info: Option<&str>, unique_marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
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
        name: Set("Nested Smoke".into()),
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
        allow_archive_writeback: Set(false),
        metadata_writeback_enabled: Set(false),
        archive_backup_retain_count: Set(1),
        archive_backup_retain_days: Set(30),
        metadata_publisher_blacklist: Set(serde_json::json!([])),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

/// The canonical nested layout: `root/Publisher/Series/CBZ`. Two
/// publishers, two series each, with no ComicInfo so the publisher
/// must come from the path.
#[tokio::test]
async fn nested_publisher_layout_creates_series_at_depth_two() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let publisher_a = tmp.path().join("Publisher A");
    let series_aa = publisher_a.join("Series AA");
    let series_ab = publisher_a.join("Series AB");
    std::fs::create_dir_all(&series_aa).unwrap();
    std::fs::create_dir_all(&series_ab).unwrap();
    write_minimal_cbz(&series_aa.join("Series AA - v01.cbz"), None, 401);
    write_minimal_cbz(&series_aa.join("Series AA - v02.cbz"), None, 402);
    write_minimal_cbz(&series_ab.join("Oneshot.cbz"), None, 403);

    let publisher_b = tmp.path().join("Publisher B");
    let series_ba = publisher_b.join("Series BA");
    std::fs::create_dir_all(&series_ba).unwrap();
    write_minimal_cbz(&series_ba.join("Series BA - v01.cbz"), None, 404);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    let stats = scanner::scan_library(&state, lib_id).await.unwrap();

    // 3 series at depth-2; 4 archives.
    assert_eq!(stats.series_created, 3);
    assert_eq!(stats.files_added, 4);

    let rows = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);

    // Each series's folder_path is the depth-2 path, never the
    // publisher container.
    for row in &rows {
        let fp = row
            .folder_path
            .as_deref()
            .expect("series should have folder_path");
        assert!(
            !fp.ends_with("Publisher A") && !fp.ends_with("Publisher B"),
            "folder_path must point at the series folder, not the publisher container: {fp}",
        );
    }

    // Publisher promotion: every series picked up the parent folder
    // name (because no ComicInfo + no series.json provided one). We
    // key by folder_path rather than series.name because filename
    // inference can shape the name however it likes — what we care
    // about is that the row anchored at `…/Publisher A/Series AA`
    // got `publisher = "Publisher A"`.
    let by_folder: std::collections::HashMap<String, Option<String>> = rows
        .iter()
        .map(|r| {
            (
                r.folder_path.clone().unwrap_or_default(),
                r.publisher.clone(),
            )
        })
        .collect();
    let aa = series_aa.to_string_lossy().into_owned();
    let ab = series_ab.to_string_lossy().into_owned();
    let ba = series_ba.to_string_lossy().into_owned();
    assert_eq!(by_folder.get(&aa), Some(&Some("Publisher A".into())));
    assert_eq!(by_folder.get(&ab), Some(&Some("Publisher A".into())));
    assert_eq!(by_folder.get(&ba), Some(&Some("Publisher B".into())));
}

/// Mixed root: a flat series at the root next to a publisher container.
/// Each top-level folder classifies independently.
#[tokio::test]
async fn mixed_layout_root_classifies_per_child() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    // Flat: tmp/Watchmen/Watchmen.cbz
    let watchmen = tmp.path().join("Watchmen");
    std::fs::create_dir_all(&watchmen).unwrap();
    write_minimal_cbz(&watchmen.join("Watchmen.cbz"), None, 410);

    // Nested: tmp/Publisher Z/Series Z/Series Z 001.cbz
    let publisher_z = tmp.path().join("Publisher Z");
    let series_z = publisher_z.join("Series Z");
    std::fs::create_dir_all(&series_z).unwrap();
    write_minimal_cbz(&series_z.join("Series Z 001.cbz"), None, 411);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    let stats = scanner::scan_library(&state, lib_id).await.unwrap();

    assert_eq!(stats.series_created, 2);
    assert_eq!(stats.files_added, 2);

    let rows = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    let by_folder: std::collections::HashMap<String, Option<String>> = rows
        .iter()
        .map(|r| {
            (
                r.folder_path.clone().unwrap_or_default(),
                r.publisher.clone(),
            )
        })
        .collect();
    // Flat: no publisher promotion (parent IS the library root).
    assert_eq!(
        by_folder.get(&watchmen.to_string_lossy().into_owned()),
        Some(&None),
    );
    // Nested: publisher promoted from parent folder.
    assert_eq!(
        by_folder.get(&series_z.to_string_lossy().into_owned()),
        Some(&Some("Publisher Z".into())),
    );
}

/// Nested-with-Specials: a Layout B series that ALSO has a Specials
/// subfolder. Confirms that M2.5's subfolder-derived special_type and
/// M3's publisher promotion compose cleanly.
#[tokio::test]
async fn nested_layout_with_specials_subfolder() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let publisher = tmp.path().join("Publisher Y");
    let series = publisher.join("Series Y");
    let specials = series.join("Specials");
    std::fs::create_dir_all(&specials).unwrap();
    // Use an unambiguous issue number ("001") so filename inference
    // doesn't classify the main-run file as a OneShot via the
    // no-recognizable-number rule — that's tangential to what this
    // test asserts.
    write_minimal_cbz(&series.join("Series Y 001.cbz"), None, 420);
    write_minimal_cbz(&specials.join("Artbook 1.cbz"), None, 421);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let series_row = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .expect("series row");
    assert_eq!(series_row.publisher.as_deref(), Some("Publisher Y"));

    let issues = IssueEntity::find()
        .filter(entity::issue::Column::SeriesId.eq(series_row.id))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(issues.len(), 2);
    let by_path: std::collections::HashMap<String, Option<String>> = issues
        .into_iter()
        .map(|i| (i.file_path, i.special_type))
        .collect();
    let st = |needle: &str| -> Option<String> {
        by_path
            .iter()
            .find(|(p, _)| p.contains(needle))
            .and_then(|(_, st)| st.clone())
    };
    assert_eq!(st("Series Y 001"), None);
    assert_eq!(st("Artbook 1"), Some("Special".into()));
}

/// Idempotency: a second scan with no on-disk changes must not
/// re-create series or duplicate issues. Mirrors the flat-layout
/// idempotency test in `scanner_smoke.rs` but for Layout B.
#[tokio::test]
async fn nested_layout_rescan_is_idempotent() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let publisher = tmp.path().join("Publisher X");
    let series = publisher.join("Series X");
    std::fs::create_dir_all(&series).unwrap();
    write_minimal_cbz(&series.join("Series X 001.cbz"), None, 430);
    write_minimal_cbz(&series.join("Series X 002.cbz"), None, 431);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    let first = scanner::scan_library(&state, lib_id).await.unwrap();
    let second = scanner::scan_library(&state, lib_id).await.unwrap();

    assert_eq!(first.series_created, 1);
    assert_eq!(first.files_added, 2);
    assert_eq!(second.series_created, 0);
    assert_eq!(second.files_added, 0);

    let series_count = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .count(&state.db)
        .await
        .unwrap();
    let issue_count = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .count(&state.db)
        .await
        .unwrap();
    assert_eq!(series_count, 1);
    assert_eq!(issue_count, 2);
}
