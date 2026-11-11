//! B10 — the scan's batch transaction holds database statements only.
//!
//! Everything that reads an archive — conversion, the content hash, the
//! parse, page probes, and the per-page hashes a content change re-anchors
//! markers and progress with (WP-6.2) — runs in `prepare_ingest`, before
//! the transaction opens, on the one handle the row's size and mtime come
//! from. `persist_prepared` writes rows from what prepare handed it.
//!
//! What this file pins:
//!  - persist reads no archive: it re-anchors correctly with the file
//!    deleted, and with the file replaced, it records (and anchors to) the
//!    version prepare read, never a newer one;
//!  - a prepared ingest that is dropped before persist (a scan aborted
//!    between the phases) writes nothing and leaves no partial row;
//!  - the moved-file case (persist adopts a row by content hash) still
//!    gets page hashes, read in prepare;
//!  - a hashed anchor written between the phases still follows its image
//!    (persist reads the pages itself, only from the recorded version);
//!  - prepare reads no pages when no anchor can use them (unchanged file,
//!    plain rename, a copy, no hashed anchor: `files_page_hashed`);
//!  - a mixed batch (CBZ, CBT, a converting CBR, an archive with an
//!    unreadable page, a byte-identical copy) ends with the rows, anchors and health
//!    findings the scanner produced before the change;
//!  - the issue rescan (the post-rewrite / post-edit path) reads the
//!    archive outside its transaction: it completes on a one-connection
//!    pool.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::LibrarySeed;
use entity::{issue, library, library_health_issue, marker, progress_record};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set, TransactionTrait};
use server::library::event_log::EventCollector;
use server::library::health::HealthCollector;
use server::library::scanner::{self, ScanStats, process};
use server::reading::page_remap::PAGE_DRIFT_TAG;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

/// A distinct PNG per colour (distinct bytes → distinct page hash).
fn png(rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(6, 4, image::Rgb(rgb));
    let mut buf = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

const RED: [u8; 3] = [200, 0, 0];
const GREEN: [u8; 3] = [0, 200, 0];
const BLUE: [u8; 3] = [0, 0, 200];
const YELLOW: [u8; 3] = [200, 200, 0];
const PURPLE: [u8; 3] = [120, 0, 120];

fn page_hash_of(rgb: [u8; 3]) -> String {
    blake3::hash(&png(rgb)).to_hex().to_string()
}

fn cbz_bytes(pages: &[[u8; 3]]) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .last_modified_time(zip::DateTime::default());
        for (i, rgb) in pages.iter().enumerate() {
            zw.start_file(format!("page-{:03}.png", i + 1), opts)
                .unwrap();
            zw.write_all(&png(*rgb)).unwrap();
        }
        zw.finish().unwrap();
    }
    buf.into_inner()
}

fn cbt_bytes(pages: &[[u8; 3]]) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut tw = tar::Builder::new(&mut buf);
        for (i, rgb) in pages.iter().enumerate() {
            let bytes = png(*rgb);
            let mut header = tar::Header::new_ustar();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tw.append_data(
                &mut header,
                format!("page-{:03}.png", i + 1),
                bytes.as_slice(),
            )
            .unwrap();
        }
        tw.into_inner().unwrap();
    }
    buf.into_inner()
}

/// A stored CBZ whose page `bad` has its image body flipped after the
/// signature: the entry still sniffs as a PNG (so it counts as a page),
/// but reading it to the end fails the zip CRC check.
fn cbz_with_unreadable_page(pages: &[[u8; 3]], bad: usize) -> Vec<u8> {
    let mut bytes = cbz_bytes(pages);
    let needle = png(pages[bad]);
    let at = bytes
        .windows(needle.len())
        .position(|w| w == needle.as_slice())
        .expect("stored page body");
    for b in &mut bytes[at + 20..at + 40] {
        *b ^= 0xFF;
    }
    bytes
}

/// Replace `path` via rename (new inode, like a download tool) and push
/// its mtime so the size+mtime fingerprint sees the change.
fn replace(path: &Path, bytes: &[u8], mtime_bump: i64) {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).unwrap();
    std::fs::rename(&tmp, path).unwrap();
    let t = filetime::FileTime::from_unix_time(Utc::now().timestamp() + mtime_bump, 0);
    filetime::set_file_mtime(path, t).unwrap();
}

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

async fn register(app: &TestApp) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"prepare@example.com","password":"correctly-horse-battery"}"#,
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
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id: Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap(),
    }
}

async fn post(app: &TestApp, auth: &Authed, uri: &str, body: serde_json::Value) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    app.router.clone().oneshot(req).await.unwrap().status()
}

async fn bookmark(app: &TestApp, auth: &Authed, issue_id: &str, page: i32) {
    let body = serde_json::json!({ "issue_id": issue_id, "page_index": page, "kind": "bookmark" });
    assert_eq!(
        post(app, auth, "/api/me/markers", body).await,
        StatusCode::CREATED
    );
}

async fn read_to(app: &TestApp, auth: &Authed, issue_id: &str, page: i32) {
    let body = serde_json::json!({ "issue_id": issue_id, "page": page });
    assert_eq!(post(app, auth, "/api/progress", body).await, StatusCode::OK);
}

async fn row_at(app: &TestApp, path: &Path) -> Option<issue::Model> {
    issue::Entity::find()
        .filter(issue::Column::FilePath.eq(path.to_string_lossy().into_owned()))
        .one(&app.state().db)
        .await
        .unwrap()
}

/// `(page_index, tags, page_hash)` per marker on the issue, sorted.
async fn markers(app: &TestApp, issue_id: &str) -> Vec<(i32, Vec<String>, Option<String>)> {
    let mut rows: Vec<_> = marker::Entity::find()
        .filter(marker::Column::IssueId.eq(issue_id))
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|m| (m.page_index, m.tags, m.page_hash))
        .collect();
    rows.sort();
    rows
}

async fn progress(app: &TestApp, user: Uuid, issue_id: &str) -> progress_record::Model {
    progress_record::Entity::find_by_id((user, issue_id.to_owned()))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
}

/// One series folder `Anchors/` holding `Anchors 001.cbz` =
/// `[RED, GREEN, BLUE, YELLOW]`, scanned, with a bookmark on GREEN (1)
/// and the resume position on BLUE (2), both hashed.
struct Anchored {
    app: TestApp,
    _dir: tempfile::TempDir,
    root: PathBuf,
    lib: Uuid,
    path: PathBuf,
    issue_id: String,
    auth: Authed,
}

async fn anchored() -> Anchored {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let series = root.join("Anchors");
    std::fs::create_dir_all(&series).unwrap();
    let path = series.join("Anchors 001.cbz");
    replace(&path, &cbz_bytes(&[RED, GREEN, BLUE, YELLOW]), 0);
    let lib = LibrarySeed::new(&root).insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib).await.unwrap();
    let issue_id = row_at(&app, &path).await.expect("scanned").id;
    let auth = register(&app).await;
    bookmark(&app, &auth, &issue_id, 1).await;
    read_to(&app, &auth, &issue_id, 2).await;
    Anchored {
        app,
        _dir: dir,
        root,
        lib,
        path,
        issue_id,
        auth,
    }
}

/// The scanner's per-folder outputs, kept across the two phases.
struct Outputs {
    stats: ScanStats,
    health: HealthCollector,
    events: EventCollector,
}

impl Outputs {
    fn new(lib: Uuid) -> Self {
        Self {
            stats: ScanStats::default(),
            health: HealthCollector::new(lib, Uuid::now_v7(), Utc::now().fixed_offset()),
            events: EventCollector::new(lib, None),
        }
    }
    fn borrow(&mut self) -> process::IngestOutputs<'_> {
        process::IngestOutputs {
            stats: &mut self.stats,
            health: &mut self.health,
            events: &mut self.events,
        }
    }
}

/// Run `prepare_ingest` for `path` the way a folder batch does (against
/// the pool, no transaction open), hand the result to `between`, then
/// persist it in its own transaction.
async fn prepare_then_persist(
    a: &Anchored,
    path: &Path,
    between: impl std::future::Future<Output = ()>,
) -> anyhow::Result<ScanStats> {
    let state = a.app.state();
    let lib = library::Entity::find_by_id(a.lib)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let row = issue::Entity::find_by_id(a.issue_id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let folder = path.parent().unwrap().to_path_buf();
    let manifest = process::IssueManifest::for_paths(&state.db, &[path.to_path_buf()])
        .await
        .unwrap();
    let ctx = process::IngestCtx {
        state: &state,
        lib: &lib,
        series_id: row.series_id,
        series_folder: &folder,
        manifest: Some(&manifest),
        force: false,
    };
    let mut out = Outputs::new(a.lib);
    let (size, mtime) = process::file_fingerprint(path).unwrap();
    let prepared = process::prepare_ingest(&ctx, &state.db, path, size, mtime, &mut out.borrow())
        .await?
        .expect("a changed file is prepared");
    between.await;
    let txn = state.db.begin().await.unwrap();
    process::persist_prepared(&ctx, &txn, prepared, None, &mut out.borrow()).await?;
    txn.commit().await.unwrap();
    Ok(out.stats)
}

/// The page hashes persist re-anchors with come from prepare: with the
/// archive deleted after prepare, the anchors still follow their images.
#[tokio::test]
async fn persist_reanchors_from_prepared_hashes_with_the_file_gone() {
    let a = anchored().await;
    // [RED, GREEN, BLUE, YELLOW] → [BLUE, YELLOW, RED, GREEN].
    replace(&a.path, &cbz_bytes(&[BLUE, YELLOW, RED, GREEN]), 60);
    let gone = a.path.clone();
    prepare_then_persist(&a, &a.path, async { std::fs::remove_file(&gone).unwrap() })
        .await
        .unwrap();

    assert_eq!(
        markers(&a.app, &a.issue_id).await,
        vec![(3, vec![], Some(page_hash_of(GREEN)))],
        "the bookmark followed GREEN to page 3"
    );
    let p = progress(&a.app, a.auth.user_id, &a.issue_id).await;
    assert_eq!(
        p.last_page, 0,
        "the resume position followed BLUE to page 0"
    );
    assert_eq!(p.page_hash, Some(page_hash_of(BLUE)));
}

/// A file replaced between prepare and persist: the row records the
/// version prepare read (size, mtime, content hash) and the anchors are
/// placed by that version's pages. The next scan sees the newer file.
#[tokio::test]
async fn file_changed_between_prepare_and_persist_records_the_prepared_version() {
    let a = anchored().await;
    let prepared_version = cbz_bytes(&[BLUE, YELLOW, RED, GREEN]);
    replace(&a.path, &prepared_version, 60);
    let (size, mtime) = process::file_fingerprint(&a.path).unwrap();
    let later = a.path.clone();
    prepare_then_persist(&a, &a.path, async {
        // [GREEN, RED, BLUE, PURPLE]: GREEN on 0, BLUE on 2.
        replace(&later, &cbz_bytes(&[GREEN, RED, BLUE, PURPLE]), 120);
    })
    .await
    .unwrap();

    let row = row_at(&a.app, &a.path).await.unwrap();
    assert_eq!(row.file_size, size);
    assert_eq!(row.file_mtime.to_utc(), mtime);
    assert_eq!(
        row.content_hash,
        blake3::hash(&prepared_version).to_hex().to_string()
    );
    assert_eq!(
        markers(&a.app, &a.issue_id).await,
        vec![(3, vec![], Some(page_hash_of(GREEN)))],
        "placed by the prepared version, not the newer file"
    );
    assert_eq!(
        progress(&a.app, a.auth.user_id, &a.issue_id)
            .await
            .last_page,
        0
    );

    // The newer bytes are a change the next scan picks up.
    let stats = scanner::scan_library(&a.app.state(), a.lib).await.unwrap();
    assert_eq!(stats.files_updated, 1, "{stats:?}");
    assert_eq!(
        markers(&a.app, &a.issue_id).await,
        vec![(0, vec![], Some(page_hash_of(GREEN)))]
    );
    assert_eq!(
        progress(&a.app, a.auth.user_id, &a.issue_id)
            .await
            .last_page,
        2
    );
}

/// Clear the page hash of every anchor on the issue, so prepare sees no
/// hashed anchor and reads no page hashes.
async fn unhash_anchors(a: &Anchored) {
    let db = &a.app.state().db;
    for m in marker::Entity::find()
        .filter(marker::Column::IssueId.eq(a.issue_id.clone()))
        .all(db)
        .await
        .unwrap()
    {
        let mut am: marker::ActiveModel = m.into();
        am.page_hash = Set(None);
        am.update(db).await.unwrap();
    }
    for p in progress_record::Entity::find()
        .filter(progress_record::Column::IssueId.eq(a.issue_id.clone()))
        .all(db)
        .await
        .unwrap()
    {
        let mut am: progress_record::ActiveModel = p.into();
        am.page_hash = Set(None);
        am.update(db).await.unwrap();
    }
}

/// Give the issue's bookmark (page 1, GREEN before the replacement) its
/// page hash back — what a reader bookmarking during the scan writes.
async fn hash_bookmark(a: &Anchored) {
    let db = &a.app.state().db;
    let m = marker::Entity::find()
        .filter(marker::Column::IssueId.eq(a.issue_id.clone()))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am: marker::ActiveModel = m.into();
    am.page_hash = Set(Some(page_hash_of(GREEN)));
    am.update(db).await.unwrap();
}

/// A hashed anchor written between the phases (prepare saw none, so it
/// read no page hashes): persist reads them itself, from the file version
/// the row records, and the anchor follows its image — not left on its
/// old index, now a different page, with no sign.
#[tokio::test]
async fn hashed_anchor_written_between_phases_follows_its_image() {
    let a = anchored().await;
    unhash_anchors(&a).await;
    // Same page count, reordered: the ordinal map alone is the identity.
    replace(&a.path, &cbz_bytes(&[BLUE, YELLOW, RED, GREEN]), 60);
    let stats = prepare_then_persist(&a, &a.path, hash_bookmark(&a))
        .await
        .unwrap();
    assert_eq!(stats.files_page_hashed, 0, "prepare saw no hashed anchor");
    assert_eq!(
        markers(&a.app, &a.issue_id).await,
        vec![(3, vec![], Some(page_hash_of(GREEN)))],
        "the bookmark followed GREEN to page 3"
    );
}

/// The same gap, but the file was replaced again before persist: the
/// pages persist could read are not the version the row records, so it
/// does not use them; the anchor keeps its ordinal (the fallback an
/// unreadable page has always had at an unchanged page count), and the
/// next scan, which sees the newer file, places it by its hash.
#[tokio::test]
async fn hashed_anchor_written_between_phases_with_the_file_replaced_again() {
    let a = anchored().await;
    unhash_anchors(&a).await;
    replace(&a.path, &cbz_bytes(&[BLUE, YELLOW, RED, GREEN]), 60);
    let later = a.path.clone();
    let between = async {
        hash_bookmark(&a).await;
        // GREEN on 0.
        replace(&later, &cbz_bytes(&[GREEN, RED, BLUE, PURPLE]), 120);
    };
    prepare_then_persist(&a, &a.path, between).await.unwrap();
    assert_eq!(
        markers(&a.app, &a.issue_id).await,
        vec![(1, vec![], Some(page_hash_of(GREEN)))],
        "not placed by a file version the row does not record"
    );

    let stats = scanner::scan_library(&a.app.state(), a.lib).await.unwrap();
    assert_eq!(
        (stats.files_updated, stats.files_page_hashed),
        (1, 1),
        "{stats:?}"
    );
    assert_eq!(
        markers(&a.app, &a.issue_id).await,
        vec![(0, vec![], Some(page_hash_of(GREEN)))]
    );
}

/// `files_page_hashed` pins the "do not read" side of the prepare gate.
#[tokio::test]
async fn page_hashes_are_read_only_when_an_anchor_can_use_them() {
    let a = anchored().await;
    let state = a.app.state();

    // Unchanged file, forced scan (the file reaches prepare's gate).
    let stats = scanner::scan_library_with(&state, a.lib, true)
        .await
        .unwrap();
    assert_eq!(
        (stats.files_updated, stats.files_page_hashed),
        (1, 0),
        "{stats:?}"
    );

    // Plain rename: size and mtime kept, so persist sees no content
    // change and re-anchors nothing.
    let renamed = a.root.join("Anchors").join("Anchors 001 (renamed).cbz");
    let before =
        filetime::FileTime::from_last_modification_time(&std::fs::metadata(&a.path).unwrap());
    std::fs::rename(&a.path, &renamed).unwrap();
    filetime::set_file_mtime(&renamed, before).unwrap();
    let stats = scanner::scan_library(&state, a.lib).await.unwrap();
    assert_eq!(stats.files_page_hashed, 0, "{stats:?}");
    assert_eq!(row_at(&a.app, &renamed).await.unwrap().id, a.issue_id);

    // A copy with a new mtime while the original (hashed anchors) is still
    // on disk: not a move (this library dedupes by content, so it is
    // reported as a duplicate and never inserted), so its pages are not
    // read.
    let copy = a.root.join("Anchors").join("Anchors 001 (copy).cbz");
    replace(&copy, &std::fs::read(&renamed).unwrap(), 60);
    let stats = scanner::scan_library(&state, a.lib).await.unwrap();
    assert_eq!(
        (stats.files_duplicate, stats.files_page_hashed),
        (1, 0),
        "{stats:?}"
    );
    std::fs::remove_file(&copy).unwrap();

    // A changed file whose anchors carry no hash.
    unhash_anchors(&a).await;
    replace(&renamed, &cbz_bytes(&[BLUE, YELLOW, RED, GREEN]), 60);
    let stats = scanner::scan_library(&state, a.lib).await.unwrap();
    assert_eq!(
        (stats.files_updated, stats.files_page_hashed),
        (1, 0),
        "{stats:?}"
    );

    // The same change with a hashed anchor is read.
    hash_bookmark(&a).await;
    replace(&renamed, &cbz_bytes(&[GREEN, RED, BLUE, PURPLE]), 120);
    let stats = scanner::scan_library(&state, a.lib).await.unwrap();
    assert_eq!(
        (stats.files_updated, stats.files_page_hashed),
        (1, 1),
        "{stats:?}"
    );
}

/// A scan aborted between the phases (the folder future dropped after
/// prepare) writes nothing: no partial row, no anchor moved, and the next
/// scan ingests the files normally.
#[tokio::test]
async fn prepared_ingest_dropped_before_persist_writes_nothing() {
    let a = anchored().await;
    let state = a.app.state();
    let before = row_at(&a.app, &a.path).await.unwrap();
    replace(&a.path, &cbz_bytes(&[BLUE, YELLOW, RED, GREEN]), 60);
    let new_path = a.path.with_file_name("Anchors 002.cbz");
    replace(&new_path, &cbz_bytes(&[PURPLE, RED]), 0);

    let lib = library::Entity::find_by_id(a.lib)
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let folder = a.path.parent().unwrap().to_path_buf();
    let paths = vec![a.path.clone(), new_path.clone()];
    let manifest = process::IssueManifest::for_paths(&state.db, &paths)
        .await
        .unwrap();
    let ctx = process::IngestCtx {
        state: &state,
        lib: &lib,
        series_id: before.series_id,
        series_folder: &folder,
        manifest: Some(&manifest),
        force: false,
    };
    let mut out = Outputs::new(a.lib);
    let mut prepared = Vec::new();
    for p in &paths {
        let (size, mtime) = process::file_fingerprint(p).unwrap();
        prepared.push(
            process::prepare_ingest(&ctx, &state.db, p, size, mtime, &mut out.borrow())
                .await
                .unwrap()
                .expect("prepared"),
        );
    }
    drop(prepared);

    assert!(row_at(&a.app, &new_path).await.is_none(), "no partial row");
    let after = row_at(&a.app, &a.path).await.unwrap();
    assert_eq!(after.content_hash, before.content_hash);
    assert_eq!(after.file_size, before.file_size);
    assert_eq!(
        markers(&a.app, &a.issue_id).await,
        vec![(1, vec![], Some(page_hash_of(GREEN)))]
    );
    assert_eq!(
        progress(&a.app, a.auth.user_id, &a.issue_id)
            .await
            .last_page,
        2
    );

    let stats = scanner::scan_library(&state, a.lib).await.unwrap();
    assert_eq!(
        (stats.files_added, stats.files_updated),
        (1, 1),
        "{stats:?}"
    );
    assert_eq!(
        markers(&a.app, &a.issue_id).await,
        vec![(3, vec![], Some(page_hash_of(GREEN)))]
    );
}

/// A file moved by copy-and-delete (new path, new mtime, same bytes):
/// persist adopts the old row by content hash and re-anchors with page
/// hashes prepare read for the new path — so a marker carrying a stale
/// drift note whose image is present gets the note cleared, as it did
/// when persist read the pages itself.
#[tokio::test]
async fn moved_file_reanchors_with_page_hashes_read_in_prepare() {
    let a = anchored().await;
    let db = &a.app.state().db;
    let m = marker::Entity::find()
        .filter(marker::Column::IssueId.eq(a.issue_id.clone()))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am: marker::ActiveModel = m.into();
    am.tags = Set(vec![PAGE_DRIFT_TAG.to_owned()]);
    am.update(db).await.unwrap();

    let moved = a.root.join("Anchors").join("Anchors 001 (moved).cbz");
    replace(&moved, &std::fs::read(&a.path).unwrap(), 60);
    std::fs::remove_file(&a.path).unwrap();
    let stats = scanner::scan_library(&a.app.state(), a.lib).await.unwrap();
    assert_eq!(stats.files_added, 0, "adopted, not added: {stats:?}");
    assert_eq!(
        stats.files_page_hashed, 1,
        "copy-and-delete with a new mtime: {stats:?}"
    );

    let row = row_at(&a.app, &moved).await.expect("row follows the move");
    assert_eq!(row.id, a.issue_id);
    assert_eq!(
        markers(&a.app, &a.issue_id).await,
        vec![(1, vec![], Some(page_hash_of(GREEN)))],
        "the image was found, so the drift note is cleared"
    );
}

/// The issue rescan — every post-rewrite and post-edit rescan — reads the
/// archive before its transaction opens. On a one-connection pool the old
/// order (open the transaction, then read the series row and the archive
/// through the pool) could not finish.
#[tokio::test]
async fn issue_rescan_reads_the_archive_outside_its_transaction() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let series = root.join("Narrow");
    std::fs::create_dir_all(&series).unwrap();
    let path = series.join("Narrow 001.cbz");
    replace(&path, &cbz_bytes(&[RED, GREEN, BLUE, YELLOW]), 0);
    let app = TestApp::spawn_with_pool(root.clone(), 1, 1).await;
    let state = app.state();
    let lib = LibrarySeed::new(&root).insert(&state.db).await;
    scanner::scan_library(&state, lib).await.unwrap();
    let issue_id = row_at(&app, &path).await.unwrap().id;
    let auth = register(&app).await;
    bookmark(&app, &auth, &issue_id, 1).await;

    replace(&path, &cbz_bytes(&[BLUE, YELLOW, RED, GREEN]), 60);
    let stats = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        scanner::scan_issue_file(&state, lib, &issue_id, false, None),
    )
    .await
    .expect("the issue rescan finishes on a one-connection pool")
    .unwrap();
    assert_eq!(stats.files_updated, 1, "{stats:?}");
    assert_eq!(
        markers(&app, &issue_id).await,
        vec![(3, vec![], Some(page_hash_of(GREEN)))]
    );
}

fn cbr_fixture() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic-3page.cbr");
    assert!(p.is_file(), "fixtures/synthetic-3page.cbr is committed");
    p
}

/// One rescan batch mixing ingest shapes, with hashed anchors on the
/// CBZ, the CBT and the archive whose page cannot be read. The results
/// below are the ones the scanner produced when persist still read the
/// pages inside the transaction: the same body was run against that code
/// and recorded every value asserted here.
#[tokio::test]
async fn mixed_batch_ends_with_the_same_rows_anchors_and_findings() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let series = root.join("Mixed (2020)");
    std::fs::create_dir_all(&series).unwrap();
    let cbz = series.join("Mixed 001.cbz");
    let cbt = series.join("Mixed 002.cbt");
    let bad = series.join("Mixed 003.cbz");
    replace(&cbz, &cbz_bytes(&[RED, GREEN, BLUE, YELLOW]), 0);
    replace(&cbt, &cbt_bytes(&[PURPLE, GREEN, BLUE, RED]), 0);
    replace(&bad, &cbz_bytes(&[YELLOW, PURPLE, RED]), 0);
    let state = app.state();
    let lib = LibrarySeed::new(&root)
        .with_auto_convert_cbr_on_scan()
        .insert(&state.db)
        .await;
    scanner::scan_library(&state, lib).await.unwrap();
    let auth = register(&app).await;
    let cbz_id = row_at(&app, &cbz).await.unwrap().id;
    let cbt_id = row_at(&app, &cbt).await.unwrap().id;
    let bad_id = row_at(&app, &bad).await.unwrap().id;
    bookmark(&app, &auth, &cbz_id, 1).await; // GREEN
    bookmark(&app, &auth, &cbt_id, 0).await; // PURPLE
    read_to(&app, &auth, &cbt_id, 3).await; // RED
    bookmark(&app, &auth, &bad_id, 1).await; // PURPLE

    // The rescan batch: two reorders, a reorder whose new first page is
    // unreadable, and a CBR to convert.
    //
    // No byte-identical copy of a file that changes in the same batch: a
    // folder's files are ingested in the order the filesystem lists them,
    // so whether such a copy meets a row that already carries its content
    // hash (a duplicate) or not (its own issue) differs from one
    // filesystem to the next. `page_hashes_are_read_only_when_an_anchor_can_use_them`
    // covers a copy whose original is unchanged, which has one answer.
    replace(&cbz, &cbz_bytes(&[BLUE, YELLOW, RED, GREEN]), 60);
    replace(&cbt, &cbt_bytes(&[RED, PURPLE, GREEN, BLUE]), 60);
    replace(
        &bad,
        &cbz_with_unreadable_page(&[RED, YELLOW, PURPLE], 0),
        60,
    );
    std::fs::copy(cbr_fixture(), series.join("Mixed 004.cbr")).unwrap();
    let stats = scanner::scan_library(&state, lib).await.unwrap();
    assert_eq!(
        (
            stats.files_updated,
            stats.files_added,
            stats.files_converted,
            stats.files_duplicate
        ),
        (3, 1, 1, 0),
        "{stats:?}"
    );

    // CBZ and CBT: anchors followed their images.
    assert_eq!(
        markers(&app, &cbz_id).await,
        vec![(3, vec![], Some(page_hash_of(GREEN)))]
    );
    assert_eq!(
        markers(&app, &cbt_id).await,
        vec![(1, vec![], Some(page_hash_of(PURPLE)))]
    );
    let p = progress(&app, auth.user_id, &cbt_id).await;
    assert_eq!((p.last_page, p.page_hash), (0, Some(page_hash_of(RED))));
    // The unreadable page fails the page hash: the anchor keeps its
    // ordinal (same page count), untagged, with its old hash.
    assert_eq!(
        markers(&app, &bad_id).await,
        vec![(1, vec![], Some(page_hash_of(PURPLE)))]
    );
    let bad_row = row_at(&app, &bad).await.unwrap();
    assert_eq!(bad_row.page_count, Some(3));
    assert_eq!(bad_row.state, issue::IssueState::Active);

    // The CBR was converted and ingested as a CBZ, the original kept as
    // `.cbr.bak`.
    let cbr_row = row_at(&app, &series.join("Mixed 004.cbz"))
        .await
        .expect("converted and ingested");
    assert_eq!(cbr_row.page_count, Some(3));
    assert!(series.join("Mixed 004.cbr.bak").is_file());
    let mut open: Vec<String> = library_health_issue::Entity::find()
        .filter(library_health_issue::Column::LibraryId.eq(lib))
        .filter(library_health_issue::Column::ResolvedAt.is_null())
        .all(&state.db)
        .await
        .unwrap()
        .into_iter()
        .map(|h| format!("{:?}", h.kind))
        .collect();
    open.sort();
    assert!(open.is_empty(), "{open:?}");
}
