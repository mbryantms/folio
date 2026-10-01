//! Scan-time CB7→CBZ conversion (per-library `auto_convert_cb7_on_scan`,
//! WP-6.5). Exercises the scanner ingest path for `.cb7` archives end to end
//! against the committed fixtures from `fixtures/make-cb7-fixture.py`.
//! Run with `cargo test -p server --test scanner_cb7_convert`.

mod common;

use common::TestApp;
use common::seed::LibrarySeed;
use entity::issue::Entity as IssueEntity;
use entity::library::Entity as LibraryEntity;
use entity::library_health_issue::Entity as HealthEntity;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use server::library::scanner;
use std::path::{Path, PathBuf};
use uuid::Uuid;

fn fixture(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    assert!(
        p.is_file(),
        "fixtures/{name} is committed (make-cb7-fixture.py)"
    );
    p
}

/// Stage `fixture` as `<root>/Thanos (2020)/Thanos 001.cb7`.
fn stage(root: &Path, fixture_name: &str) -> PathBuf {
    let folder = root.join("Thanos (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    let dst = folder.join("Thanos 001.cb7");
    std::fs::copy(fixture(fixture_name), &dst).unwrap();
    dst
}

async fn unsupported_issues(
    app: &TestApp,
    lib_id: Uuid,
) -> Vec<entity::library_health_issue::Model> {
    HealthEntity::find()
        .filter(entity::library_health_issue::Column::LibraryId.eq(lib_id))
        .filter(entity::library_health_issue::Column::Kind.eq("UnsupportedArchiveFormat"))
        .all(&app.state().db)
        .await
        .unwrap()
}

#[tokio::test]
async fn scan_converts_cb7_to_cbz_when_enabled() {
    for fixture_name in ["synthetic-3page.cb7", "synthetic-3page-solid.cb7"] {
        let app = TestApp::spawn().await;
        let tmp = tempfile::tempdir().unwrap();
        let cb7_path = stage(tmp.path(), fixture_name);
        let original = std::fs::read(&cb7_path).unwrap();

        let lib_id = LibrarySeed::new(tmp.path())
            .with_auto_convert_cb7_on_scan()
            .insert(&app.state().db)
            .await;
        let state = app.state();

        let stats = scanner::scan_library(&state, lib_id).await.expect("scan");
        assert_eq!(stats.files_converted, 1, "{fixture_name}: {stats:?}");
        assert_eq!(stats.files_added, 1, "{fixture_name}: {stats:?}");

        // On disk: `.cbz` written, `.cb7` moved to `.cb7.bak` byte-for-byte.
        let cbz_path = cb7_path.with_extension("cbz");
        assert!(cbz_path.exists(), "converted .cbz written");
        assert!(!cb7_path.exists(), "original .cb7 moved away");
        assert_eq!(
            std::fs::read(cb7_path.with_extension("cb7.bak")).unwrap(),
            original,
            "{fixture_name}: original kept as .cb7.bak"
        );

        // The issue row points at the `.cbz`, with all three pages and the
        // fixture's ComicInfo ingested.
        let issues = IssueEntity::find()
            .filter(entity::issue::Column::LibraryId.eq(lib_id))
            .all(&state.db)
            .await
            .unwrap();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].file_path.ends_with(".cbz"), "row points at .cbz");
        assert_eq!(issues[0].page_count, Some(3), "{fixture_name}: pages");

        // CB7 conversion doesn't touch the CBR page-editor confirm gate.
        let libr = LibraryEntity::find_by_id(lib_id)
            .one(&state.db)
            .await
            .unwrap()
            .unwrap();
        assert!(libr.cbr_convert_confirmed_at.is_none());

        assert!(
            unsupported_issues(&app, lib_id)
                .await
                .iter()
                .all(|i| i.resolved_at.is_some()),
            "no open UnsupportedArchiveFormat issue after conversion",
        );

        // Rescan is idempotent: `.cb7.bak` isn't a recognized extension.
        let second = scanner::scan_library(&state, lib_id).await.expect("rescan");
        assert_eq!(second.files_converted, 0, "no re-conversion: {second:?}");
        assert_eq!(second.files_added, 0, "no new rows: {second:?}");
    }
}

/// The CB7 flag is separate from the CBR one: a library that only opted
/// into CBR conversion leaves its `.cb7` files alone (skipped + health
/// issue), exactly like a library with both flags off.
#[tokio::test]
async fn scan_skips_cb7_when_only_cbr_conversion_enabled() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let cb7_path = stage(tmp.path(), "synthetic-3page.cb7");
    let original = std::fs::read(&cb7_path).unwrap();

    let lib_id = LibrarySeed::new(tmp.path())
        .with_auto_convert_cbr_on_scan()
        .insert(&app.state().db)
        .await;
    let stats = scanner::scan_library(&app.state(), lib_id)
        .await
        .expect("scan");
    assert_eq!(stats.files_converted, 0, "no conversion: {stats:?}");
    assert_eq!(stats.files_added, 0, "no rows added: {stats:?}");
    assert!(stats.files_skipped >= 1, "CB7 skipped: {stats:?}");

    assert_eq!(
        std::fs::read(&cb7_path).unwrap(),
        original,
        "file untouched"
    );
    assert!(!cb7_path.with_extension("cbz").exists(), "no .cbz written");
    let unsupported = unsupported_issues(&app, lib_id).await;
    assert_eq!(unsupported.len(), 1, "one UnsupportedArchiveFormat issue");
    assert!(unsupported[0].resolved_at.is_none(), "issue is open");
}

/// Conversion enabled but the archive is encrypted: conversion fails
/// softly — the file is left untouched and surfaces as unsupported.
#[tokio::test]
async fn scan_leaves_encrypted_cb7_untouched() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let cb7_path = stage(tmp.path(), "synthetic-3page-encrypted.cb7");
    let original = std::fs::read(&cb7_path).unwrap();

    let lib_id = LibrarySeed::new(tmp.path())
        .with_auto_convert_cb7_on_scan()
        .insert(&app.state().db)
        .await;
    let stats = scanner::scan_library(&app.state(), lib_id)
        .await
        .expect("scan");
    assert_eq!(stats.files_converted, 0, "{stats:?}");
    assert_eq!(stats.files_added, 0, "{stats:?}");

    assert_eq!(
        std::fs::read(&cb7_path).unwrap(),
        original,
        "file untouched"
    );
    assert!(!cb7_path.with_extension("cbz").exists());
    assert!(!cb7_path.with_extension("cb7.bak").exists());
    assert_eq!(unsupported_issues(&app, lib_id).await.len(), 1);
}
