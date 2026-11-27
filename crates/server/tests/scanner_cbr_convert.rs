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

/// An old RAR whose header after the last file is damaged (Fantastic Four
/// (1961) #309 in production: every page reads, then UnRAR answers "File
/// header damaged") converts with all its pages instead of failing every
/// scan as `UnsupportedArchiveFormat`.
#[tokio::test]
async fn scan_converts_cbr_with_a_damaged_trailing_header() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/synthetic-3page-damaged-tail.cbr");
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Fantastic Four (1961)");
    std::fs::create_dir_all(&folder).unwrap();
    let cbr_path = folder.join("Fantastic Four V1961 309 (December 1987).cbr");
    std::fs::copy(&fixture, &cbr_path).unwrap();

    let db = &app.state().db;
    let lib_id = LibrarySeed::new(tmp.path())
        .with_auto_convert_cbr_on_scan()
        .insert(db)
        .await;
    let state = app.state();

    let stats = scanner::scan_library(&state, lib_id).await.expect("scan");
    assert_eq!(stats.files_converted, 1, "the CBR converted: {stats:?}");
    assert_eq!(
        stats.files_added, 1,
        "the converted CBZ ingested: {stats:?}"
    );
    assert!(cbr_path.with_extension("cbz").exists());
    assert!(cbr_path.with_extension("cbr.bak").exists(), "original kept");

    let issues = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(
        issues[0].page_count,
        Some(3),
        "every page before the bad block"
    );

    let unsupported = HealthEntity::find()
        .filter(entity::library_health_issue::Column::LibraryId.eq(lib_id))
        .filter(entity::library_health_issue::Column::Kind.eq("UnsupportedArchiveFormat"))
        .all(&state.db)
        .await
        .unwrap();
    assert!(
        unsupported.is_empty(),
        "no UnsupportedArchiveFormat: {unsupported:?}"
    );
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
        archive::container::detect_container_path_unchecked(&cbz_path).unwrap(),
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
    assert_eq!(issues[0].state, entity::issue::IssueState::Active);
    assert_eq!(issues[0].page_count, Some(3));

    // No misleading ComicInfo diagnosis anywhere.
    let rows = HealthEntity::find()
        .filter(entity::library_health_issue::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap();
    assert!(
        rows.iter().all(|r| r.kind
            != entity::library_health_issue::HealthIssueKind::MalformedComicInfo
            && r.kind != entity::library_health_issue::HealthIssueKind::MalformedArchive),
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
        .filter(|r| {
            r.kind == entity::library_health_issue::HealthIssueKind::UnsupportedArchiveFormat
        })
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
        rows.iter()
            .all(|r| r.kind != entity::library_health_issue::HealthIssueKind::MalformedComicInfo),
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

/// A CBR the conversion refuses on a protective limit is an
/// `ArchiveSafetyRefusal` naming the check, not an unsupported format, and
/// the source file is left as it was.
#[tokio::test]
async fn conversion_refused_by_a_limit_is_a_safety_refusal() {
    let fixture = first_cbr_fixture().expect("fixtures/synthetic-3page.cbr is committed");
    let app = TestApp::spawn().await;
    let state = app.state();
    // The fixture holds three pages; one is the limit.
    let mut cfg = (*state.cfg()).clone();
    cfg.archive_max_entries = 1;
    state.replace_cfg(cfg);

    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Thanos (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    let cbr_path = folder.join("Thanos 001.cbr");
    std::fs::copy(&fixture, &cbr_path).unwrap();
    let original = std::fs::read(&cbr_path).unwrap();

    let lib_id = LibrarySeed::new(tmp.path())
        .with_auto_convert_cbr_on_scan()
        .insert(&state.db)
        .await;
    let stats = scanner::scan_library(&state, lib_id).await.expect("scan");
    assert_eq!(stats.files_converted, 0, "{stats:?}");
    assert_eq!(stats.files_added, 0, "{stats:?}");
    assert_eq!(stats.files_skipped, 1, "{stats:?}");

    assert_eq!(
        std::fs::read(&cbr_path).unwrap(),
        original,
        "file untouched"
    );
    assert!(!cbr_path.with_extension("cbz").exists(), "no .cbz written");

    let rows = HealthEntity::find()
        .filter(entity::library_health_issue::Column::LibraryId.eq(lib_id))
        .filter(entity::library_health_issue::Column::ResolvedAt.is_null())
        .all(&state.db)
        .await
        .unwrap();
    let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
    assert_eq!(kinds, ["ArchiveSafetyRefusal"], "{kinds:?}");
    assert_eq!(rows[0].payload["data"]["check"], "entry_count");
}

/// Copy a committed damaged fixture into a series folder of a fresh
/// library that converts CBRs, and scan it.
async fn scan_damaged_fixture(
    app: &TestApp,
    fixture: &str,
    tmp: &std::path::Path,
) -> (
    uuid::Uuid,
    std::path::PathBuf,
    server::library::scanner::ScanStats,
) {
    let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(fixture);
    let folder = tmp.join("The Avengers (1963)");
    std::fs::create_dir_all(&folder).unwrap();
    let cbr_path = folder.join("The Avengers V1963 253 (March 1985).cbr");
    std::fs::copy(&src, &cbr_path).unwrap();
    let lib_id = LibrarySeed::new(tmp)
        .with_auto_convert_cbr_on_scan()
        .insert(&app.state().db)
        .await;
    let stats = scanner::scan_library(&app.state(), lib_id)
        .await
        .expect("scan");
    (lib_id, cbr_path, stats)
}

async fn damage_rows(app: &TestApp, issue_id: &str) -> Vec<entity::issue_damaged_page::Model> {
    entity::issue_damaged_page::Entity::find()
        .filter(entity::issue_damaged_page::Column::IssueId.eq(issue_id))
        .all(&app.state().db)
        .await
        .unwrap()
}

async fn open_damage_findings(
    app: &TestApp,
    lib_id: uuid::Uuid,
) -> Vec<entity::library_health_issue::Model> {
    HealthEntity::find()
        .filter(entity::library_health_issue::Column::LibraryId.eq(lib_id))
        .filter(entity::library_health_issue::Column::Kind.eq("DamagedPage"))
        .filter(entity::library_health_issue::Column::ResolvedAt.is_null())
        .all(&app.state().db)
        .await
        .unwrap()
}

/// A CBR with one page that fails its checksum (The Avengers (1963) #253
/// in production) converts with every page instead of being skipped: the
/// damaged page keeps the bytes UnRAR decoded, and the damage is recorded
/// on the issue, reported as a `DamagedPage` finding, and survives a
/// forced rescan. The original is kept as `.cbr.bak`.
#[tokio::test]
async fn scan_converts_cbr_with_a_damaged_page_and_records_it() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (lib_id, cbr_path, stats) =
        scan_damaged_fixture(&app, "synthetic-3page-damaged-page.cbr", tmp.path()).await;
    assert_eq!(stats.files_converted, 1, "{stats:?}");
    assert_eq!(stats.files_added, 1, "{stats:?}");
    let cbz_path = cbr_path.with_extension("cbz");
    assert!(cbz_path.exists());
    assert!(cbr_path.with_extension("cbr.bak").exists(), "original kept");

    let issues = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(
        issues[0].page_count,
        Some(3),
        "every page, the damaged one too"
    );

    let rows = damage_rows(&app, &issues[0].id).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row.entry_name, "p0002.jpg");
    assert_eq!(row.source_entry, "page-002.jpg");
    assert_eq!(row.page_index, 1);
    assert_eq!(row.kind, "salvaged");
    assert_eq!(row.reason, archive::damage::CHECKSUM_FAILED);
    assert_eq!((row.declared_size, row.recovered_size), (86, 86));
    assert_eq!(
        row.source_backup.as_deref(),
        Some(
            cbr_path
                .with_extension("cbr.bak")
                .to_string_lossy()
                .as_ref()
        )
    );

    // The CBZ holds the salvaged bytes: the JPEG start, the flipped byte.
    let mut cbz =
        archive::cbz::Cbz::open_path_unchecked(&cbz_path, archive::ArchiveLimits::default())
            .unwrap();
    let page = cbz.read_entry_bytes_by_name("p0002.jpg").unwrap();
    assert!(page.starts_with(&[0xFF, 0xD8, 0xFF]));
    assert_eq!(page[40], 2 ^ 0xFF);

    let findings = open_damage_findings(&app, lib_id).await;
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].payload["data"]["page_index"], 1);
    assert_eq!(findings[0].payload["data"]["kind"], "salvaged");
    assert!(
        HealthEntity::find()
            .filter(entity::library_health_issue::Column::LibraryId.eq(lib_id))
            .filter(entity::library_health_issue::Column::Kind.eq("UnsupportedArchiveFormat"))
            .all(&app.state().db)
            .await
            .unwrap()
            .is_empty()
    );

    // A forced rescan reads the file again and keeps both.
    scanner::scan_library_with(&app.state(), lib_id, true)
        .await
        .expect("rescan");
    assert_eq!(damage_rows(&app, &issues[0].id).await.len(), 1);
    assert_eq!(open_damage_findings(&app, lib_id).await.len(), 1);
}

/// When nothing usable survives (the damaged byte is the page's first, so
/// what UnRAR decoded is no image), the page becomes Folio's placeholder,
/// written as PNG and recorded as such.
#[tokio::test]
async fn scan_writes_a_placeholder_for_a_page_with_nothing_usable() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (lib_id, cbr_path, stats) =
        scan_damaged_fixture(&app, "synthetic-3page-damaged-page-head.cbr", tmp.path()).await;
    assert_eq!(stats.files_added, 1, "{stats:?}");
    let issue = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(issue.page_count, Some(3));
    let rows = damage_rows(&app, &issue.id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, "placeholder");
    assert_eq!(rows[0].entry_name, "p0002.png");
    assert_eq!(rows[0].page_index, 1);

    let mut cbz = archive::cbz::Cbz::open_path_unchecked(
        cbr_path.with_extension("cbz"),
        archive::ArchiveLimits::default(),
    )
    .unwrap();
    assert_eq!(
        cbz.read_entry_bytes_by_name("p0002.png").unwrap(),
        server::library::damaged_pages::placeholder_page()
    );
}

/// Replacing the converted file with a good copy clears the record and
/// resolves the finding: the page no longer holds the damaged bytes.
#[tokio::test]
async fn a_replacement_file_clears_the_damage_record() {
    use std::io::Write;
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (lib_id, cbr_path, _) =
        scan_damaged_fixture(&app, "synthetic-3page-damaged-page.cbr", tmp.path()).await;
    let issue = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(damage_rows(&app, &issue.id).await.len(), 1);

    // A good copy under the same name: same page names, clean bytes.
    let cbz_path = cbr_path.with_extension("cbz");
    {
        let f = std::fs::File::create(&cbz_path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for i in 1..=3u8 {
            zw.start_file(format!("p{i:04}.jpg"), opts).unwrap();
            zw.write_all(&[
                0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0, i, i, i,
            ])
            .unwrap();
        }
        zw.finish().unwrap();
    }
    scanner::scan_library(&app.state(), lib_id)
        .await
        .expect("rescan");
    assert!(
        damage_rows(&app, &issue.id).await.is_empty(),
        "the good copy has no damaged page"
    );
    assert!(open_damage_findings(&app, lib_id).await.is_empty());
}
