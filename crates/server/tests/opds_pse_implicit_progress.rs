//! Integration tests for M3 of opds-sync-1.0 — implicit progress
//! writes from OPDS-PSE page-stream hits.
//!
//! Verifies:
//!  - A single stream hit creates / advances a `progress_record` row.
//!  - Hitting the LAST page (`n == page_count - 1`) sets
//!    `finished = true`.
//!  - Concurrent / prefetch-style backwards hits don't regress
//!    `last_page` — only monotonic advances are recorded.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library,
    progress_record::ActiveModel as ProgressAM,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, Set};
use std::io::Write;
use tower::ServiceExt;
use uuid::Uuid;

const PNG_HEADER: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

fn page_payload(seed: u8) -> Vec<u8> {
    let mut v = PNG_HEADER.to_vec();
    while v.len() < 256 {
        v.push(seed.wrapping_add(v.len() as u8));
    }
    v
}

/// Build a CBZ with `n` distinct pages so the PSE handler sees a real
/// archive with the expected page count.
fn build_multipage_cbz(path: &std::path::Path, n: usize) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for i in 0..n {
        zw.start_file(format!("page-{i:03}.png"), opts).unwrap();
        zw.write_all(&page_payload(i as u8)).unwrap();
    }
    zw.finish().unwrap();
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

fn extract_cookie(resp: &axum::http::Response<Body>, name: &str) -> String {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|s| {
            let prefix = format!("{name}=");
            s.split(';')
                .next()
                .and_then(|kv| kv.strip_prefix(&prefix))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("expected cookie {name}"))
}

async fn body_bytes(b: Body) -> Vec<u8> {
    to_bytes(b, usize::MAX).await.unwrap().to_vec()
}

async fn register(app: &TestApp, email: &str) -> Authed {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let session = extract_cookie(&resp, "__Host-comic_session");
    let csrf = extract_cookie(&resp, "__Host-comic_csrf");
    let json: serde_json::Value =
        serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session,
        csrf,
        user_id,
    }
}

async fn promote_to_admin(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = entity::user::Entity::find_by_id(user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::user::ActiveModel = user.into();
    am.role = Set("admin".into());
    am.update(&db).await.unwrap();
}

async fn seed_issue(
    app: &TestApp,
    cbz_path: &std::path::Path,
    page_count: i32,
) -> (Uuid, Uuid, String) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("Test Library".into()),
        root_path: Set(cbz_path.parent().unwrap().to_string_lossy().into_owned()),
        default_language: Set("en".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(lib_id.to_string()),
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
    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set("Series".into()),
        normalized_name: Set(normalize_name("Series")),
        year: Set(None),
        volume: Set(None),
        publisher: Set(None),
        imprint: Set(None),
        status: Set("continuing".into()),
        total_issues: Set(None),
        age_rating: Set(None),
        summary: Set(None),
        language_code: Set("en".into()),
        comicvine_id: Set(None),
        metron_id: Set(None),
        gtin: Set(None),
        series_group: Set(None),
        slug: Set(series_id.to_string()),
        alternate_names: Set(serde_json::json!([])),
        created_at: Set(now),
        updated_at: Set(now),
        folder_path: Set(None),
        last_scanned_at: Set(None),
        match_key: Set(None),
        removed_at: Set(None),
        removal_confirmed_at: Set(None),
        status_user_set_at: Set(None),
        reading_direction: Set(None),
        preserve_canonical_order: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();
    let bytes = std::fs::read(cbz_path).unwrap();
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let size = std::fs::metadata(cbz_path).unwrap().len() as i64;
    IssueAM {
        id: Set(hash.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(Uuid::now_v7().to_string()),
        file_path: Set(cbz_path.to_string_lossy().into_owned()),
        file_size: Set(size),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(hash.clone()),
        title: Set(Some("Issue".into())),
        sort_number: Set(Some(1.0)),
        number_raw: Set(Some("1".into())),
        volume: Set(None),
        year: Set(None),
        month: Set(None),
        day: Set(None),
        summary: Set(None),
        notes: Set(None),
        language_code: Set(None),
        format: Set(None),
        black_and_white: Set(None),
        manga: Set(None),
        age_rating: Set(None),
        page_count: Set(Some(page_count)),
        pages: Set(serde_json::json!([])),
        comic_info_raw: Set(serde_json::json!({})),
        alternate_series: Set(None),
        story_arc: Set(None),
        story_arc_number: Set(None),
        characters: Set(None),
        teams: Set(None),
        locations: Set(None),
        tags: Set(None),
        genre: Set(None),
        writer: Set(None),
        penciller: Set(None),
        inker: Set(None),
        colorist: Set(None),
        letterer: Set(None),
        cover_artist: Set(None),
        editor: Set(None),
        translator: Set(None),
        publisher: Set(None),
        imprint: Set(None),
        scan_information: Set(None),
        community_rating: Set(None),
        review: Set(None),
        web_url: Set(None),
        comicvine_id: Set(None),
        metron_id: Set(None),
        gtin: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        removed_at: Set(None),
        removal_confirmed_at: Set(None),
        superseded_by: Set(None),
        special_type: Set(None),
        hash_algorithm: Set(1),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        additional_links: Set(serde_json::json!([])),
        user_edited: Set(serde_json::json!([])),
        comicinfo_count: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    (lib_id, series_id, hash)
}

/// Pull the signed PSE query string out of a real feed render — same
/// pattern as `tests/opds_pse.rs` — so test tampering can't sneak in
/// via a hand-rolled signature.
async fn fetch_pse_query(app: &TestApp, auth: &Authed, series_id: Uuid) -> String {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/opds/v1/series/{series_id}"))
                .header(header::COOKIE, auth.cookies())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = String::from_utf8(body_bytes(resp.into_body()).await).unwrap();
    let stream_idx = body
        .find(r#"rel="http://vaemendis.net/opds-pse/stream""#)
        .expect("pse stream link present");
    let href_start = body[stream_idx..]
        .find("href=\"")
        .map(|i| stream_idx + i + "href=\"".len())
        .unwrap();
    let href_end = body[href_start..]
        .find('"')
        .map(|i| href_start + i)
        .unwrap();
    let href = &body[href_start..href_end];
    let query_start = href.find('?').map(|i| i + 1).unwrap();
    href[query_start..].replace("&amp;", "&")
}

async fn pse_hit(app: &TestApp, issue_id: &str, n: u32, query: &str) -> StatusCode {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/opds/pse/{issue_id}/{n}?{query}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

/// Poll the progress_record row until it exists / matches the expected
/// (last_page, finished) tuple. The PSE handler fires the upsert
/// fire-and-forget so the request can return before the row lands; a
/// short poll loop is reliable without making tests flake on slow CI.
async fn wait_for_progress(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
    issue_id: &str,
    expect_last_page: i32,
    expect_finished: bool,
) -> entity::progress_record::Model {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let row = entity::progress_record::Entity::find()
            .filter(entity::progress_record::Column::UserId.eq(user_id))
            .filter(entity::progress_record::Column::IssueId.eq(issue_id.to_owned()))
            .one(db)
            .await
            .unwrap();
        if let Some(r) = row.as_ref()
            && r.last_page == expect_last_page
            && r.finished == expect_finished
        {
            return r.clone();
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "progress row never matched (last_page={expect_last_page}, finished={expect_finished}); got {row:?}"
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_pse_hit_advances_progress() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "pse-implicit-single@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("multi.cbz");
    build_multipage_cbz(&cbz, 10);
    let (_lib, series_id, issue_id) = seed_issue(&app, &cbz, 10).await;
    let query = fetch_pse_query(&app, &auth, series_id).await;

    let status = pse_hit(&app, &issue_id, 4, &query).await;
    assert_eq!(status, StatusCode::OK);

    let db = Database::connect(&app.db_url).await.unwrap();
    let row = wait_for_progress(&db, auth.user_id, &issue_id, 4, false).await;
    assert_eq!(
        row.device.as_deref(),
        Some("opds-pse"),
        "device tag identifies the implicit-write origin"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pse_hit_on_last_page_marks_finished() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "pse-implicit-finished@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("last.cbz");
    build_multipage_cbz(&cbz, 5);
    let (_lib, series_id, issue_id) = seed_issue(&app, &cbz, 5).await;
    let query = fetch_pse_query(&app, &auth, series_id).await;

    // page_count=5; last 0-indexed page is 4.
    let status = pse_hit(&app, &issue_id, 4, &query).await;
    assert_eq!(status, StatusCode::OK);

    let db = Database::connect(&app.db_url).await.unwrap();
    let row = wait_for_progress(&db, auth.user_id, &issue_id, 4, true).await;
    assert!(row.finished, "last-page hit flips finished=true");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pse_backwards_hit_does_not_regress_recorded_progress() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "pse-implicit-monotonic@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let dir = tempfile::tempdir().unwrap();
    let cbz = dir.path().join("mono.cbz");
    build_multipage_cbz(&cbz, 10);
    let (_lib, series_id, issue_id) = seed_issue(&app, &cbz, 10).await;

    // Pre-seed an explicit progress row at page 7 (e.g. from a prior
    // explicit PUT or from a different device). Subsequent PSE hits at
    // pages < 7 must NOT regress the stored last_page.
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    ProgressAM {
        user_id: Set(auth.user_id),
        issue_id: Set(issue_id.clone()),
        last_page: Set(7),
        percent: Set(0.8),
        finished: Set(false),
        updated_at: Set(now),
        device: Set(Some("web-reader".into())),
    }
    .insert(&db)
    .await
    .unwrap();

    let query = fetch_pse_query(&app, &auth, series_id).await;
    // KOReader-style prefetch hits at pages 2 + 3 (well behind page 7).
    assert_eq!(pse_hit(&app, &issue_id, 2, &query).await, StatusCode::OK);
    assert_eq!(pse_hit(&app, &issue_id, 3, &query).await, StatusCode::OK);
    // Then a real advance to page 8 must take.
    assert_eq!(pse_hit(&app, &issue_id, 8, &query).await, StatusCode::OK);

    // Allow the fire-and-forget tasks to land.
    let row = wait_for_progress(&db, auth.user_id, &issue_id, 8, false).await;
    assert_eq!(row.last_page, 8, "monotonic advance recorded");
    assert_eq!(
        row.device.as_deref(),
        Some("opds-pse"),
        "device tag flipped to opds-pse on the forward write"
    );
}
