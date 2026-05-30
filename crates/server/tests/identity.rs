//! Library Scanner v1 — Milestone 6 series identity & match_key override.
//!
//! Validates the focused MVP:
//!   - the second scan of the same library reuses the existing series via the
//!     `folder_path` fast path (no second `series_created`)
//!   - `PATCH /series/{id}` accepts `match_key` and the value persists
//!   - a folder rename keeps the same series_id (resolution falls through to
//!     `normalized_name + year` and backfills `folder_path`)
//!   - moving an issue file between folders preserves its issue id

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use entity::{
    library::ActiveModel as LibraryAM,
    series::{Column as SeriesCol, Entity as SeriesEntity},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::library::scanner;
use std::io::Write;
use std::path::Path;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
}

async fn register_admin(app: &TestApp) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"id@example.com","password":"correctly-horse-battery"}"#,
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
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

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

/// Same as `write_cbz` but stamps a caller-supplied ComicInfo.xml
/// alongside the page. Used by volume / metadata regression tests that
/// need to exercise specific tag values (e.g. `<Volume>2016</Volume>`).
fn write_cbz_with_comicinfo(path: &Path, comicinfo_xml: &str) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.start_file("ComicInfo.xml", opts).unwrap();
    zw.write_all(comicinfo_xml.as_bytes()).unwrap();
    zw.finish().unwrap();
}

async fn create_library(app: &TestApp, root: &Path) -> Uuid {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = chrono::Utc::now().fixed_offset();
    LibraryAM {
        id: Set(id),
        name: Set("Identity Lib".into()),
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
async fn second_scan_reuses_series_via_folder_path() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Iota (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Iota 001.cbz"), 1);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    let s1 = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(
        s1.series_created, 1,
        "first scan creates the series: {s1:?}"
    );

    // Touch the file so the mtime gate doesn't short-circuit the second walk.
    let _ = std::fs::File::options()
        .write(true)
        .open(folder.join("Iota 001.cbz"))
        .unwrap()
        .write_all(&[])
        .ok();
    let new_time = filetime::FileTime::from_system_time(std::time::SystemTime::now());
    filetime::set_file_mtime(folder.join("Iota 001.cbz"), new_time).unwrap();

    let s2 = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(s2.series_created, 0, "second scan reuses series: {s2:?}");

    // Only one series row in the DB.
    let series = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(series.len(), 1);
}

#[tokio::test]
async fn folder_rename_keeps_same_series_row() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Kappa (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Kappa 001.cbz"), 1);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    let s1 = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(s1.series_created, 1);
    let series_before = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();

    // Rename the folder. ComicInfo is absent, so identity falls back to the
    // filename-inferred Series name "Kappa". The series row stays. Its
    // folder_path is backfilled to the new path.
    let renamed = tmp.path().join("Series Kappa Vol 1 (2025)");
    std::fs::rename(&folder, &renamed).unwrap();
    // Touch the renamed file so per-folder mtime gate fires for both old and new
    // (the rename usually updates parent mtime; force the file mtime too).
    let new_time = filetime::FileTime::from_system_time(std::time::SystemTime::now());
    filetime::set_file_mtime(renamed.join("Kappa 001.cbz"), new_time).unwrap();

    let s2 = scanner::scan_library(&state, lib_id).await.unwrap();
    // Note: with no ComicInfo and a different folder name, filename inference
    // picks up "Kappa" as the series name (same as before) — so identity falls
    // through normalized_name+year to the existing row, no new series.
    assert_eq!(
        s2.series_created, 0,
        "rename should not create a new series: {s2:?}"
    );

    let all_series = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(all_series.len(), 1, "still one series row");
    assert_eq!(all_series[0].id, series_before.id);
    assert_eq!(
        all_series[0].folder_path.as_deref(),
        Some(renamed.to_string_lossy().as_ref()),
        "folder_path is backfilled to the new location",
    );
}

#[tokio::test]
async fn two_volumes_of_same_series_get_distinct_rows_and_dont_cycle() {
    // Regression for the "folder-collapse" bug (dev DB 2026-05-14): two
    // sibling on-disk folders for different volumes of one comic
    // (`Wolverine & the X-Men (2011)` and `…(2014)` in production) used
    // to merge into one `series` row because identity resolution matched
    // by normalized_name+year alone. The shared row could only hold one
    // `folder_path`, so subsequent scans cycled — soft-deleting
    // whichever folder's issues weren't this scan's `seen_paths`, then
    // restoring them next time.
    //
    // After the fix, identity resolution also keys on `volume` (which
    // the filename parser extracts from `V<n>` tokens), so the two
    // folders resolve to distinct rows even when their ComicInfo years
    // overlap.
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let folder_v1 = tmp.path().join("Series Mu (2011)");
    let folder_v2 = tmp.path().join("Series Mu (2014)");
    std::fs::create_dir_all(&folder_v1).unwrap();
    std::fs::create_dir_all(&folder_v2).unwrap();
    // Filenames carry the year in a Mylar-style bracket group so the
    // parser populates `year` per folder. The dedup tuple
    // `(name, year, volume)` then differs on `year` alone, which is
    // the disambiguator most real-world sibling-volume releases rely
    // on. (`V<year>` tokens used to disambiguate by accidentally
    // landing in `volume`, but that pattern was a 99.9 %-pollution
    // bug — see `parsers::filename::plausible_volume`.)
    write_cbz(&folder_v1.join("Series Mu 001 (2011).cbz"), 1);
    write_cbz(&folder_v2.join("Series Mu 001 (2014).cbz"), 2);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(
        stats.series_created, 2,
        "each volume folder must own its own series row: {stats:?}",
    );

    let series = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(series.len(), 2, "two distinct series rows expected");
    let mut folder_paths: Vec<String> = series
        .iter()
        .filter_map(|s| s.folder_path.clone())
        .collect();
    folder_paths.sort();
    assert_eq!(
        folder_paths,
        vec![
            folder_v1.to_string_lossy().into_owned(),
            folder_v2.to_string_lossy().into_owned(),
        ],
        "each series row tracks exactly one folder",
    );

    // A second scan over a stable two-folder library must not cycle.
    let stats2 = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(
        stats2.issues_removed, 0,
        "stable library: nothing should be soft-deleted on rescan: {stats2:?}",
    );
    assert_eq!(
        stats2.issues_restored, 0,
        "stable library: nothing should be restored on rescan: {stats2:?}",
    );
}

#[tokio::test]
async fn folder_name_v_token_disambiguates_same_year_siblings() {
    // Two sibling folders, identical series name AND identical
    // publication year, distinguished only by a `V<N>` token in one
    // folder leaf. This is the "Howard the Duck V4 (2015) vs Howard
    // the Duck (2015)" pattern — common for series with multiple
    // runs that happened to launch in the same year as a relaunch.
    //
    // Pre-fix, both folders' filename inference produced
    // `volume = None`, the dedup tuple collided on
    // `(name, year, NULL)`, and the unique constraint
    // `series_library_normalized_uniq` swallowed the second insert
    // silently — the second folder's issues were dropped on the floor.
    //
    // The folder-leaf V-token fallback (`parsers::filename::folder_volume_token`)
    // catches `V2` here and feeds it into the identity hint, so the
    // tuples differ on `volume` and both folders create distinct rows.
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let folder_a = tmp.path().join("Series Nu (2015)");
    let folder_b = tmp.path().join("Series Nu V2 (2015)");
    std::fs::create_dir_all(&folder_a).unwrap();
    std::fs::create_dir_all(&folder_b).unwrap();
    write_cbz(&folder_a.join("Series Nu 001 (2015).cbz"), 1);
    write_cbz(&folder_b.join("Series Nu 001 (2015).cbz"), 2);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(
        stats.series_created, 2,
        "folder-leaf V-token must disambiguate same-year siblings: {stats:?}",
    );

    let mut series = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    series.sort_by_key(|s| s.volume);
    assert_eq!(series.len(), 2);
    assert_eq!(series[0].volume, None, "non-V folder gets NULL volume");
    assert_eq!(series[1].volume, Some(2), "V2 folder gets volume = 2");
}

#[tokio::test]
async fn series_json_volume_is_authoritative_over_filename_inference() {
    // Real-world case from one user's library (Deadpool & The Mercs
    // For Money, 2026-05-24). Both folders carry `series.json`
    // sidecars with the canonical volume, but the CBZ filenames are
    // Mylar3-stamped `V<year>` ("V2016") — pre-fix this poisoned
    // `issue.volume = 2016` for every file and propagated up to
    // `series.volume = 2016`, causing both folders to collide on
    // `(name, year=2016, volume=2016)`.
    //
    // With the fixes layered: filename `V2016` is rejected by the
    // plausibility filter; series.json overrides filename inference
    // for volume; the two folders' identity hints differ on volume
    // (NULL vs 2) and both rows are created.
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let folder_v1 = tmp.path().join("Series Xi (2016)");
    let folder_v2 = tmp.path().join("Series Xi V2 (2016)");
    std::fs::create_dir_all(&folder_v1).unwrap();
    std::fs::create_dir_all(&folder_v2).unwrap();

    // Mylar3-style filenames with `V<year>` (the contamination
    // source) and the year in a bracket group.
    write_cbz(&folder_v1.join("Series Xi V2016 001 (April 2016).cbz"), 1);
    write_cbz(
        &folder_v2.join("Series Xi V2016 001 (September 2016).cbz"),
        2,
    );

    // Mylar3 series.json sidecars carry the canonical volume.
    std::fs::write(
        folder_v1.join("series.json"),
        r#"{"version":"1.0.2","metadata":{"type":"comicSeries","name":"Series Xi","year":2016,"volume":null,"publisher":"Marvel"}}"#,
    )
    .unwrap();
    std::fs::write(
        folder_v2.join("series.json"),
        r#"{"version":"1.0.2","metadata":{"type":"comicSeries","name":"Series Xi","year":2016,"volume":2,"publisher":"Marvel"}}"#,
    )
    .unwrap();

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(
        stats.series_created, 2,
        "sidecar volume must override filename V-token: {stats:?}",
    );

    let mut series = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    series.sort_by_key(|s| s.volume);
    assert_eq!(series.len(), 2);
    assert_eq!(
        series[0].volume, None,
        "sidecar volume=null is preserved (not the filename's V2016)",
    );
    assert_eq!(
        series[1].volume,
        Some(2),
        "sidecar volume=2 wins over filename V2016",
    );
    // Both rows should also have publisher set from sidecar.
    assert_eq!(series[0].publisher.as_deref(), Some("Marvel"));
    assert_eq!(series[1].publisher.as_deref(), Some("Marvel"));
}

#[tokio::test]
async fn rescan_self_heals_stale_year_stamped_volume_from_sidecar() {
    // A series row that earlier-buggy scans stamped with
    // `volume = 2016` (the publication year, picked up from the
    // Mylar3 `V2016` filename token) should self-heal on rescan once
    // a `series.json` sidecar is present. The reconcile pass reads
    // the sidecar's `volume` and writes it through, overriding the
    // year-stamped value. No DB migration required.
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();

    let folder = tmp.path().join("Series Omicron (2017)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Series Omicron 001 (2017).cbz"), 1);
    std::fs::write(
        folder.join("series.json"),
        r#"{"version":"1.0.2","metadata":{"type":"comicSeries","name":"Series Omicron","year":2017,"volume":3,"publisher":"Marvel"}}"#,
    )
    .unwrap();

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    // Initial scan picks up volume from sidecar (= 3).
    scanner::scan_library(&state, lib_id).await.unwrap();
    let row = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.volume, Some(3));

    // Simulate the pre-fix state: manually corrupt the row's volume
    // to the year value, as if a buggy earlier scan had stamped it.
    let mut am: entity::series::ActiveModel = row.clone().into();
    am.volume = Set(Some(2017));
    am.update(&state.db).await.unwrap();

    // Re-touch the folder so the scanner doesn't fast-path-skip it.
    let new_mtime = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    let f = std::fs::File::open(&folder).unwrap();
    f.set_modified(new_mtime).ok();
    std::fs::write(
        folder.join("series.json"),
        r#"{"version":"1.0.2","metadata":{"type":"comicSeries","name":"Series Omicron","year":2017,"volume":3,"publisher":"Marvel"}}"#,
    )
    .unwrap();

    // Rescan should heal the stale volume back to 3.
    scanner::scan_library(&state, lib_id).await.unwrap();
    let healed = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        healed.volume,
        Some(3),
        "sidecar volume must overwrite year-stamped value on rescan",
    );
}

#[tokio::test]
async fn comicinfo_year_stamped_volume_is_rejected_at_ingest() {
    // ComicInfo.xml inside each CBZ commonly carries the same Mylar3
    // `V<year>` pollution as filenames — e.g. `<Volume>2016</Volume>`
    // on a 2016 publication. v0.6.1 gated only filename inference;
    // v0.6.2 extends the plausibility filter to ComicInfo + MetronInfo
    // so the year-stamp drops out at every read site.
    //
    // This test writes a CBZ whose ComicInfo carries `<Volume>2016</Volume>`
    // and asserts `issue.volume` ends up NULL (not 2016).
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Pi (2016)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz_with_comicinfo(
        &folder.join("Series Pi 001 (2016).cbz"),
        r#"<?xml version="1.0"?>
<ComicInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <Series>Series Pi</Series>
  <Number>1</Number>
  <Volume>2016</Volume>
  <Year>2016</Year>
</ComicInfo>"#,
    );

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();

    let series_row = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        series_row.volume, None,
        "series.volume must not be year-stamped from ComicInfo",
    );

    let issue_volumes: Vec<Option<i32>> = entity::issue::Entity::find()
        .filter(entity::issue::Column::SeriesId.eq(series_row.id))
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.volume)
        .collect();
    assert_eq!(
        issue_volumes,
        vec![None],
        "issue.volume must not be year-stamped from ComicInfo",
    );
}

#[tokio::test]
async fn sidecar_volume_null_clears_stale_year_stamp() {
    // A series row that an earlier-buggy scan stamped with
    // `volume = 2016`, whose folder now carries a `series.json`
    // sidecar with `volume: null` (the explicit "no volume" assertion
    // for single-run titles). Reconcile must write NULL through —
    // pre-fix behavior treated sidecar `null` the same as "no sidecar"
    // and fell back to MODE(), which re-affirmed the year-stamp.
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Rho (2017)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Series Rho 001 (2017).cbz"), 1);
    std::fs::write(
        folder.join("series.json"),
        r#"{"version":"1.0.2","metadata":{"type":"comicSeries","name":"Series Rho","year":2017,"volume":null,"publisher":"Marvel"}}"#,
    )
    .unwrap();

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let row = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    // First scan via the post-v0.6.1 path already lands at NULL
    // because sidecar.volume=None and filename V-token is filtered.
    assert_eq!(row.volume, None);

    // Simulate the pre-fix corruption: an earlier scan with the
    // contaminated parser had written `volume = 2017` here.
    let mut am: entity::series::ActiveModel = row.into();
    am.volume = Set(Some(2017));
    am.update(&state.db).await.unwrap();

    // Re-touch + rescan. The sidecar's explicit `null` volume must
    // overwrite the year-stamp.
    let new_time = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    let f = std::fs::File::open(&folder).unwrap();
    f.set_modified(new_time).ok();
    std::fs::write(
        folder.join("series.json"),
        r#"{"version":"1.0.2","metadata":{"type":"comicSeries","name":"Series Rho","year":2017,"volume":null,"publisher":"Marvel"}}"#,
    )
    .unwrap();

    scanner::scan_library(&state, lib_id).await.unwrap();
    let healed = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        healed.volume, None,
        "sidecar volume=null must clear year-stamped value on rescan",
    );
}

#[tokio::test]
async fn match_key_patch_persists_and_is_sticky() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Series Lambda (2025)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Lambda 001.cbz"), 1);
    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let series_row = SeriesEntity::find()
        .filter(SeriesCol::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let series_id = series_row.id;
    let series_slug = series_row.slug.clone();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/api/series/{series_slug}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(r#"{"match_key":"comicvine:1234"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = body_json(resp.into_body()).await;

    // Re-scan: scanner must NOT clear match_key (sticky).
    scanner::scan_library(&state, lib_id).await.unwrap();
    let after = SeriesEntity::find_by_id(series_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.match_key.as_deref(), Some("comicvine:1234"));

    // Empty string clears it.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/api/series/{series_slug}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(r#"{"match_key":"   "}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let after = SeriesEntity::find_by_id(series_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.match_key, None);
}
