//! Library Scanner v1 — Milestone 8 file-processing pipeline (spec §6).
//!
//! Validates the in-scope additions:
//!   - specials/annuals/one-shot detection (`special_type` column populated)
//!   - ComicInfo PageCount is stored as metadata, not treated as health truth
//!   - MetronInfo.xml beats ComicInfo.xml on overlapping fields (§4.4)
//!   - series.json populates series-level metadata when ComicInfo is silent
//!
//! Documented deferrals from M8 (carry-over to a follow-up plan):
//!   - volume year-vs-sequence column split (§6.4)
//!   - hash-mismatch `superseded_by` linkage (§6.2)
//!   - dedupe-by-content `issue_paths` alias table (§6, §10.1 DuplicateContent)

mod common;

use common::TestApp;
use entity::{
    issue::Entity as IssueEntity, library::ActiveModel as LibraryAM,
    library_health_issue::Entity as HealthEntity, series::Entity as SeriesEntity,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::library::scanner;
use std::io::Write;
use std::path::Path;
use uuid::Uuid;

fn write_cbz_with_xml(
    path: &Path,
    marker: u32,
    pages: usize,
    comic_info_xml: Option<&str>,
    metron_info_xml: Option<&str>,
) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    for i in 0..pages {
        let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&marker.to_le_bytes());
        png.extend_from_slice(&(i as u32).to_le_bytes());
        png.extend(std::iter::repeat_n(0u8, 32));
        zw.start_file(format!("page-{i:03}.png"), opts).unwrap();
        zw.write_all(&png).unwrap();
    }

    if let Some(xml) = comic_info_xml {
        zw.start_file("ComicInfo.xml", opts).unwrap();
        zw.write_all(xml.as_bytes()).unwrap();
    }
    if let Some(xml) = metron_info_xml {
        zw.start_file("MetronInfo.xml", opts).unwrap();
        zw.write_all(xml.as_bytes()).unwrap();
    }
    zw.finish().unwrap();
}

async fn create_library(app: &TestApp, root: &Path) -> Uuid {
    create_library_with_missing_report(app, root, false).await
}

async fn create_library_with_missing_report(
    app: &TestApp,
    root: &Path,
    report_missing_comicinfo: bool,
) -> Uuid {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = chrono::Utc::now().fixed_offset();
    LibraryAM {
        id: Set(id),
        name: Set("M8 Lib".into()),
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
        report_missing_comicinfo: Set(report_missing_comicinfo),
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

#[tokio::test]
async fn special_type_detection_classifies_files() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Pi (2025)");
    std::fs::create_dir_all(&folder).unwrap();

    // Standard issue with a number → no special_type.
    write_cbz_with_xml(&folder.join("Pi 001.cbz"), 1, 2, None, None);
    // Annual via filename token.
    write_cbz_with_xml(&folder.join("Pi Annual 2025.cbz"), 2, 2, None, None);
    // Special via ComicInfo Format.
    write_cbz_with_xml(
        &folder.join("Pi Bonus.cbz"),
        3,
        2,
        Some(
            r#"<?xml version="1.0"?><ComicInfo><Series>Pi</Series><Format>Special</Format></ComicInfo>"#,
        ),
        None,
    );
    // One-shot via no-recognizable-number (filename has no #).
    write_cbz_with_xml(&folder.join("Pi Origin Story.cbz"), 4, 2, None, None);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let issues = IssueEntity::find().all(&state.db).await.unwrap();
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
    assert_eq!(
        st("Pi 001"),
        None,
        "regular issue is not special: {by_path:?}"
    );
    assert_eq!(st("Pi Annual"), Some("Annual".into()));
    assert_eq!(st("Pi Bonus"), Some("Special".into()));
    assert_eq!(st("Pi Origin Story"), Some("OneShot".into()));
}

#[tokio::test]
async fn page_count_reflects_archive_not_comicinfo() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Rho (2025)");
    std::fs::create_dir_all(&folder).unwrap();

    // ComicInfo declares 5 pages but the CBZ only has 2. The strip
    // thumbnail worker can only encode pages that actually exist in the
    // archive, so trusting `<PageCount>` would leave the readiness
    // denominator chasing pages that aren't there. The scanner now
    // stores the archive's image-entry count instead.
    let comic_info = r#"<?xml version="1.0"?>
        <ComicInfo>
            <Series>Rho</Series>
            <Number>1</Number>
            <PageCount>5</PageCount>
        </ComicInfo>"#;
    write_cbz_with_xml(&folder.join("Rho 001.cbz"), 1, 2, Some(comic_info), None);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let issue = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .expect("issue row");
    assert_eq!(issue.page_count, Some(2));

    let health = HealthEntity::find().all(&state.db).await.unwrap();
    assert!(
        health.iter().all(|i| i.kind != "PageCountMismatch"),
        "PageCountMismatch is retired and should not be emitted: {health:?}",
    );
}

/// Regression: rescanning a library where nothing on disk has changed must
/// preserve open archive-derived health issues. Before the touch-on-skip fix
/// the scanner's per-file fast-path would let the auto-resolve sweep close
/// archive health rows just because they weren't re-emitted.
#[tokio::test]
async fn rescan_preserves_unchanged_file_health_issues() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Sigma (2025)");
    std::fs::create_dir_all(&folder).unwrap();

    let archive = folder.join("Sigma 001.cbz");
    write_cbz_with_xml(&archive, 1, 2, None, None);

    let lib_id = create_library_with_missing_report(&app, tmp.path(), true).await;
    let state = app.state();

    // Initial scan: emits the missing ComicInfo health issue.
    scanner::scan_library(&state, lib_id).await.unwrap();
    let after_first: Vec<_> = HealthEntity::find()
        .filter(entity::library_health_issue::Column::Kind.eq("MissingComicInfo"))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(after_first.len(), 1);
    assert!(
        after_first[0].resolved_at.is_none(),
        "should be open after first scan"
    );
    let first_id = after_first[0].id;

    // Rescan without touching anything on disk. Both fast-paths trigger:
    // the folder mtime is unchanged so `process_folder` short-circuits;
    // even if it didn't, the file's size+mtime are unchanged so
    // `process_file` would short-circuit too.
    scanner::scan_library(&state, lib_id).await.unwrap();
    let after_second: Vec<_> = HealthEntity::find()
        .filter(entity::library_health_issue::Column::Kind.eq("MissingComicInfo"))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(after_second.len(), 1, "no duplicate row should appear");
    assert_eq!(after_second[0].id, first_id, "same row, not a new one");
    assert!(
        after_second[0].resolved_at.is_none(),
        "warning must stay open across rescans of an unchanged file",
    );
    assert!(
        after_second[0].last_seen_at >= after_first[0].last_seen_at,
        "last_seen_at should be bumped to reflect the rescan",
    );
}

/// Same protection at the file granularity: when only one file in a folder
/// is unchanged but the folder itself is dirty (so `process_folder` doesn't
/// skip), the per-file fast-path must still touch the existing health row.
#[tokio::test]
async fn unchanged_file_inside_dirty_folder_preserves_health_issue() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Tau (2025)");
    std::fs::create_dir_all(&folder).unwrap();

    let missing = folder.join("Tau 001.cbz");
    write_cbz_with_xml(&missing, 1, 2, None, None);

    let lib_id = create_library_with_missing_report(&app, tmp.path(), true).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let after_first: Vec<_> = HealthEntity::find()
        .filter(entity::library_health_issue::Column::Kind.eq("MissingComicInfo"))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(after_first.len(), 1);
    let first_id = after_first[0].id;

    // Add a second file to the same folder. The folder's max mtime advances
    // so the folder-level fast-path no longer skips, but the original
    // missing-ComicInfo file is byte-for-byte unchanged so its per-file fast-path
    // does skip. The health issue for it must still survive.
    let companion = folder.join("Tau 002.cbz");
    let companion_info = r#"<?xml version="1.0"?>
        <ComicInfo>
            <Series>Tau</Series>
            <Number>2</Number>
        </ComicInfo>"#;
    write_cbz_with_xml(&companion, 2, 3, Some(companion_info), None);

    scanner::scan_library(&state, lib_id).await.unwrap();
    let after_second: Vec<_> = HealthEntity::find()
        .filter(entity::library_health_issue::Column::Kind.eq("MissingComicInfo"))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(after_second.len(), 1);
    assert_eq!(after_second[0].id, first_id);
    assert!(
        after_second[0].resolved_at.is_none(),
        "per-file fast-path should preserve the warning",
    );
}

/// And the negative case: when missing ComicInfo is fixed, the issue must
/// auto-resolve.
#[tokio::test]
async fn rewriting_missing_comicinfo_archive_resolves_health_issue() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Upsilon (2025)");
    std::fs::create_dir_all(&folder).unwrap();

    let archive = folder.join("Upsilon 001.cbz");
    write_cbz_with_xml(&archive, 1, 2, None, None);

    let lib_id = create_library_with_missing_report(&app, tmp.path(), true).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let after_first: Vec<_> = HealthEntity::find()
        .filter(entity::library_health_issue::Column::Kind.eq("MissingComicInfo"))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(after_first.len(), 1);
    assert!(after_first[0].resolved_at.is_none());

    // Rewrite the archive with ComicInfo. Since we replace the file its
    // mtime advances, so neither fast-path can skip.
    std::thread::sleep(std::time::Duration::from_millis(10));
    let good = r#"<?xml version="1.0"?>
        <ComicInfo>
            <Series>Upsilon</Series>
            <Number>1</Number>
        </ComicInfo>"#;
    write_cbz_with_xml(&archive, 1, 2, Some(good), None);
    // Force a real mtime delta that's portable across filesystems.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
    let _ = filetime::set_file_mtime(&archive, filetime::FileTime::from_system_time(later));

    scanner::scan_library(&state, lib_id).await.unwrap();
    let after_second: Vec<_> = HealthEntity::find()
        .filter(entity::library_health_issue::Column::Kind.eq("MissingComicInfo"))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(
        after_second.len(),
        1,
        "row stays around but is now resolved"
    );
    assert!(
        after_second[0].resolved_at.is_some(),
        "fixed missing ComicInfo should auto-resolve, got {:?}",
        after_second[0],
    );
}

#[tokio::test]
async fn metroninfo_overrides_comicinfo() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Sigma (2024)");
    std::fs::create_dir_all(&folder).unwrap();

    // ComicInfo says one Series + Title; MetronInfo says different + adds writer credit.
    let comic_info = r#"<?xml version="1.0"?>
        <ComicInfo>
            <Series>Sigma OLD</Series>
            <Number>1</Number>
            <Title>Old Title</Title>
            <Writer>Old Writer</Writer>
        </ComicInfo>"#;
    let metron_info = r#"<?xml version="1.0"?>
        <MetronInfo>
            <Series>Sigma</Series>
            <Title>New Title</Title>
            <Credits>
                <Credit role="Writer"><Creator><Name>Brand New Writer</Name></Creator></Credit>
            </Credits>
        </MetronInfo>"#;
    write_cbz_with_xml(
        &folder.join("Sigma 001.cbz"),
        1,
        2,
        Some(comic_info),
        Some(metron_info),
    );

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let issue = IssueEntity::find().one(&state.db).await.unwrap().unwrap();
    assert_eq!(
        issue.title.as_deref(),
        Some("New Title"),
        "MetronInfo title wins"
    );
    assert_eq!(
        issue.writer.as_deref(),
        Some("Brand New Writer"),
        "MetronInfo Writer credit wins",
    );
}

#[tokio::test]
async fn series_json_fills_series_metadata() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Tau"); // no (year) in folder name
    std::fs::create_dir_all(&folder).unwrap();

    // ComicInfo deliberately omits publisher/year — series.json provides them.
    write_cbz_with_xml(
        &folder.join("Tau 001.cbz"),
        1,
        2,
        Some(
            r#"<?xml version="1.0"?><ComicInfo><Series>Tau</Series><Number>1</Number></ComicInfo>"#,
        ),
        None,
    );
    let series_json = r#"{
        "metadata": {
            "name": "Tau",
            "publisher": "Tau Press",
            "year_began": 2019,
            "total_issues": 12,
            "age_rating": "All Ages"
        }
    }"#;
    std::fs::write(folder.join("series.json"), series_json).unwrap();

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let series = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(series.publisher.as_deref(), Some("Tau Press"));
    assert_eq!(series.year, Some(2019));
    assert_eq!(series.total_issues, Some(12));
    assert_eq!(series.age_rating.as_deref(), Some("All Ages"));
}

#[tokio::test]
async fn series_json_status_and_summary_apply_on_first_scan() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Omega (2013)");
    std::fs::create_dir_all(&folder).unwrap();

    write_cbz_with_xml(
        &folder.join("Omega 001.cbz"),
        1,
        2,
        Some(
            r#"<?xml version="1.0"?><ComicInfo><Series>Omega</Series><Number>1</Number></ComicInfo>"#,
        ),
        None,
    );
    let series_json = r#"{
        "metadata": {
            "name": "Omega",
            "publisher": "Image",
            "year_began": 2013,
            "total_issues": 43,
            "status": "Ended",
            "description_text": "Forty-three issue science fiction title.",
            "comicid": 69537
        }
    }"#;
    std::fs::write(folder.join("series.json"), series_json).unwrap();

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let series = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        series.status, "ended",
        "first scan must apply series.json status"
    );
    assert_eq!(
        series.summary.as_deref(),
        Some("Forty-three issue science fiction title."),
        "first scan must apply series.json description_text"
    );
    assert_eq!(series.total_issues, Some(43));
    assert_eq!(series.comicvine_id, Some(69537));
}

#[tokio::test]
async fn series_json_added_after_initial_scan_takes_effect_on_rescan() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Sigma (2014)");
    std::fs::create_dir_all(&folder).unwrap();

    // Scan #1: no series.json present — series row gets default
    // status="continuing" / summary=NULL.
    write_cbz_with_xml(
        &folder.join("Sigma 001.cbz"),
        1,
        2,
        Some(
            r#"<?xml version="1.0"?><ComicInfo><Series>Sigma</Series><Number>1</Number></ComicInfo>"#,
        ),
        None,
    );
    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let after_first = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_first.status, "continuing");
    assert!(after_first.summary.is_none());

    // Drop series.json into the folder and scan again.
    let series_json = r#"{
        "metadata": {
            "name": "Sigma",
            "status": "Ended",
            "total_issues": 22,
            "description_text": "A finished mini-series."
        }
    }"#;
    std::fs::write(folder.join("series.json"), series_json).unwrap();

    scanner::scan_library(&state, lib_id).await.unwrap();

    let after_second = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after_second.status, "ended",
        "rescan must apply sidecar status"
    );
    assert_eq!(
        after_second.summary.as_deref(),
        Some("A finished mini-series."),
        "rescan must apply sidecar description"
    );
    assert_eq!(after_second.total_issues, Some(22));
}

#[tokio::test]
async fn series_json_takes_effect_even_when_folder_mtime_unchanged() {
    // Models the upgrade scenario: existing series rows already have
    // status="continuing" / summary=NULL because the prior binary didn't
    // know about series.json. The user adds a series.json sidecar but
    // backdates its mtime (or copies it preserving timestamps from a
    // template). On the next scan, the folder mtime fast-path would
    // mark the folder skipped_unchanged. The scanner must STILL run
    // reconcile_series_status against the sidecar so the row gets fixed.
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Theta (2018)");
    std::fs::create_dir_all(&folder).unwrap();

    write_cbz_with_xml(
        &folder.join("Theta 001.cbz"),
        1,
        2,
        Some(
            r#"<?xml version="1.0"?><ComicInfo><Series>Theta</Series><Number>1</Number></ComicInfo>"#,
        ),
        None,
    );
    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let after_first = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_first.status, "continuing");
    assert!(after_first.summary.is_none());
    let last_scanned = after_first
        .last_scanned_at
        .expect("series.last_scanned_at stamped after first scan");

    // Drop a series.json into the folder, then back-date both the
    // sidecar and the existing CBZ to a time before the recorded
    // last_scanned_at. This simulates a user who added the sidecar with
    // `cp -p` (preserving an older source mtime) — or, more commonly,
    // an upgrade where the binary now knows about new sidecar fields
    // but the user hasn't touched the folder since the previous scan.
    let series_json = r#"{
        "metadata": {
            "name": "Theta",
            "status": "Ended",
            "total_issues": 7,
            "description_text": "A short ended run."
        }
    }"#;
    std::fs::write(folder.join("series.json"), series_json).unwrap();
    let backdate = filetime::FileTime::from_system_time(
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000),
    );
    filetime::set_file_mtime(folder.join("series.json"), backdate).unwrap();
    filetime::set_file_mtime(folder.join("Theta 001.cbz"), backdate).unwrap();
    filetime::set_file_mtime(&folder, backdate).unwrap();
    // Sanity: the folder's recursive max mtime is now older than the
    // series row's last_scanned_at, so the scanner's folder fast-path
    // will mark this folder skipped_unchanged.
    assert!(
        backdate.unix_seconds() < last_scanned.timestamp(),
        "backdated mtime must be before last_scanned_at to exercise the fast-path"
    );

    scanner::scan_library(&state, lib_id).await.unwrap();

    let after_second = SeriesEntity::find()
        .filter(entity::series::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after_second.status, "ended",
        "rescan must apply sidecar status even when folder mtime was unchanged"
    );
    assert_eq!(
        after_second.summary.as_deref(),
        Some("A short ended run."),
        "rescan must apply sidecar description even when folder mtime was unchanged"
    );
    assert_eq!(after_second.total_issues, Some(7));
}
