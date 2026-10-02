//! Roadmap WP-8.4 — lazy `page_hash` backfill for anchors written before
//! WP-6.2, and `page_hash` in the account export.
//!
//! Legacy markers and progress rows have no page hash. When the page
//! server next opens the issue's archive (or the reader loads the issue's
//! markers), a background task hashes the referenced pages — at most
//! `MAX_PAGES_PER_OPEN` per open — and stamps the unhashed rows without
//! bumping `updated_at`. There is no library-wide hashing job.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use common::TestApp;
use common::seed::LibrarySeed;
use entity::{issue, marker, progress_record};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::library::scanner;
use server::reading::page_hash_backfill::{MAX_PAGES_PER_OPEN, backfill_issue};
use server::reading::page_remap::PAGE_REMOVED_TAG;
use std::io::{Cursor, Write};
use std::path::Path;
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

fn png(rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(6, 4, image::Rgb(rgb));
    let mut buf = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

/// A distinct colour per page ordinal.
fn colour(i: usize) -> [u8; 3] {
    [(i * 5 % 256) as u8, 100, (255 - i) as u8]
}

fn hash_of_page(i: usize) -> String {
    blake3::hash(&png(colour(i))).to_hex().to_string()
}

fn write_cbz(path: &Path, pages: usize) {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for i in 0..pages {
            zw.start_file(format!("page-{:03}.png", i + 1), opts)
                .unwrap();
            zw.write_all(&png(colour(i))).unwrap();
        }
        zw.finish().unwrap();
    }
    std::fs::write(path, buf.into_inner()).unwrap();
}

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

impl Authed {
    fn cookies(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
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
                    r#"{"email":"backfill@example.com","password":"correctly-horse-battery"}"#,
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

async fn get(app: &TestApp, auth: &Authed, uri: &str) -> (StatusCode, Vec<u8>) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::COOKIE, auth.cookies())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, bytes.to_vec())
}

/// One scanned `pages`-page issue. Returns the issue row.
async fn scanned_issue(app: &TestApp, root: &Path, pages: usize) -> issue::Model {
    let series = root.join("Legacy");
    std::fs::create_dir_all(&series).unwrap();
    let path = series.join("Legacy 001.cbz");
    write_cbz(&path, pages);
    let lib = LibrarySeed::new(root).insert(&app.state().db).await;
    scanner::scan_library(&app.state(), lib).await.unwrap();
    let row = issue::Entity::find()
        .filter(issue::Column::FilePath.eq(path.to_string_lossy().into_owned()))
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("scanned issue");
    assert_eq!(row.page_count, Some(pages as i32));
    // The scan opened the archive through its own reader, not the page
    // server's cache; drop any cached handle so the first page request
    // is a real open.
    app.state().zip_lru.invalidate(&row.id);
    row
}

/// A pre-WP-6.2 marker: no page hash, `updated_at` an hour ago.
async fn legacy_marker(app: &TestApp, user: Uuid, row: &issue::Model, page: i32) -> Uuid {
    let then = (Utc::now() - Duration::hours(1)).fixed_offset();
    let id = Uuid::now_v7();
    marker::ActiveModel {
        id: Set(id),
        user_id: Set(user),
        series_id: Set(row.series_id),
        issue_id: Set(row.id.clone()),
        page_index: Set(page),
        kind: Set("bookmark".into()),
        is_favorite: Set(false),
        tags: Set(vec![]),
        region: Set(None),
        selection: Set(None),
        body: Set(None),
        color: Set(None),
        created_at: Set(then),
        updated_at: Set(then),
        hidden_from_log: Set(false),
        page_hash: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
    id
}

async fn legacy_progress(app: &TestApp, user: Uuid, row: &issue::Model, page: i32) {
    let then = (Utc::now() - Duration::hours(1)).fixed_offset();
    progress_record::ActiveModel {
        user_id: Set(user),
        issue_id: Set(row.id.clone()),
        last_page: Set(page),
        percent: Set(0.25),
        finished: Set(false),
        updated_at: Set(then),
        device: Set(None),
        finished_at: Set(None),
        is_backfill: Set(false),
        run: Set(0),
        page_hash: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

async fn marker_row(app: &TestApp, id: Uuid) -> marker::Model {
    marker::Entity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
}

async fn progress_row(app: &TestApp, user: Uuid, issue_id: &str) -> progress_record::Model {
    progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user))
        .filter(progress_record::Column::IssueId.eq(issue_id))
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backfill_stamps_legacy_anchors_with_the_page_they_point_at() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let row = scanned_issue(&app, dir.path(), 4).await;
    let auth = register(&app).await;
    let m1 = legacy_marker(&app, auth.user_id, &row, 1).await;
    let m3 = legacy_marker(&app, auth.user_id, &row, 3).await;
    legacy_progress(&app, auth.user_id, &row, 2).await;
    // A marker an archive edit moved off a removed page sits on a guessed
    // neighbour: it must stay unhashed.
    let moved = legacy_marker(&app, auth.user_id, &row, 0).await;
    let mut am: marker::ActiveModel = marker_row(&app, moved).await.into();
    am.tags = Set(vec![PAGE_REMOVED_TAG.to_owned()]);
    am.update(&app.state().db).await.unwrap();
    let before = marker_row(&app, m1).await.updated_at;

    let out = backfill_issue(&app.state(), &row).await.unwrap();
    assert_eq!(out.pages_hashed, 3);
    assert_eq!(out.markers_stamped, 2);
    assert_eq!(out.progress_stamped, 1);

    let a = marker_row(&app, m1).await;
    assert_eq!(a.page_hash.as_deref(), Some(hash_of_page(1).as_str()));
    // A hash is not a user-visible change: no sync wake-up.
    assert_eq!(a.updated_at, before);
    let b = marker_row(&app, m3).await;
    assert_eq!(b.page_hash.as_deref(), Some(hash_of_page(3).as_str()));
    let p = progress_row(&app, auth.user_id, &row.id).await;
    assert_eq!(p.page_hash.as_deref(), Some(hash_of_page(2).as_str()));
    assert_eq!(marker_row(&app, moved).await.page_hash, None);

    // Nothing left: a second pass is a no-op.
    let again = backfill_issue(&app.state(), &row).await.unwrap();
    assert_eq!(again.pages_hashed, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backfill_is_bounded_per_open() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let pages = MAX_PAGES_PER_OPEN + 8;
    let row = scanned_issue(&app, dir.path(), pages).await;
    let auth = register(&app).await;
    for p in 0..pages {
        legacy_marker(&app, auth.user_id, &row, p as i32).await;
    }
    let first = backfill_issue(&app.state(), &row).await.unwrap();
    assert_eq!(first.pages_hashed, MAX_PAGES_PER_OPEN);
    let second = backfill_issue(&app.state(), &row).await.unwrap();
    assert_eq!(second.pages_hashed, 8);
    let missing = marker::Entity::find()
        .filter(marker::Column::IssueId.eq(row.id.clone()))
        .filter(marker::Column::PageHash.is_null())
        .all(&app.state().db)
        .await
        .unwrap();
    assert!(missing.is_empty());
}

/// Poll until the background backfill has stamped `id` (it runs off the
/// request path, so the response returns first).
async fn wait_for_hash(app: &TestApp, id: Uuid) -> String {
    for _ in 0..200 {
        if let Some(h) = marker_row(app, id).await.page_hash {
            return h;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("marker {id} never gained a page hash");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_the_archive_triggers_the_backfill() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let row = scanned_issue(&app, dir.path(), 4).await;
    let auth = register(&app).await;
    let m = legacy_marker(&app, auth.user_id, &row, 2).await;
    legacy_progress(&app, auth.user_id, &row, 1).await;

    // The page server opens the archive on a cache miss…
    let (status, _) = get(&app, &auth, &format!("/issues/{}/pages/0", row.id)).await;
    assert_eq!(status, StatusCode::OK);
    // …and the backfill runs in the background.
    assert_eq!(wait_for_hash(&app, m).await, hash_of_page(2));
    for _ in 0..200 {
        if progress_row(&app, auth.user_id, &row.id)
            .await
            .page_hash
            .is_some()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let p = progress_row(&app, auth.user_id, &row.id).await;
    assert_eq!(p.page_hash.as_deref(), Some(hash_of_page(1).as_str()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loading_the_readers_markers_triggers_the_backfill() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let row = scanned_issue(&app, dir.path(), 4).await;
    let auth = register(&app).await;
    let m = legacy_marker(&app, auth.user_id, &row, 3).await;

    let (status, _) = get(&app, &auth, &format!("/api/me/issues/{}/markers", row.id)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(wait_for_hash(&app, m).await, hash_of_page(3));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_export_includes_page_hashes() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let row = scanned_issue(&app, dir.path(), 4).await;
    let auth = register(&app).await;
    legacy_marker(&app, auth.user_id, &row, 1).await;
    legacy_progress(&app, auth.user_id, &row, 2).await;
    backfill_issue(&app.state(), &row).await.unwrap();

    let (status, bytes) = get(&app, &auth, "/api/me/export").await;
    assert_eq!(status, StatusCode::OK);
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let sections = &doc["sections"];
    assert_eq!(sections["markers"][0]["page_hash"], hash_of_page(1));
    assert_eq!(sections["progress"][0]["page_hash"], hash_of_page(2));
}
