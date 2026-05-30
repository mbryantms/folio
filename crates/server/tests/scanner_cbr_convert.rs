//! Scan-time CBR→CBZ conversion (per-library `auto_convert_cbr_on_scan`).
//!
//! Exercises the scanner ingest path for `.cbr` archives end-to-end. RAR
//! files can't be created in-repo (the `unrar` crate is extract-only), so
//! these tests are `#[ignore]`d + gated on a local `fixtures/*.cbr`. Run
//! with `cargo test -p server --test scanner_cbr_convert -- --ignored`.

mod common;

use common::TestApp;
use common::seed::LibrarySeed;
use entity::issue::Entity as IssueEntity;
use entity::library::Entity as LibraryEntity;
use entity::library_health_issue::Entity as HealthEntity;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use server::library::scanner;

/// First `*.cbr` under the workspace `fixtures/` dir, if any.
fn first_cbr_fixture() -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("cbr"))
        })
}

#[tokio::test]
#[ignore = "needs a local fixtures/*.cbr (not committed); run with --ignored"]
async fn scan_converts_cbr_to_cbz_when_enabled() {
    let Some(fixture) = first_cbr_fixture() else {
        return; // no local fixture — skip
    };
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Thanos (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    let cbr_path = folder.join("Thanos 001.cbr");
    std::fs::copy(&fixture, &cbr_path).unwrap();

    let db = &app.state().db;
    let lib_id = LibrarySeed::new(tmp.path())
        .with_auto_convert_cbr_on_scan()
        .insert(db)
        .await;
    let state = app.state();

    let stats = scanner::scan_library(&state, lib_id).await.expect("scan");
    assert_eq!(stats.files_converted, 1, "one CBR converted: {stats:?}");
    assert_eq!(stats.files_added, 1, "converted CBZ ingested: {stats:?}");

    // On disk: the `.cbz` exists and the `.cbr` is gone. A true RAR also
    // leaves a `.cbr.bak` (the repack path); a ZIP-disguised-as-CBR is
    // renamed in place with no backup. Accept either.
    let cbz_path = cbr_path.with_extension("cbz");
    let bak_path = cbr_path.with_extension("cbr.bak");
    assert!(cbz_path.exists(), "converted .cbz written");
    assert!(!cbr_path.exists(), "original .cbr renamed away");

    // The issue row points at the `.cbz`.
    let issues = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(issues.len(), 1);
    assert!(issues[0].file_path.ends_with(".cbz"), "row points at .cbz");

    // The library remembers the first conversion so the page editor stops
    // prompting.
    let libr = LibraryEntity::find_by_id(lib_id)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert!(libr.cbr_convert_confirmed_at.is_some());

    // No stale `UnsupportedArchiveFormat` health issue lingers.
    let unsupported = HealthEntity::find()
        .filter(entity::library_health_issue::Column::LibraryId.eq(lib_id))
        .filter(entity::library_health_issue::Column::Kind.eq("UnsupportedArchiveFormat"))
        .all(&state.db)
        .await
        .unwrap();
    assert!(
        unsupported.iter().all(|i| i.resolved_at.is_some()),
        "no open UnsupportedArchiveFormat issue after conversion",
    );

    // Rescan is idempotent: the `.cbr.bak` isn't a recognized extension so
    // conversion never re-fires, and the `.cbz` is unchanged.
    let second = scanner::scan_library(&state, lib_id).await.expect("rescan");
    assert_eq!(second.files_converted, 0, "no re-conversion: {second:?}");
    assert_eq!(second.files_added, 0, "no new rows: {second:?}");
    assert!(cbz_path.exists(), ".cbz still present on rescan");
    let _ = bak_path; // RAR-only artifact; not asserted here.
}

#[tokio::test]
#[ignore = "needs a local fixtures/*.cbr (not committed); run with --ignored"]
async fn scan_skips_cbr_when_disabled() {
    let Some(fixture) = first_cbr_fixture() else {
        return;
    };
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Thanos (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    let cbr_path = folder.join("Thanos 001.cbr");
    std::fs::copy(&fixture, &cbr_path).unwrap();

    let db = &app.state().db;
    // Default seed: conversion flag off.
    let lib_id = LibrarySeed::new(tmp.path()).insert(db).await;
    let state = app.state();

    let stats = scanner::scan_library(&state, lib_id).await.expect("scan");
    assert_eq!(stats.files_converted, 0, "no conversion: {stats:?}");
    assert_eq!(stats.files_added, 0, "no rows added: {stats:?}");
    assert!(stats.files_skipped >= 1, "CBR skipped: {stats:?}");

    // The `.cbr` is untouched; no `.cbz` was written.
    assert!(cbr_path.exists(), ".cbr left in place");
    assert!(!cbr_path.with_extension("cbz").exists(), "no .cbz written");

    // An open `UnsupportedArchiveFormat` health issue surfaces the skip.
    let unsupported = HealthEntity::find()
        .filter(entity::library_health_issue::Column::LibraryId.eq(lib_id))
        .filter(entity::library_health_issue::Column::Kind.eq("UnsupportedArchiveFormat"))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(unsupported.len(), 1, "one UnsupportedArchiveFormat issue");
    assert!(unsupported[0].resolved_at.is_none(), "issue is open");
}
