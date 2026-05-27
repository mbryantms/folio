//! metadata-providers-1.0 M9 — perceptual hash integration tests.
//!
//! Covers:
//!   - `upsert_archive_cover_hashes` writes a new row when none
//!     exists, updates in place when one does (idempotent).
//!   - `run_backfill` picks up rows with NULL phash, decodes the
//!     on-disk bytes, writes the hashes.
//!   - `run_backfill` skips (doesn't error) rows whose on-disk file
//!     vanished.
//!
//! Unit-level phash tests (similarity scoring, JPEG tolerance, etc.)
//! live in the module itself; this file is the DB-touching surface.

mod common;

use chrono::Utc;
use common::TestApp;
use common::seed::{LibrarySeed, SeriesSeed, seed_issue};
use entity::issue_cover;
use image::{ImageBuffer, Rgb};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::metadata::phash;
use tempfile::tempdir;

fn write_test_png(path: &std::path::Path) {
    // 64×96 PNG with a per-pixel gradient — wrap-mod 256 keeps the
    // u8 math from overflowing at the edges.
    let buf: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(64, 96, |x, y| {
        Rgb([
            ((x * 4) % 256) as u8,
            ((y * 3) % 256) as u8,
            128,
        ])
    });
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir).unwrap();
    image::DynamicImage::ImageRgb8(buf)
        .save_with_format(path, image::ImageFormat::Png)
        .unwrap();
}

#[tokio::test]
async fn upsert_archive_cover_hashes_inserts_then_updates_in_place() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib, "Test Series").insert(&app.state().db).await;
    let issue_id = seed_issue(&app.state().db, lib, series_id, &dir.path().join("issue.cbz"), b"cbz", 1.0).await;
    let img = image::DynamicImage::ImageRgb8(ImageBuffer::from_fn(80, 120, |x, _| {
        Rgb([x as u8 * 3, 100, 200])
    }));

    let id1 = phash::upsert_archive_cover_hashes(&app.state().db, &issue_id, "thumbs/test/cover.webp", &img)
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
    let id2 = phash::upsert_archive_cover_hashes(&app.state().db, &issue_id, "thumbs/test/cover.webp", &img2)
        .await
        .expect("second upsert");
    assert_eq!(id2, id1, "should update in place, not insert a new row");

    let all_rows = issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(&issue_id))
        .filter(issue_cover::Column::SourceProvider.eq("archive_extracted"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(all_rows.len(), 1, "only one archive_extracted row should exist");
    assert_ne!(
        all_rows[0].phash, row1.phash,
        "phash should have been overwritten by the second call"
    );
}

#[tokio::test]
async fn run_backfill_hashes_null_rows_from_disk_bytes() {
    let app = TestApp::spawn().await;
    let data_path = app.state().cfg().data_path.clone();
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib, "Backfill series").insert(&app.state().db).await;
    let issue_id = seed_issue(&app.state().db, lib, series_id, &dir.path().join("bf.cbz"), b"x", 1.0).await;

    // Write a cover file to disk + create an issue_cover row that
    // points at it with NULL hashes — same shape rows pre-M9 left
    // behind.
    let rel = "thumbs/issues/bf/covers/cover.png".to_owned();
    let on_disk = data_path.join(&rel);
    write_test_png(&on_disk);
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
        local_path: Set(rel.clone()),
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

    let outcome = phash::run_backfill(&app.state().db, &data_path).await.unwrap();
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
}

#[tokio::test]
async fn run_backfill_skips_rows_with_missing_files() {
    let app = TestApp::spawn().await;
    let data_path = app.state().cfg().data_path.clone();
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib, "Missing-file series").insert(&app.state().db).await;
    let issue_id = seed_issue(&app.state().db, lib, series_id, &dir.path().join("mf.cbz"), b"x", 1.0).await;

    // Insert a row pointing at a non-existent file.
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

    let outcome = phash::run_backfill(&app.state().db, &data_path).await.unwrap();
    // Skipped, not errored — the file-missing case is expected
    // (the cover may have been wiped between scan + backfill).
    assert!(outcome.skipped >= 1);
    let row = issue_cover::Entity::find_by_id(cover_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert!(row.phash.is_none(), "skipped rows stay NULL");
}
