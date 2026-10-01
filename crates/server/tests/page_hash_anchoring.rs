//! Page-hash anchoring for markers and reading progress (roadmap WP-6.2,
//! audit R28).
//!
//! Markers and progress record the hash of the page image they were
//! written on. When a rescan finds new archive bytes under the same
//! issue (a replaced archive — re-downloaded release, another tool's
//! re-pack), anchors re-resolve to wherever their image now sits; only
//! anchors without a hash, or whose image is gone, fall back to the
//! WP-1.2 ordinal map. A marker whose image vanished gets the
//! `page-drift` tag (the drift note).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::LibrarySeed;
use entity::{issue, marker, progress_record};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::jobs::archive_edit::{ArchiveEditJob, PageOp, Rot, edit_one_issue};
use server::library::scanner;
use server::reading::page_remap::{PAGE_DRIFT_TAG, PAGE_REMOVED_TAG};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

/// A distinct PNG per colour (distinct bytes → distinct page hash).
fn png(rgb: [u8; 3]) -> Vec<u8> {
    // Non-square so a 90° rotation really changes the bytes.
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

/// Write a CBZ whose page order (natural sort of the entry names) is
/// `pages`, then push its mtime forward so the scanner's size+mtime
/// fingerprint sees a change even within one clock tick.
fn write_cbz(path: &Path, pages: &[[u8; 3]], mtime_bump: i64) {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (i, rgb) in pages.iter().enumerate() {
            zw.start_file(format!("page-{:03}.png", i + 1), opts)
                .unwrap();
            zw.write_all(&png(*rgb)).unwrap();
        }
        zw.finish().unwrap();
    }
    // Replace via rename like a download tool would (new inode).
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, buf.into_inner()).unwrap();
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
                    r#"{"email":"anchor@example.com","password":"correctly-horse-battery"}"#,
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

async fn create_marker(app: &TestApp, auth: &Authed, issue_id: &str, page: i32, kind: &str) {
    let mut body = serde_json::json!({ "issue_id": issue_id, "page_index": page, "kind": kind });
    if kind == "note" {
        body["body"] = serde_json::json!("remember this panel");
    }
    assert_eq!(
        post(app, auth, "/api/me/markers", body).await,
        StatusCode::CREATED
    );
}

async fn write_progress(app: &TestApp, auth: &Authed, issue_id: &str, page: i32) {
    let body = serde_json::json!({ "issue_id": issue_id, "page": page });
    assert_eq!(post(app, auth, "/api/progress", body).await, StatusCode::OK);
}

/// Library root with one series folder and a 4-page issue
/// `[RED, GREEN, BLUE, YELLOW]`, scanned. Returns (library, issue id,
/// archive path).
async fn scanned_issue(app: &TestApp, root: &Path, writeback: bool) -> (Uuid, String, PathBuf) {
    let series = root.join("Anchors");
    std::fs::create_dir_all(&series).unwrap();
    let path = series.join("Anchors 001.cbz");
    write_cbz(&path, &[RED, GREEN, BLUE, YELLOW], 0);
    let mut seed = LibrarySeed::new(root);
    if writeback {
        seed = seed.with_sidecar_writeback();
    }
    let lib = seed.insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib).await.unwrap();
    let row = issue::Entity::find()
        .filter(issue::Column::FilePath.eq(path.to_string_lossy().into_owned()))
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("scanned issue");
    assert_eq!(row.page_count, Some(4));
    (lib, row.id, path)
}

/// `kind → (page_index, tags, page_hash)`.
async fn markers(app: &TestApp, issue_id: &str) -> Vec<(String, i32, Vec<String>, Option<String>)> {
    let mut rows: Vec<_> = marker::Entity::find()
        .filter(marker::Column::IssueId.eq(issue_id))
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|m| (m.kind, m.page_index, m.tags, m.page_hash))
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

async fn insert_legacy_marker(app: &TestApp, user: Uuid, issue_id: &str, page: i32) {
    let row = issue::Entity::find_by_id(issue_id.to_owned())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let now = Utc::now().fixed_offset();
    marker::ActiveModel {
        id: Set(Uuid::now_v7()),
        user_id: Set(user),
        series_id: Set(row.series_id),
        issue_id: Set(issue_id.to_owned()),
        page_index: Set(page),
        kind: Set("favorite".into()),
        is_favorite: Set(false),
        tags: Set(vec![]),
        region: Set(None),
        selection: Set(None),
        body: Set(None),
        color: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        hidden_from_log: Set(false),
        page_hash: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

#[tokio::test]
async fn capture_records_the_page_image_hash() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let (_lib, issue_id, _path) = scanned_issue(&app, dir.path(), false).await;
    let auth = register(&app).await;

    create_marker(&app, &auth, &issue_id, 1, "bookmark").await;
    write_progress(&app, &auth, &issue_id, 2).await;

    assert_eq!(
        markers(&app, &issue_id).await,
        vec![("bookmark".into(), 1, vec![], Some(page_hash_of(GREEN)))]
    );
    let p = progress(&app, auth.user_id, &issue_id).await;
    assert_eq!(p.last_page, 2);
    assert_eq!(p.page_hash, Some(page_hash_of(BLUE)));

    // An implicit write behind the stored page keeps the furthest page
    // and its hash.
    write_progress(&app, &auth, &issue_id, 0).await;
    let p = progress(&app, auth.user_id, &issue_id).await;
    assert_eq!(p.last_page, 2);
    assert_eq!(p.page_hash, Some(page_hash_of(BLUE)));
}

/// The headline case: an external replacement with the same images in a
/// different order. Markers and the resume position follow their images.
#[tokio::test]
async fn replaced_archive_with_reordered_pages_keeps_anchors_on_their_images() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let (lib, issue_id, path) = scanned_issue(&app, dir.path(), false).await;
    let auth = register(&app).await;

    create_marker(&app, &auth, &issue_id, 1, "note").await; // GREEN
    create_marker(&app, &auth, &issue_id, 3, "bookmark").await; // YELLOW
    write_progress(&app, &auth, &issue_id, 2).await; // BLUE

    // [RED, GREEN, BLUE, YELLOW] → [BLUE, YELLOW, RED, GREEN].
    write_cbz(&path, &[BLUE, YELLOW, RED, GREEN], 60);
    scanner::scan_library(&app.state(), lib).await.unwrap();

    assert_eq!(
        markers(&app, &issue_id).await,
        vec![
            ("bookmark".into(), 1, vec![], Some(page_hash_of(YELLOW))),
            ("note".into(), 3, vec![], Some(page_hash_of(GREEN))),
        ]
    );
    let p = progress(&app, auth.user_id, &issue_id).await;
    assert_eq!(p.last_page, 0, "resume position follows BLUE to page 0");
    assert_eq!(p.page_hash, Some(page_hash_of(BLUE)));
    assert!(p.percent.abs() < 1e-9, "percent {}", p.percent);

    // The page server now serves the replaced archive (the scanner
    // dropped the cached handle), so the next capture reads new bytes.
    write_progress(&app, &auth, &issue_id, 1).await;
    let p = progress(&app, auth.user_id, &issue_id).await;
    assert_eq!(p.page_hash, Some(page_hash_of(YELLOW)));

    // A second rescan of unchanged bytes is a no-op.
    let before = markers(&app, &issue_id).await;
    scanner::scan_library(&app.state(), lib).await.unwrap();
    assert_eq!(markers(&app, &issue_id).await, before);
}

/// Anchors without a hash (written before WP-6.2) keep the WP-1.2
/// ordinal behaviour: identity for surviving ordinals, pulled onto the
/// last page with `page-removed` when the archive shrank. Hashed anchors
/// on the same issue still resolve by hash.
#[tokio::test]
async fn legacy_anchors_without_a_hash_use_the_ordinal_fallback() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let (lib, issue_id, path) = scanned_issue(&app, dir.path(), false).await;
    let auth = register(&app).await;

    insert_legacy_marker(&app, auth.user_id, &issue_id, 3).await;
    create_marker(&app, &auth, &issue_id, 2, "bookmark").await; // BLUE, hashed
    write_progress(&app, &auth, &issue_id, 1).await;
    // Make the progress row legacy (no hash).
    let mut am: progress_record::ActiveModel = progress(&app, auth.user_id, &issue_id).await.into();
    am.page_hash = Set(None);
    am.update(&app.state().db).await.unwrap();

    // [RED, GREEN, BLUE, YELLOW] → [BLUE, RED] (2 pages).
    write_cbz(&path, &[BLUE, RED], 60);
    scanner::scan_library(&app.state(), lib).await.unwrap();

    assert_eq!(
        markers(&app, &issue_id).await,
        vec![
            // Hashed: follows BLUE to page 0.
            ("bookmark".into(), 0, vec![], Some(page_hash_of(BLUE))),
            // Legacy on old page 3: past the new end → last page, tagged.
            ("favorite".into(), 1, vec![PAGE_REMOVED_TAG.into()], None),
        ]
    );
    // Legacy progress on page 1 survives the truncation by ordinal.
    let p = progress(&app, auth.user_id, &issue_id).await;
    assert_eq!(p.last_page, 1);
    assert_eq!(p.page_hash, None);
}

/// When the anchored image is gone from the replacement, the marker
/// keeps its ordinal (fallback) and gains the drift note; a later
/// replacement that restores the image re-resolves it and clears the tag.
#[tokio::test]
async fn vanished_image_gets_a_drift_note_and_recovers_when_restored() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let (lib, issue_id, path) = scanned_issue(&app, dir.path(), false).await;
    let auth = register(&app).await;

    create_marker(&app, &auth, &issue_id, 1, "bookmark").await; // GREEN
    write_progress(&app, &auth, &issue_id, 1).await; // GREEN

    // GREEN is replaced by PURPLE; everything else stays.
    write_cbz(&path, &[RED, PURPLE, BLUE, YELLOW], 60);
    scanner::scan_library(&app.state(), lib).await.unwrap();

    assert_eq!(
        markers(&app, &issue_id).await,
        vec![(
            "bookmark".into(),
            1,
            vec![PAGE_DRIFT_TAG.into()],
            Some(page_hash_of(GREEN)),
        )]
    );
    let p = progress(&app, auth.user_id, &issue_id).await;
    assert_eq!(p.last_page, 1, "progress falls back to its ordinal");

    // GREEN comes back, now at page 3.
    write_cbz(&path, &[RED, PURPLE, BLUE, GREEN], 120);
    scanner::scan_library(&app.state(), lib).await.unwrap();
    assert_eq!(
        markers(&app, &issue_id).await,
        vec![("bookmark".into(), 3, vec![], Some(page_hash_of(GREEN)))]
    );
    let p = progress(&app, auth.user_id, &issue_id).await;
    assert_eq!(p.last_page, 3);
}

/// Folio's own archive editor is authoritative about where pages went;
/// it re-stamps anchor hashes with the rewritten bytes, so a rotated
/// page's markers do not read as drifted on the follow-up rescan.
#[tokio::test]
async fn archive_edit_restamps_hashes_so_the_rescan_sees_no_drift() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let (lib, issue_id, path) = scanned_issue(&app, dir.path(), true).await;
    let auth = register(&app).await;

    create_marker(&app, &auth, &issue_id, 1, "bookmark").await; // GREEN
    insert_legacy_marker(&app, auth.user_id, &issue_id, 2).await; // BLUE, no hash
    write_progress(&app, &auth, &issue_id, 1).await;

    // Rotate GREEN, then move it to the front: [GREEN', RED, BLUE, YELLOW].
    let job = ArchiveEditJob {
        issue_id: issue_id.clone(),
        ops: vec![
            PageOp::Rotate {
                ordinal: 1,
                degrees: Rot::R90,
            },
            PageOp::Reorder {
                new_order: vec![1, 0, 2, 3],
            },
        ],
        bulk_op: None,
        actor_id: None,
        actor_ip: None,
        actor_ua: None,
        attempt: 0,
    };
    edit_one_issue(&app.state(), &job).await.unwrap();

    let rows = markers(&app, &issue_id).await;
    assert_eq!(rows[0].0, "bookmark");
    assert_eq!(rows[0].1, 0, "bookmark follows the edited page");
    let rotated = rows[0].3.clone().expect("re-stamped hash");
    assert_ne!(rotated, page_hash_of(GREEN), "rotation changed the bytes");
    // The legacy marker learned its hash from the exact edit map.
    assert_eq!(
        rows[1],
        ("favorite".into(), 2, vec![], Some(page_hash_of(BLUE)))
    );
    let p = progress(&app, auth.user_id, &issue_id).await;
    assert_eq!(
        (p.last_page, p.page_hash.clone()),
        (0, Some(rotated.clone()))
    );

    // The follow-up rescan (new bytes) resolves every anchor in place.
    let t = filetime::FileTime::from_unix_time(Utc::now().timestamp() + 60, 0);
    filetime::set_file_mtime(&path, t).unwrap();
    scanner::scan_library(&app.state(), lib).await.unwrap();
    assert_eq!(markers(&app, &issue_id).await, rows);
    assert_eq!(progress(&app, auth.user_id, &issue_id).await.last_page, 0);
}
