//! Scan-time CBR→CBZ conversion (per-library `auto_convert_cbr_on_scan`).
//!
//! Exercises the scanner ingest path for `.cbr` archives end-to-end. RAR
//! compression can't be produced in-repo, but a STORED RAR5 can — see
//! `fixtures/make-cbr-fixture.py` + the committed `synthetic-3page.cbr`. Run
//! with `cargo test -p server --test scanner_cbr_convert`.

mod common;

use common::TestApp;
use common::seed::LibrarySeed;
use entity::issue::Entity as IssueEntity;
use entity::library::Entity as LibraryEntity;
use entity::library_health_issue::Entity as HealthEntity;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use server::library::scanner;

/// The committed synthetic `fixtures/synthetic-3page.cbr` (RAR5, stored, three
/// JFIF-stub pages — see `fixtures/make-cbr-fixture.py`), falling back to any
/// other `*.cbr` a developer dropped under `fixtures/`.
fn first_cbr_fixture() -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let synthetic = dir.join("synthetic-3page.cbr");
    if synthetic.is_file() {
        return Some(synthetic);
    }
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
async fn scan_converts_cbr_to_cbz_when_enabled() {
    let fixture = first_cbr_fixture()
        .expect("fixtures/synthetic-3page.cbr is committed — see fixtures/make-cbr-fixture.py");
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

/// A RAR wearing `.cbz` (Martian Manhunter (2006) #2/#3: opens in YACreader,
/// used to surface as a misleading `MalformedComicInfo` here) is the same
/// read-only container under the wrong name. With conversion enabled it is
/// repacked **in place** — same path, now a real ZIP — and ingested.
#[tokio::test]
async fn scan_converts_rar_named_cbz_in_place_when_enabled() {
    let fixture = first_cbr_fixture()
        .expect("fixtures/synthetic-3page.cbr is committed — see fixtures/make-cbr-fixture.py");
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Martian Manhunter (2006)");
    std::fs::create_dir_all(&folder).unwrap();
    let cbz_path = folder.join("Martian Manhunter V2006 002 (November 2006).cbz");
    std::fs::copy(&fixture, &cbz_path).unwrap();
    let original = std::fs::read(&cbz_path).unwrap();

    let db = &app.state().db;
    let lib_id = LibrarySeed::new(tmp.path())
        .with_auto_convert_cbr_on_scan()
        .insert(db)
        .await;
    let state = app.state();

    let stats = scanner::scan_library(&state, lib_id).await.expect("scan");
    assert_eq!(
        stats.files_converted, 1,
        "mislabeled RAR converted: {stats:?}"
    );
    assert_eq!(stats.files_added, 1, "converted CBZ ingested: {stats:?}");
    assert_eq!(
        stats.files_malformed, 0,
        "nothing reported malformed: {stats:?}"
    );

    // Same path, now a ZIP; the RAR bytes survive as `<name>.cbz.bak`.
    assert!(cbz_path.exists(), ".cbz still at its path");
    assert_eq!(
        archive::container::detect_container(&cbz_path).unwrap(),
        archive::container::Container::Zip
    );
    let bak = folder.join("Martian Manhunter V2006 002 (November 2006).cbz.bak");
    assert_eq!(
        std::fs::read(&bak).unwrap(),
        original,
        "original parked as .bak"
    );

    let issues = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].state, "active");
    assert_eq!(issues[0].page_count, Some(3));

    // No misleading ComicInfo diagnosis anywhere.
    let rows = HealthEntity::find()
        .filter(entity::library_health_issue::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    assert!(
        rows.iter()
            .all(|r| r.kind != "MalformedComicInfo" && r.kind != "MalformedArchive"),
        "no malformed rows: {rows:?}"
    );

    // Rescan is idempotent: `.cbz.bak` isn't enumerated and the `.cbz` is
    // now a plain ZIP on the fast path.
    let second = scanner::scan_library(&state, lib_id).await.expect("rescan");
    assert_eq!(second.files_converted, 0, "no re-conversion: {second:?}");
    assert_eq!(second.files_added, 0, "no new rows: {second:?}");
}

/// Same file with conversion off: it is skipped exactly like a `.cbr`
/// would be — an `UnsupportedArchiveFormat` row naming the real container,
/// not a `MalformedComicInfo` row blaming an XML file the archive doesn't
/// even contain. The bytes are untouched.
#[tokio::test]
async fn scan_flags_rar_named_cbz_as_unsupported_when_disabled() {
    let fixture = first_cbr_fixture()
        .expect("fixtures/synthetic-3page.cbr is committed — see fixtures/make-cbr-fixture.py");
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Martian Manhunter (2006)");
    std::fs::create_dir_all(&folder).unwrap();
    let cbz_path = folder.join("Martian Manhunter V2006 003 (December 2006).cbz");
    std::fs::copy(&fixture, &cbz_path).unwrap();
    let original = std::fs::read(&cbz_path).unwrap();

    let db = &app.state().db;
    let lib_id = LibrarySeed::new(tmp.path()).insert(db).await;
    let state = app.state();

    let stats = scanner::scan_library(&state, lib_id).await.expect("scan");
    assert_eq!(stats.files_converted, 0, "no conversion: {stats:?}");
    assert_eq!(stats.files_added, 0, "no rows added: {stats:?}");
    assert_eq!(stats.files_malformed, 0, "not counted malformed: {stats:?}");
    assert_eq!(
        std::fs::read(&cbz_path).unwrap(),
        original,
        "file untouched"
    );
    assert!(
        !folder
            .join("Martian Manhunter V2006 003 (December 2006).cbz.bak")
            .exists()
    );

    let rows = HealthEntity::find()
        .filter(entity::library_health_issue::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    let unsupported: Vec<_> = rows
        .iter()
        .filter(|r| r.kind == "UnsupportedArchiveFormat")
        .collect();
    assert_eq!(
        unsupported.len(),
        1,
        "one UnsupportedArchiveFormat row: {rows:?}"
    );
    assert!(unsupported[0].resolved_at.is_none(), "issue is open");
    assert_eq!(
        unsupported[0].payload["data"]["ext"].as_str(),
        Some("cbr"),
        "payload names the real container: {}",
        unsupported[0].payload
    );
    assert!(
        rows.iter().all(|r| r.kind != "MalformedComicInfo"),
        "no MalformedComicInfo row: {rows:?}"
    );
}

#[tokio::test]
async fn scan_skips_cbr_when_disabled() {
    let fixture = first_cbr_fixture()
        .expect("fixtures/synthetic-3page.cbr is committed — see fixtures/make-cbr-fixture.py");
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
