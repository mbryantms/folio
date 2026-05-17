//! Regression tests for the scanner's content-hash decoupling fix.
//!
//! Before the fix, the scanner stored each issue's BLAKE3 content hash in
//! both `issues.id` (PK) and `issues.content_hash`, and the update path
//! re-`Set(...)`'d `am.id` to the freshly-computed hash on every scan.
//! When a user retagged a file with ComicTagger (adds a `ComicInfo.xml`,
//! changes the file's bytes), the UPDATE's WHERE clause used the new
//! hash and matched zero rows — sea-orm returned `RecordNotUpdated`,
//! the chunk transaction rolled back, and the row kept its old, sparse
//! metadata forever. Health-issues weren't emitted, so the failure was
//! silent.
//!
//! Plan: `~/.claude/plans/scanner-content-hash-1.0.md` (M3).

mod common;

use chrono::Utc;
use common::TestApp;
use entity::{
    issue::Entity as IssueEntity, library::ActiveModel as LibraryAM,
    progress_record::ActiveModel as ProgressAM, progress_record::Entity as ProgressEntity,
    user::ActiveModel as UserAM,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, Set};
use server::library::scanner;
use std::io::Write;
use std::path::Path;
use uuid::Uuid;

/// Write a CBZ to `path` with `unique_marker` bytes salted into the cover
/// page and an optional `ComicInfo.xml` payload. Two calls with different
/// markers produce different BLAKE3 hashes; two calls with identical
/// inputs may NOT produce identical bytes because `ZipWriter` records
/// per-file system times in the local headers. For byte-identical
/// fixtures, write once and `std::fs::copy` (see
/// `duplicate_content_still_emits_health_issue`).
fn write_cbz(path: &Path, comic_info: Option<&str>, unique_marker: u32) {
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
    let db = Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    LibraryAM {
        id: Set(id),
        name: Set("Retag Lib".into()),
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
        thumbnail_format: Set("webp".into()),
        thumbnail_cover_quality: Set(server::library::thumbnails::DEFAULT_COVER_QUALITY as i32),
        thumbnail_page_quality: Set(server::library::thumbnails::DEFAULT_STRIP_QUALITY as i32),
        generate_page_thumbs_on_scan: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

async fn seed_user(db_url: &str) -> Uuid {
    let db = Database::connect(db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    UserAM {
        id: Set(id),
        external_id: Set(format!("local:{id}")),
        display_name: Set("retag".into()),
        email: Set(Some(format!("retag-{id}@test"))),
        email_verified: Set(true),
        password_hash: Set(Some("x".into())),
        totp_secret: Set(None),
        state: Set("active".into()),
        role: Set("user".into()),
        token_version: Set(0),
        created_at: Set(now),
        updated_at: Set(now),
        last_login_at: Set(None),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

const RICH_COMICINFO: &str = r#"<?xml version="1.0"?>
<ComicInfo>
  <Title>There Will Always Be War</Title>
  <Series>Secret Warriors</Series>
  <Number>10</Number>
  <Volume>2009</Volume>
  <Year>2010</Year>
  <Month>1</Month>
  <Writer>Jonathan Hickman</Writer>
  <Penciller>Alessandro Vitti</Penciller>
  <Publisher>Marvel</Publisher>
</ComicInfo>"#;

/// M3 test 1: the canonical regression. Initial scan sees a CBZ with no
/// `ComicInfo.xml` and writes a sparse row. The user retags the file
/// (bytes change → new BLAKE3 hash), rescans, and metadata fields land
/// on the row. Pre-fix this hit `RecordNotUpdated` and the rich
/// metadata never made it in.
#[tokio::test]
async fn retag_refreshes_metadata() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Secret Warriors (2009)");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("Secret Warriors V2009 010.cbz");
    write_cbz(&file, None, 10);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(stats.files_added, 1);

    let before = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .expect("issue row exists after first scan");
    let original_id = before.id.clone();
    let original_hash = before.content_hash.clone();
    assert_eq!(
        before.id, before.content_hash,
        "first insert: id and content_hash agree",
    );
    assert!(before.writer.is_none(), "no ComicInfo yet → no writer");
    assert!(before.publisher.is_none());

    // Retag: rewrite the same path with a ComicInfo.xml and a different
    // marker. New bytes → new BLAKE3 hash → the scanner's update path
    // must survive the divergence.
    write_cbz(&file, Some(RICH_COMICINFO), 11);

    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(
        stats.files_updated, 1,
        "retag should produce an update, not rollback: {stats:?}",
    );

    let after = IssueEntity::find_by_id(original_id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .expect("same id row still present");
    assert_eq!(after.id, original_id, "id is the stable identifier");
    assert_ne!(
        after.content_hash, original_hash,
        "content_hash refreshed to new BLAKE3",
    );
    assert_eq!(after.writer.as_deref(), Some("Jonathan Hickman"));
    assert_eq!(after.publisher.as_deref(), Some("Marvel"));
    assert_eq!(after.year, Some(2010));
    assert_eq!(after.title.as_deref(), Some("There Will Always Be War"));
}

/// M3 test 2: user reading history must survive retags. `progress_records`
/// keys off `issues.id`; the fix's whole point is that `id` stays put
/// across content-hash drift, so a progress row written before retag
/// still resolves to the same issue after.
#[tokio::test]
async fn retag_preserves_id_and_fks() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Preserve (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("Preserve 001.cbz");
    write_cbz(&file, None, 700);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();
    let user_id = seed_user(&app.db_url).await;

    scanner::scan_library(&state, lib_id).await.unwrap();
    let issue = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let issue_id = issue.id.clone();

    // Write a progress record keyed off the issue's stable id.
    ProgressAM {
        user_id: Set(user_id),
        issue_id: Set(issue_id.clone()),
        last_page: Set(5),
        percent: Set(0.25),
        finished: Set(false),
        updated_at: Set(Utc::now().fixed_offset()),
        device: Set(Some("phone".into())),
    }
    .insert(&state.db)
    .await
    .unwrap();

    // Retag and rescan.
    write_cbz(&file, Some(RICH_COMICINFO), 701);
    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(stats.files_updated, 1);

    let progress = ProgressEntity::find_by_id((user_id, issue_id.clone()))
        .one(&state.db)
        .await
        .unwrap()
        .expect("progress row still keyed off the same issue id");
    assert_eq!(progress.last_page, 5);
    assert_eq!(progress.device.as_deref(), Some("phone"));

    let after = IssueEntity::find_by_id(issue_id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.writer.as_deref(), Some("Jonathan Hickman"));
}

/// M3 test 3: manual "Scan issue" with `force=true` re-reads the file
/// even when size+mtime would normally short-circuit the per-file fast
/// path. We backdate the mtime to its pre-retag value to prove the
/// force path doesn't depend on stat changes.
#[tokio::test]
async fn force_scan_picks_up_retag_with_unchanged_mtime() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Forced (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("Forced 001.cbz");
    write_cbz(&file, None, 800);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    scanner::scan_library(&state, lib_id).await.unwrap();
    let issue = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let issue_id = issue.id.clone();
    let original_mtime = std::fs::metadata(&file).unwrap().modified().unwrap();

    // Retag, then forcibly reset the file's mtime back to the pre-retag
    // value. Default scans would short-circuit; force=true must not.
    write_cbz(&file, Some(RICH_COMICINFO), 801);
    let pinned = filetime::FileTime::from_system_time(original_mtime);
    filetime::set_file_mtime(&file, pinned).unwrap();

    let stats = scanner::scan_issue_file(&state, lib_id, &issue_id, true, None)
        .await
        .unwrap();
    assert_eq!(
        stats.files_updated, 1,
        "force=true must re-ingest even with pinned mtime: {stats:?}",
    );
    assert_eq!(stats.files_unchanged, 0);

    let after = IssueEntity::find_by_id(issue_id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.writer.as_deref(), Some("Jonathan Hickman"));
    assert_eq!(after.publisher.as_deref(), Some("Marvel"));
}

/// M3 test 4: after retagging an existing row, a file with the new
/// content hash should still dedupe via the `content_hash` column —
/// because the row's `id` is the historical hash and no longer equals
/// the file's current bytes. Pre-M2 this used `find_by_id(new_hash)`
/// which would miss the row entirely.
///
/// Scenario: original file scanned (id=H1, content_hash=H1). File is
/// retagged in place (id=H1, content_hash=H2). Then the file is moved
/// to a new path on disk before the next scan. The file_path lookup at
/// the new path misses; the dedupe-by-content lookup must find the
/// existing row via `content_hash = H2` (NOT `id = H2`) and treat the
/// new path as a move, not a fresh insert.
#[tokio::test]
async fn retag_then_move_dedupes_by_content() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Drift (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    let original = folder.join("Drift 001.cbz");
    write_cbz(&original, None, 900);

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    scanner::scan_library(&state, lib_id).await.unwrap();
    let before = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let original_id = before.id.clone();
    let original_hash = before.content_hash.clone();

    // Retag in place — diverges id and content_hash.
    write_cbz(&original, Some(RICH_COMICINFO), 901);
    scanner::scan_library(&state, lib_id).await.unwrap();
    let after_retag = IssueEntity::find_by_id(original_id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let new_hash = after_retag.content_hash.clone();
    assert_ne!(new_hash, original_hash, "retag changed the file's hash");
    assert_ne!(
        new_hash, after_retag.id,
        "id stayed at the original hash; content_hash moved on",
    );

    // Move the file to a new path. file_path lookup will miss; dedupe-
    // by-content must catch it via the live `content_hash` column.
    let renamed = folder.join("Drift Renamed.cbz");
    std::fs::rename(&original, &renamed).unwrap();

    scanner::scan_library(&state, lib_id).await.unwrap();

    let active = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .filter(entity::issue::Column::RemovedAt.is_null())
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(active.len(), 1, "move shouldn't create a new row");
    let row = &active[0];
    assert_eq!(row.id, original_id, "stable identifier survives the move");
    assert_eq!(
        row.file_path,
        renamed.to_string_lossy(),
        "file_path tracks the new location",
    );
    assert_eq!(row.content_hash, new_hash);
}

/// M3 test 5: dedupe still flags two byte-identical files as duplicate
/// content rather than crashing the chunk transaction. Pre-M2 the
/// lookup was `find_by_id(hash)`; post-M2 it's
/// `filter(content_hash.eq(hash))`. Either way the second file must
/// produce a `DuplicateContent` health-issue path, not a panic.
#[tokio::test]
async fn duplicate_content_still_emits_health_issue() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Twins (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    let primary = folder.join("Twin A.cbz");
    write_cbz(&primary, None, 1000);
    // Byte-identical copy (ZipWriter would otherwise embed differing
    // local-header timestamps even with the same input).
    let duplicate = folder.join("Twin B.cbz");
    std::fs::copy(&primary, &duplicate).unwrap();

    let lib_id = create_library(&app, tmp.path()).await;
    let state = app.state();

    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(stats.files_seen, 2, "scanner walks both files: {stats:?}",);
    assert_eq!(
        stats.files_added, 1,
        "only one of the byte-twins is persisted: {stats:?}",
    );
    assert_eq!(
        stats.files_duplicate, 1,
        "the other is flagged DuplicateContent: {stats:?}",
    );

    let issue_count = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .all(&state.db)
        .await
        .unwrap()
        .len();
    assert_eq!(issue_count, 1);
}
