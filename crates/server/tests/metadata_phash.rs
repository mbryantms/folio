//! metadata-providers-1.0 M9 — perceptual hash integration tests.
//!
//! Covers:
//!   - `upsert_archive_cover_hashes` writes a new row when none
//!     exists, updates in place when one does (idempotent).
//!   - `run_backfill` picks up rows with NULL phash, decodes the
//!     parent issue's archive cover page, writes the hashes.
//!   - `run_backfill` soft-skips rows whose archive can't be opened
//!     (missing file, undecodable bytes) without erroring.
//!
//! Unit-level phash tests (similarity scoring, JPEG tolerance, etc.)
//! live in the module itself; this file is the DB-touching surface.

mod common;

use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed, seed_issue};
use entity::issue_cover;
use image::{ImageBuffer, ImageFormat, Rgb};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::metadata::phash;
use std::io::{Cursor, Write};
use tempfile::tempdir;

/// Build a real, openable CBZ containing `pages` PNG entries. The
/// scanner's `archive::open` will refuse a stub `b"x"` payload — for
/// the backfill tests we need bytes that actually parse as a zip.
fn build_cbz_bytes(pages: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for n in 0..pages {
            zw.start_file(format!("page-{n:03}.png"), opts).unwrap();
            let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(32, 48, |x, y| {
                Rgb([((x + n as u32) * 8) as u8, (y * 5) as u8, 128])
            });
            let mut png = Vec::new();
            image::DynamicImage::ImageRgb8(img)
                .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
                .unwrap();
            zw.write_all(&png).unwrap();
        }
        zw.finish().unwrap();
    }
    buf
}

#[tokio::test]
async fn upsert_archive_cover_hashes_inserts_then_updates_in_place() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib, "Test Series")
        .insert(&app.state().db)
        .await;
    let issue_id = seed_issue(
        &app.state().db,
        lib,
        series_id,
        &dir.path().join("issue.cbz"),
        b"cbz",
        1.0,
    )
    .await;
    let img = image::DynamicImage::ImageRgb8(ImageBuffer::from_fn(80, 120, |x, _| {
        Rgb([x as u8 * 3, 100, 200])
    }));

    let id1 = phash::upsert_archive_cover_hashes(
        &app.state().db,
        &issue_id,
        "thumbs/test/cover.webp",
        &img,
    )
    .await
    .expect("first upsert");
    let row1 = issue_cover::Entity::find_by_id(id1)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(row1.phash.is_some());
    assert!(row1.dhash.is_some());
    assert!(row1.ahash.is_some());
    assert_eq!(row1.source_provider.as_deref(), Some("archive_extracted"));
    assert_eq!(row1.kind, "primary");
    assert_eq!(row1.ordinal, 0);
    assert!(!row1.is_active, "archive-extracted rows default inactive");

    // Idempotent: second call with a *different* image updates the
    // same row rather than inserting a new one.
    let img2 = image::DynamicImage::ImageRgb8(ImageBuffer::from_fn(80, 120, |_, y| {
        Rgb([200, y as u8 * 2, 50])
    }));
    let id2 = phash::upsert_archive_cover_hashes(
        &app.state().db,
        &issue_id,
        "thumbs/test/cover.webp",
        &img2,
    )
    .await
    .expect("second upsert");
    assert_eq!(id2, id1, "should update in place, not insert a new row");

    let all_rows = issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(&issue_id))
        .filter(issue_cover::Column::SourceProvider.eq("archive_extracted"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(
        all_rows.len(),
        1,
        "only one archive_extracted row should exist"
    );
    assert_ne!(
        all_rows[0].phash, row1.phash,
        "phash should have been overwritten by the second call"
    );
}

#[tokio::test]
async fn run_backfill_hashes_null_rows_from_archive_bytes() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib, "Backfill series")
        .insert(&app.state().db)
        .await;
    let cbz_path = dir.path().join("bf.cbz");
    let cbz_bytes = build_cbz_bytes(3);
    let issue_id = IssueSeed::new(lib, series_id, &cbz_path, &cbz_bytes, 1.0)
        .insert(&app.state().db)
        .await;

    // Insert an `issue_cover` row that points at a stale legacy
    // thumb path with NULL hashes — same shape rows pre-inline-hash
    // left behind. The backfill should ignore `local_path` for the
    // hash compute and decode the *archive* page instead.
    let cover_id = uuid::Uuid::now_v7();
    issue_cover::ActiveModel {
        id: Set(cover_id),
        issue_id: Set(issue_id.clone()),
        kind: Set("primary".into()),
        ordinal: Set(0),
        source_provider: Set(Some("archive_extracted".into())),
        source_external_id: Set(None),
        source_url: Set(None),
        variant_label: Set(None),
        variant_artist_person_id: Set(None),
        local_path: Set("thumbs/issues/bf/covers/cover.webp".into()),
        width: Set(None),
        height: Set(None),
        phash: Set(None),
        dhash: Set(None),
        ahash: Set(None),
        fetched_at: Set(Utc::now().fixed_offset()),
        is_active: Set(false),
    }
    .insert(&app.state().db)
    .await
    .unwrap();

    let limits = app.state().cfg().archive_limits();
    let outcome = phash::run_backfill(&app.state().db, limits).await.unwrap();
    assert!(outcome.considered >= 1);
    assert!(outcome.hashed >= 1);
    assert_eq!(outcome.errored, 0);

    let row = issue_cover::Entity::find_by_id(cover_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.phash.is_some(), "phash should be populated");
    assert!(row.dhash.is_some());
    assert!(row.ahash.is_some());
    assert!(
        row.width.is_some(),
        "width should be populated from decoded source"
    );
    assert!(row.height.is_some());
}

#[tokio::test]
async fn series_representative_phash_returns_hashed_primary_cover() {
    // Regression: the lookup joins the `issues` table; a stale `issue`
    // (singular) table name made the query error, and the orchestrator
    // swallowed it via `.unwrap_or(None)` → series-scope cover matching
    // silently disabled for every provider. This asserts the query runs
    // and returns the seeded primary-cover phash.
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib, "Repr series")
        .insert(&app.state().db)
        .await;
    let issue_id = seed_issue(
        &app.state().db,
        lib,
        series_id,
        &dir.path().join("repr.cbz"),
        b"cbz",
        1.0,
    )
    .await;
    let img = image::DynamicImage::ImageRgb8(ImageBuffer::from_fn(80, 120, |x, y| {
        Rgb([x as u8 * 2, y as u8, 64])
    }));
    let cover_id =
        phash::upsert_archive_cover_hashes(&app.state().db, &issue_id, "thumbs/repr/c.webp", &img)
            .await
            .expect("seed cover hashes");
    let seeded = issue_cover::Entity::find_by_id(cover_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .phash;

    let repr = phash::series_representative_phash(&app.state().db, series_id)
        .await
        .expect("query must not error");
    assert_eq!(
        repr, seeded,
        "representative phash = seeded primary cover phash"
    );
    assert!(repr.is_some());

    // A series with no hashed covers returns None (text-only fallback).
    let empty_series = SeriesSeed::new(lib, "No covers")
        .insert(&app.state().db)
        .await;
    let none = phash::series_representative_phash(&app.state().db, empty_series)
        .await
        .expect("query must not error");
    assert!(none.is_none());
}

#[tokio::test]
async fn run_backfill_skips_rows_when_archive_unreadable() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib, "Missing-file series")
        .insert(&app.state().db)
        .await;
    // Issue points at a path whose bytes aren't a valid archive.
    let issue_id = seed_issue(
        &app.state().db,
        lib,
        series_id,
        &dir.path().join("mf.cbz"),
        b"not-an-archive",
        1.0,
    )
    .await;

    let cover_id = uuid::Uuid::now_v7();
    issue_cover::ActiveModel {
        id: Set(cover_id),
        issue_id: Set(issue_id.clone()),
        kind: Set("primary".into()),
        ordinal: Set(0),
        source_provider: Set(Some("archive_extracted".into())),
        source_external_id: Set(None),
        source_url: Set(None),
        variant_label: Set(None),
        variant_artist_person_id: Set(None),
        local_path: Set("does/not/exist.png".into()),
        width: Set(None),
        height: Set(None),
        phash: Set(None),
        dhash: Set(None),
        ahash: Set(None),
        fetched_at: Set(Utc::now().fixed_offset()),
        is_active: Set(false),
    }
    .insert(&app.state().db)
    .await
    .unwrap();

    let limits = app.state().cfg().archive_limits();
    let outcome = phash::run_backfill(&app.state().db, limits).await.unwrap();
    // Skipped, not errored — soft-fail keeps the sweep moving when
    // one archive happens to be unreadable.
    assert!(outcome.skipped >= 1);
    let row = issue_cover::Entity::find_by_id(cover_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.phash.is_none(), "skipped rows stay NULL");
}
