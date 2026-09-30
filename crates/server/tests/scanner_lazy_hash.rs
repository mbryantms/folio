//! First-import lazy-hash mode (roadmap WP-3.2, audit §3.1 "Import").
//!
//! A library with `trust_fingerprint_on_first_import` ingests its first
//! scan on size+mtime alone (issue id = path+size+mtime fingerprint,
//! `hash_algorithm = 0`), then `jobs::hash_backfill` stamps the real BLAKE3
//! and re-runs the dedupe check. These tests pin:
//!
//!   - the cold import of a representative generated set reads **zero**
//!     bytes for hashing (the "slow disk" cost the mode exists to avoid —
//!     asserted structurally via `bytes_hashed` against an inline-hashed
//!     control import of the same set, not via wall-clock timing);
//!   - hashes backfill, ids stay pinned, progress counts drain to zero;
//!   - retag detection works after the backfill (and a retag that lands
//!     *before* the backfill settles the row through the update path);
//!   - dedupe re-checks after hashing and the standard `DuplicateContent`
//!     health issue surfaces on the follow-up scan;
//!   - the mode only covers the first import;
//!   - the settings round-trip + progress endpoint.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::{TestApp, seed::LibrarySeed};
use entity::{
    issue::{self, Entity as IssueEntity},
    library_health_issue,
    progress_record::ActiveModel as ProgressAM,
    user::ActiveModel as UserAM,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::jobs::hash_backfill;
use server::library::hash::blake3_file;
use server::library::scanner::{self, process::lazy_fingerprint};
use std::io::Write;
use std::path::{Path, PathBuf};
use tower::ServiceExt;
use uuid::Uuid;

/// Stored-entry CBZ: one PNG-signed page salted with `marker`, an optional
/// ComicInfo, and `padding` bytes of incompressible filler so a full-file
/// hash costs real I/O (the thing lazy mode must not pay at scan time).
fn write_cbz(path: &Path, comic_info: Option<&str>, marker: u32, padding: usize) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    if let Some(xml) = comic_info {
        zw.start_file("ComicInfo.xml", opts).unwrap();
        zw.write_all(xml.as_bytes()).unwrap();
    }
    if padding > 0 {
        // xorshift filler keyed on the marker — deterministic, not zeros.
        let mut x = u64::from(marker) | 1;
        let mut buf = Vec::with_capacity(padding);
        while buf.len() < padding {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            buf.extend_from_slice(&x.to_le_bytes());
        }
        buf.truncate(padding);
        zw.start_file("padding.dat", opts).unwrap();
        zw.write_all(&buf).unwrap();
    }
    zw.finish().unwrap();
}

fn comic_info(series: &str, number: u32) -> String {
    format!(
        r#"<?xml version="1.0"?><ComicInfo><Series>{series}</Series><Number>{number}</Number><Year>2020</Year><Pages><Page Image="0" Type="FrontCover" ImageWidth="10" ImageHeight="15"/></Pages></ComicInfo>"#
    )
}

/// Representative multi-series library: `series` folders × `per_series`
/// issues, each archive padded to `padding` bytes. Returns total bytes.
fn generate_library(root: &Path, series: u32, per_series: u32, padding: usize) -> u64 {
    let mut total = 0u64;
    for s in 0..series {
        let name = format!("Stress Series {s:02}");
        let folder = root.join(format!("{name} (2020)"));
        std::fs::create_dir_all(&folder).unwrap();
        for n in 1..=per_series {
            let p = folder.join(format!("{name} {n:03}.cbz"));
            write_cbz(&p, Some(&comic_info(&name, n)), s * 1000 + n, padding);
            total += std::fs::metadata(&p).unwrap().len();
        }
    }
    total
}

async fn lazy_library(app: &TestApp, root: &Path) -> Uuid {
    LibrarySeed::new(root)
        .with_trust_fingerprint_on_first_import()
        .insert(&app.state().db)
        .await
}

async fn rows(app: &TestApp, lib_id: Uuid) -> Vec<issue::Model> {
    IssueEntity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .all(&app.state().db)
        .await
        .unwrap()
}

async fn seed_user(app: &TestApp) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    UserAM {
        id: Set(id),
        external_id: Set(format!("local:{id}")),
        display_name: Set("lazy".into()),
        email: Set(Some(format!("lazy-{id}@test"))),
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
    .insert(&app.state().db)
    .await
    .unwrap();
    id
}

async fn add_progress(app: &TestApp, user_id: Uuid, issue_id: &str) {
    ProgressAM {
        user_id: Set(user_id),
        issue_id: Set(issue_id.to_owned()),
        last_page: Set(0),
        percent: Set(0.5),
        finished: Set(false),
        finished_at: Set(None),
        updated_at: Set(Utc::now().fixed_offset()),
        device: Set(None),
        is_backfill: Set(false),
        run: Set(0),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

/// Done-when #1: a cold import completes without hashing. The same set
/// imported with the mode off hashes every byte, which is the cost a slow
/// disk (NAS / spinning rust) turns into hours.
#[tokio::test]
async fn cold_first_import_completes_without_hashing() {
    let app = TestApp::spawn().await;
    let state = app.state();

    let lazy_tmp = tempfile::tempdir().unwrap();
    let total_bytes = generate_library(lazy_tmp.path(), 8, 15, 256 * 1024);
    let lazy_id = lazy_library(&app, lazy_tmp.path()).await;

    let stats = scanner::scan_library(&state, lazy_id).await.unwrap();
    assert_eq!(stats.files_added, 120, "{stats:?}");
    assert_eq!(stats.files_hash_deferred, 120, "{stats:?}");
    assert_eq!(stats.bytes_hashed, 0, "no full-file reads: {stats:?}");
    assert!(
        !stats.parallel_phase_timings_ms.contains_key("hash"),
        "hash phase never ran: {stats:?}",
    );
    for row in rows(&app, lazy_id).await {
        assert_eq!(row.hash_algorithm, 0, "row pending: {}", row.file_path);
        let (size, mtime) = scanner::process::file_fingerprint(Path::new(&row.file_path)).unwrap();
        let fp = lazy_fingerprint(&row.file_path, size, mtime);
        assert_eq!(row.id, fp, "identity is the path fingerprint");
        assert_eq!(row.content_hash, fp, "placeholder until the hash lands");
        assert!(row.series_id != Uuid::nil());
        assert_eq!(row.page_count, Some(1), "metadata still ingested");
    }
    let (pending, total) = hash_backfill::progress(&state.db, lazy_id).await.unwrap();
    assert_eq!((pending, total), (120, 120));

    // Control: same shape, mode off → every byte hashed inline.
    let control_tmp = tempfile::tempdir().unwrap();
    let control_bytes = generate_library(control_tmp.path(), 8, 15, 256 * 1024);
    let control_id = LibrarySeed::new(control_tmp.path()).insert(&state.db).await;
    let control = scanner::scan_library(&state, control_id).await.unwrap();
    assert_eq!(control.files_added, 120);
    assert_eq!(control.files_hash_deferred, 0);
    assert_eq!(control.bytes_hashed, control_bytes);
    assert!(total_bytes >= 120 * 256 * 1024);
}

/// Done-when #2: hashes backfill; the id stays pinned; the progress count
/// drains; a rescan after the drain is a pure fast-path no-op.
#[tokio::test]
async fn hashes_backfill_and_ids_stay_pinned() {
    let app = TestApp::spawn().await;
    let state = app.state();
    let tmp = tempfile::tempdir().unwrap();
    generate_library(tmp.path(), 3, 4, 8 * 1024);
    let lib_id = lazy_library(&app, tmp.path()).await;
    scanner::scan_library(&state, lib_id).await.unwrap();
    let before = rows(&app, lib_id).await;
    assert_eq!(before.len(), 12);

    let out = hash_backfill::drain_library(&state, lib_id).await.unwrap();
    assert_eq!(out.hashed, 12, "{out:?}");
    assert_eq!(out.duplicates_removed, 0);
    assert_eq!(out.skipped, 0);

    for old in &before {
        let row = IssueEntity::find_by_id(old.id.clone())
            .one(&state.db)
            .await
            .unwrap()
            .expect("id unchanged by the backfill");
        assert_eq!(row.hash_algorithm, 1);
        assert_eq!(row.content_hash, blake3_file(&row.file_path).unwrap());
        assert_ne!(row.id, row.content_hash, "id stays the path fingerprint");
    }
    assert_eq!(
        hash_backfill::progress(&state.db, lib_id).await.unwrap(),
        (0, 12)
    );
    // A second drain has nothing to do.
    let again = hash_backfill::drain_library(&state, lib_id).await.unwrap();
    assert_eq!(again, hash_backfill::DrainOutcome::default());

    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(stats.files_added + stats.files_updated, 0, "{stats:?}");
    assert_eq!(stats.bytes_hashed, 0);
}

/// Done-when #3: retag detection still works after the backfill — same
/// id, fresh content hash, fresh metadata, reading progress kept.
#[tokio::test]
async fn retag_detected_after_backfill() {
    let app = TestApp::spawn().await;
    let state = app.state();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Retag Lazy (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("Retag Lazy 001.cbz");
    write_cbz(&file, None, 7, 4096);
    let lib_id = lazy_library(&app, tmp.path()).await;

    scanner::scan_library(&state, lib_id).await.unwrap();
    hash_backfill::drain_library(&state, lib_id).await.unwrap();
    let settled = rows(&app, lib_id).await.remove(0);
    assert_eq!(settled.hash_algorithm, 1);
    let user = seed_user(&app).await;
    add_progress(&app, user, &settled.id).await;

    // Retag: new bytes + ComicInfo. Bump mtime explicitly so the size+mtime
    // fast path can't mistake it for unchanged on coarse-mtime filesystems.
    write_cbz(
        &file,
        Some(
            r#"<?xml version="1.0"?><ComicInfo><Title>Tagged</Title><Writer>Someone</Writer></ComicInfo>"#,
        ),
        8,
        4096,
    );
    filetime::set_file_mtime(
        &file,
        filetime::FileTime::from_unix_time(Utc::now().timestamp() + 60, 0),
    )
    .unwrap();
    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(stats.files_updated, 1, "retag is an update: {stats:?}");
    assert_eq!(stats.files_added, 0);

    let after = IssueEntity::find_by_id(settled.id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .expect("same row");
    assert_ne!(after.content_hash, settled.content_hash);
    assert_eq!(after.content_hash, blake3_file(&file).unwrap());
    assert_eq!(after.hash_algorithm, 1);
    assert_eq!(after.title.as_deref(), Some("Tagged"));
    assert_eq!(after.writer.as_deref(), Some("Someone"));
    assert!(
        entity::progress_record::Entity::find_by_id((user, settled.id.clone()))
            .one(&state.db)
            .await
            .unwrap()
            .is_some(),
        "progress keyed off the pinned id survives",
    );
    assert_eq!(rows(&app, lib_id).await.len(), 1);
}

/// A retag that lands before the backfill reached the row settles it
/// through the scanner's update path (which always hashes); the drain then
/// has nothing left to do.
#[tokio::test]
async fn retag_before_backfill_settles_through_rescan() {
    let app = TestApp::spawn().await;
    let state = app.state();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Early (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("Early 001.cbz");
    write_cbz(&file, None, 21, 1024);
    let lib_id = lazy_library(&app, tmp.path()).await;
    scanner::scan_library(&state, lib_id).await.unwrap();
    let pending = rows(&app, lib_id).await.remove(0);
    assert_eq!(pending.hash_algorithm, 0);

    write_cbz(&file, Some(&comic_info("Early", 1)), 22, 2048);
    filetime::set_file_mtime(
        &file,
        filetime::FileTime::from_unix_time(Utc::now().timestamp() + 60, 0),
    )
    .unwrap();
    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(stats.files_updated, 1, "{stats:?}");

    let row = IssueEntity::find_by_id(pending.id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.hash_algorithm, 1, "update path settles the row");
    assert_eq!(row.content_hash, blake3_file(&file).unwrap());
    let out = hash_backfill::drain_library(&state, lib_id).await.unwrap();
    assert_eq!(out, hash_backfill::DrainOutcome::default());
}

/// Dedupe re-checks after hashing: byte-identical twins both ingest during
/// the lazy import (no hash to compare), the drain drops one, and the
/// follow-up scan re-ingests the dropped path through the normal hashed
/// path — `files_duplicate` + a `DuplicateContent` health issue, exactly
/// the steady state an inline-hashed import produces.
#[tokio::test]
async fn dedupe_rechecked_after_hashing() {
    let app = TestApp::spawn().await;
    let state = app.state();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Twins (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    let a = folder.join("Twin A.cbz");
    write_cbz(&a, None, 99, 1024);
    let b = folder.join("Twin B.cbz");
    std::fs::copy(&a, &b).unwrap();
    let lib_id = lazy_library(&app, tmp.path()).await;

    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(stats.files_added, 2, "no hash → no dedupe yet: {stats:?}");
    assert_eq!(stats.files_duplicate, 0);

    let out = hash_backfill::drain_library(&state, lib_id).await.unwrap();
    assert_eq!(out.duplicates_removed, 1, "{out:?}");
    let left = rows(&app, lib_id).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].hash_algorithm, 1);

    // The job enqueues a scoped rescan of the dropped copy's series folder;
    // drive it directly here.
    assert_eq!(out.rescan_series.len(), 1);
    let series_id = *out.rescan_series.iter().next().unwrap();
    let stats = scanner::scan_series_folder(
        &state,
        lib_id,
        series_id,
        &folder,
        scanner::ScanKind::Series,
        None,
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(stats.files_duplicate, 1, "{stats:?}");
    assert_eq!(stats.files_unchanged, 1);
    assert_eq!(rows(&app, lib_id).await.len(), 1);
    let dup = library_health_issue::Entity::find()
        .filter(library_health_issue::Column::LibraryId.eq(lib_id))
        .filter(library_health_issue::Column::Kind.eq("DuplicateContent"))
        .all(&state.db)
        .await
        .unwrap();
    assert_eq!(dup.len(), 1, "standard DuplicateContent finding");
}

/// When the reader already has progress on the copy that hashed second,
/// the dedupe keeps that row (its id, progress and URL) and drops the other.
#[tokio::test]
async fn dedupe_keeps_the_copy_with_reading_progress() {
    let app = TestApp::spawn().await;
    let state = app.state();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Pair (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    let a = folder.join("Pair A.cbz");
    write_cbz(&a, None, 5, 512);
    std::fs::copy(&a, folder.join("Pair B.cbz")).unwrap();
    let lib_id = lazy_library(&app, tmp.path()).await;
    scanner::scan_library(&state, lib_id).await.unwrap();

    // The drain walks pending rows by id; put progress on the one it
    // reaches second (it finds the first already settled → duplicate).
    let mut both = rows(&app, lib_id).await;
    both.sort_by(|x, y| x.id.cmp(&y.id));
    let read_id = both[1].id.clone();
    let user = seed_user(&app).await;
    add_progress(&app, user, &read_id).await;

    let out = hash_backfill::drain_library(&state, lib_id).await.unwrap();
    assert_eq!(out.duplicates_removed, 1);
    let left = rows(&app, lib_id).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, read_id, "the row with progress survives");
    assert_eq!(left[0].hash_algorithm, 1);
    assert!(
        entity::progress_record::Entity::find_by_id((user, read_id))
            .one(&state.db)
            .await
            .unwrap()
            .is_some()
    );
}

/// Twin CBZs in one lazily-imported library, returning `(lib_id, rows
/// sorted by id)`. The drain walks pending rows by id, so `rows[1]` is the
/// one it finds to be a duplicate of the already-settled `rows[0]`.
async fn lazy_twins(
    app: &TestApp,
    root: &Path,
    dedupe_by_content: bool,
) -> (Uuid, Vec<issue::Model>) {
    let folder = root.join("Twins (2021)");
    std::fs::create_dir_all(&folder).unwrap();
    let a = folder.join("Twin 001.cbz");
    write_cbz(&a, None, 77, 512);
    std::fs::copy(&a, folder.join("Twin 001 (copy).cbz")).unwrap();
    let lib_id = lazy_library(app, root).await;
    if !dedupe_by_content {
        let lib = entity::library::Entity::find_by_id(lib_id)
            .one(&app.state().db)
            .await
            .unwrap()
            .unwrap();
        let mut am: entity::library::ActiveModel = lib.into();
        am.dedupe_by_content = Set(false);
        am.update(&app.state().db).await.unwrap();
    }
    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    let mut both = rows(app, lib_id).await;
    assert_eq!(both.len(), 2);
    both.sort_by(|x, y| x.id.cmp(&y.id));
    (lib_id, both)
}

async fn decide(app: &TestApp, lib_id: Uuid, issue_id: &str, decision: &str) {
    entity::issue_duplicate_decision::ActiveModel {
        issue_id: Set(issue_id.to_owned()),
        library_id: Set(lib_id),
        decision: Set(decision.to_owned()),
        decided_by: Set(None),
        decided_at: Set(Utc::now().fixed_offset()),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

/// `dedupe_by_content = false` (WP-3.3 made it live): the post-hash
/// re-check never deletes — both copies stay as separate issues, settled,
/// with the same content hash, for the Duplicates page to group.
#[tokio::test]
async fn dedupe_off_keeps_both_copies_after_hashing() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (lib_id, both) = lazy_twins(&app, tmp.path(), false).await;

    let out = hash_backfill::drain_library(&app.state(), lib_id)
        .await
        .unwrap();
    assert_eq!(out.hashed, 2, "{out:?}");
    assert_eq!(out.duplicates_removed, 0);
    assert!(out.rescan_series.is_empty());
    let after = rows(&app, lib_id).await;
    assert_eq!(after.len(), 2, "both copies kept");
    assert!(after.iter().all(|r| r.hash_algorithm == 1));
    assert_eq!(after[0].content_hash, after[1].content_hash);
    let mut ids: Vec<_> = after.iter().map(|r| r.id.clone()).collect();
    ids.sort();
    assert_eq!(ids, both.iter().map(|r| r.id.clone()).collect::<Vec<_>>());
}

/// `dedupe_by_content = true` with an admin verdict: the decided copy is
/// kept over an undecided one (even though the drain would otherwise drop
/// it), and when both copies carry a verdict neither is deleted.
#[tokio::test]
async fn dedupe_never_deletes_a_row_with_a_duplicate_decision() {
    let app = TestApp::spawn().await;

    // (1) Verdict on the copy the drain would drop → it survives instead.
    let tmp = tempfile::tempdir().unwrap();
    let (lib_id, both) = lazy_twins(&app, tmp.path(), true).await;
    decide(&app, lib_id, &both[1].id, "keep").await;
    let out = hash_backfill::drain_library(&app.state(), lib_id)
        .await
        .unwrap();
    assert_eq!(out.duplicates_removed, 1, "{out:?}");
    let left = rows(&app, lib_id).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, both[1].id, "decided row kept");
    assert_eq!(left[0].hash_algorithm, 1);

    // (2) Verdicts on both → nothing deleted.
    let tmp2 = tempfile::tempdir().unwrap();
    let (lib2, both2) = lazy_twins(&app, tmp2.path(), true).await;
    decide(&app, lib2, &both2[0].id, "keep").await;
    decide(&app, lib2, &both2[1].id, "keep").await;
    let out = hash_backfill::drain_library(&app.state(), lib2)
        .await
        .unwrap();
    assert_eq!(out.duplicates_removed, 0, "{out:?}");
    assert_eq!(out.hashed, 2);
    let left = rows(&app, lib2).await;
    assert_eq!(left.len(), 2, "both decided rows kept");
    assert!(left.iter().all(|r| r.hash_algorithm == 1));
}

/// Lazy mode covers the *first* import only: once a full scan completed,
/// newly added files hash inline again (so moves/duplicates are caught at
/// ingest).
#[tokio::test]
async fn only_the_first_import_is_lazy() {
    let app = TestApp::spawn().await;
    let state = app.state();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Later (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Later 001.cbz"), None, 1, 1024);
    let lib_id = lazy_library(&app, tmp.path()).await;
    let first = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(first.files_hash_deferred, 1);

    let added = folder.join("Later 002.cbz");
    write_cbz(&added, None, 2, 1024);
    let second = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(second.files_added, 1, "{second:?}");
    assert_eq!(second.files_hash_deferred, 0);
    assert!(second.bytes_hashed > 0);
    let row = IssueEntity::find()
        .filter(issue::Column::FilePath.eq(added.to_string_lossy().into_owned()))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.hash_algorithm, 1);
    assert_eq!(row.id, blake3_file(&added).unwrap());
}

/// A file that changed or vanished after the lazy ingest is left pending
/// for the next scan rather than hashed against a stale row.
#[tokio::test]
async fn drain_skips_rows_whose_file_drifted() {
    let app = TestApp::spawn().await;
    let state = app.state();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Drift (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    let gone = folder.join("Drift 001.cbz");
    let changed = folder.join("Drift 002.cbz");
    write_cbz(&gone, None, 1, 512);
    write_cbz(&changed, None, 2, 512);
    let lib_id = lazy_library(&app, tmp.path()).await;
    scanner::scan_library(&state, lib_id).await.unwrap();

    std::fs::remove_file(&gone).unwrap();
    write_cbz(&changed, None, 3, 4096);
    let out = hash_backfill::drain_library(&state, lib_id).await.unwrap();
    assert_eq!(out.hashed, 0);
    assert_eq!(out.skipped, 2);
    assert!(
        rows(&app, lib_id)
            .await
            .iter()
            .all(|r| r.hash_algorithm == 0)
    );
}

// ───────── HTTP: settings toggle + progress endpoint ─────────

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
                    r#"{"email":"lazy@example.com","password":"correctly-horse-battery"}"#,
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

async fn call(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf);
    let body = match body {
        Some(b) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, json)
}

#[tokio::test]
async fn settings_toggle_and_progress_endpoint() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let root: PathBuf = tmp.path().to_path_buf();
    let folder = root.join("Api (2020)");
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(&folder.join("Api 001.cbz"), None, 1, 512);
    write_cbz(&folder.join("Api 002.cbz"), None, 2, 512);

    // Create with the flag set (so a `scan_now` import benefits)…
    let (status, lib) = call(
        &app,
        &auth,
        Method::POST,
        "/api/libraries",
        Some(serde_json::json!({
            "name": "Lazy Api",
            "root_path": root.to_string_lossy(),
            "trust_fingerprint_on_first_import": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{lib}");
    assert_eq!(lib["trust_fingerprint_on_first_import"], true);
    let slug = lib["slug"].as_str().unwrap().to_owned();
    let lib_id = Uuid::parse_str(lib["id"].as_str().unwrap()).unwrap();

    // …and it round-trips through PATCH.
    let (status, patched) = call(
        &app,
        &auth,
        Method::PATCH,
        &format!("/api/libraries/{slug}"),
        Some(serde_json::json!({ "trust_fingerprint_on_first_import": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(patched["trust_fingerprint_on_first_import"], false);
    let (_, patched) = call(
        &app,
        &auth,
        Method::PATCH,
        &format!("/api/libraries/{slug}"),
        Some(serde_json::json!({ "trust_fingerprint_on_first_import": true })),
    )
    .await;
    assert_eq!(patched["trust_fingerprint_on_first_import"], true);

    let uri = format!("/api/libraries/{slug}/hash-backfill");
    let (status, v) = call(&app, &auth, Method::GET, &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["first_import_active"], true);
    assert_eq!(v["enabled"], true);
    assert_eq!(v["pending"], 0);

    scanner::scan_library(&app.state(), lib_id).await.unwrap();
    let (_, v) = call(&app, &auth, Method::GET, &uri, None).await;
    assert_eq!(v["pending"], 2, "{v}");
    assert_eq!(v["total"], 2);
    assert_eq!(v["hashed"], 0);
    assert_eq!(v["first_import_active"], false, "first scan completed");

    let (status, started) = call(&app, &auth, Method::POST, &uri, None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    assert_eq!(started["enqueued"], true);
    assert_eq!(started["pending"], 2);

    hash_backfill::drain_library(&app.state(), lib_id)
        .await
        .unwrap();
    let (_, v) = call(&app, &auth, Method::GET, &uri, None).await;
    assert_eq!(v["pending"], 0);
    assert_eq!(v["hashed"], 2);
    let (_, started) = call(&app, &auth, Method::POST, &uri, None).await;
    assert_eq!(started["enqueued"], false, "nothing pending → no job");
}
