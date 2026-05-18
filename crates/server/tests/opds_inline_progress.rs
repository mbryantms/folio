//! Integration tests for M1 of opds-sync-1.0 — inline read-state on
//! OPDS feed entries.
//!
//! Verifies that every issue entry across the v1 and v2 feeds carries:
//!  - `pse:lastRead` + `pse:lastReadDate` (Atom) when a progress row exists
//!  - `metadata.position` (OPDS 2.0) when a progress row exists
//!  - no annotation at all when the user has no progress row
//!  - per-user fan-out across a multi-issue feed renders each entry's
//!    state independently
//!
//! The five tests below cover (1) progress-present in v1, (2) progress-absent
//! in v1, (3) finished issue, (4) multi-issue feed with mixed states, and
//! (5) v2 `metadata.position` shape.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library,
    progress_record::ActiveModel as ProgressAM,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, Set};
use tower::ServiceExt;
use uuid::Uuid;

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

fn extract_cookie(resp: &Response<Body>, name: &str) -> String {
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

async fn body_text(b: Body) -> String {
    String::from_utf8(body_bytes(b).await).unwrap()
}

async fn body_json(b: Body) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(b).await).unwrap()
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
    let json: serde_json::Value = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session,
        csrf,
        user_id,
    }
}

async fn get_cookie(app: &TestApp, uri: &str, auth: &Authed) -> Response<Body> {
    app.router
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
        .unwrap()
}

async fn seed_library(db: &DatabaseConnection, root: &std::path::Path) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(id),
        name: Set(format!("Lib {}", &id.to_string()[..8])),
        root_path: Set(root.to_string_lossy().into_owned()),
        default_language: Set("en".into()),
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
    .insert(db)
    .await
    .unwrap();
    id
}

async fn seed_series(db: &DatabaseConnection, lib_id: Uuid, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    SeriesAM {
        id: Set(id),
        library_id: Set(lib_id),
        name: Set(name.into()),
        normalized_name: Set(normalize_name(name)),
        year: Set(Some(2020)),
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
        slug: Set(id.to_string()),
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
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn seed_issue(
    db: &DatabaseConnection,
    lib_id: Uuid,
    series_id: Uuid,
    file_path: &std::path::Path,
    payload: &[u8],
    page_count: i32,
    sort_number: f64,
) -> String {
    std::fs::write(file_path, payload).unwrap();
    let bytes = std::fs::read(file_path).unwrap();
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let now = Utc::now().fixed_offset();
    IssueAM {
        id: Set(hash.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(Uuid::now_v7().to_string()),
        file_path: Set(file_path.to_string_lossy().into_owned()),
        file_size: Set(std::fs::metadata(file_path).unwrap().len() as i64),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(hash.clone()),
        title: Set(Some(format!("Issue {sort_number}"))),
        sort_number: Set(Some(sort_number)),
        number_raw: Set(Some(format!("{sort_number}"))),
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
    .insert(db)
    .await
    .unwrap();
    hash
}

async fn seed_progress(
    db: &DatabaseConnection,
    user_id: Uuid,
    issue_id: &str,
    last_page: i32,
    percent: f64,
    finished: bool,
) {
    let now = Utc::now().fixed_offset();
    ProgressAM {
        user_id: Set(user_id),
        issue_id: Set(issue_id.into()),
        last_page: Set(last_page),
        percent: Set(percent),
        finished: Set(finished),
        updated_at: Set(now),
        device: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
}

/// Extract the substring between the entry's `urn:issue:<id>` id line and
/// its closing `</entry>` tag. Used to scope assertions to a single
/// entry in multi-entry feed responses.
fn entry_block<'a>(body: &'a str, issue_id: &str) -> &'a str {
    let needle = format!("urn:issue:{issue_id}");
    let start = body.find(&needle).expect("entry present");
    // Walk back to `<entry>` boundary
    let entry_start = body[..start].rfind("<entry>").expect("entry open");
    let entry_end = body[entry_start..].find("</entry>").expect("entry close") + entry_start;
    &body[entry_start..entry_end]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_entry_carries_pse_last_read_when_progress_present() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v1-progress-present@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Saga").await;
    let issue_id = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"saga-01",
        32,
        1.0,
    )
    .await;
    seed_progress(&db, auth.user_id, &issue_id, 14, 0.4375, false).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series_id}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let block = entry_block(&body, &issue_id);
    assert!(
        block.contains("<pse:lastRead>14</pse:lastRead>"),
        "lastRead present: {block}"
    );
    assert!(
        block.contains("<pse:lastReadDate>"),
        "lastReadDate present: {block}"
    );
    // The feed-root namespace must already be declared; assert it so a future
    // refactor that drops the declaration trips this test instead of shipping
    // a broken feed.
    assert!(
        body.contains(r#"xmlns:pse="http://vaemendis.net/opds-pse/ns""#),
        "feed declares pse namespace"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_entry_omits_pse_last_read_when_progress_absent() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v1-progress-absent@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Unread").await;
    let issue_id = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"unread-01",
        24,
        1.0,
    )
    .await;
    // No progress seeded.

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series_id}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let block = entry_block(&body, &issue_id);
    assert!(
        !block.contains("<pse:lastRead>"),
        "lastRead absent when no progress row: {block}"
    );
    assert!(
        !block.contains("<pse:lastReadDate>"),
        "lastReadDate absent when no progress row: {block}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_finished_issue_emits_last_page() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v1-finished@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Done").await;
    let issue_id = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"done-01",
        20,
        1.0,
    )
    .await;
    // page=19 (last 0-based), finished=true, percent=1.0
    seed_progress(&db, auth.user_id, &issue_id, 19, 1.0, true).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series_id}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    let block = entry_block(&body, &issue_id);
    assert!(
        block.contains("<pse:lastRead>19</pse:lastRead>"),
        "finished issue emits last_page=19: {block}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_multi_issue_feed_renders_mixed_states() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v1-multi@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Mixed").await;
    let a = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"mixed-a",
        30,
        1.0,
    )
    .await;
    let b = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("b.cbz"),
        b"mixed-b",
        30,
        2.0,
    )
    .await;
    let c = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("c.cbz"),
        b"mixed-c",
        30,
        3.0,
    )
    .await;
    // a: in progress (page 5); b: untouched; c: finished
    seed_progress(&db, auth.user_id, &a, 5, 0.1666, false).await;
    seed_progress(&db, auth.user_id, &c, 29, 1.0, true).await;

    let resp = get_cookie(&app, &format!("/opds/v1/series/{series_id}"), &auth).await;
    let body = body_text(resp.into_body()).await;
    let block_a = entry_block(&body, &a);
    let block_b = entry_block(&body, &b);
    let block_c = entry_block(&body, &c);
    assert!(
        block_a.contains("<pse:lastRead>5</pse:lastRead>"),
        "a in-progress: {block_a}"
    );
    assert!(
        !block_b.contains("<pse:lastRead>"),
        "b untouched, no annotation: {block_b}"
    );
    assert!(
        block_c.contains("<pse:lastRead>29</pse:lastRead>"),
        "c finished: {block_c}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_publication_carries_metadata_position() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2-position@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Readium").await;
    let issue_id = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"readium-01",
        32,
        1.0,
    )
    .await;
    // last_page=14, percent=0.4375 → totalProgression=0.4375, position=15
    seed_progress(&db, auth.user_id, &issue_id, 14, 0.4375, false).await;

    let resp = get_cookie(&app, &format!("/opds/v2/series/{series_id}"), &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let publications = body["publications"].as_array().expect("publications array");
    let pub_ = publications
        .iter()
        .find(|p| {
            p["metadata"]["identifier"]
                .as_str()
                .map(|s| s.contains(&issue_id))
                .unwrap_or(false)
        })
        .expect("publication for seeded issue");
    let pos = &pub_["metadata"]["position"];
    assert_eq!(pos["position"], 15, "position = last_page + 1");
    assert!(
        (pos["totalProgression"].as_f64().unwrap() - 0.4375).abs() < 0.0001,
        "totalProgression matches stored percent: {pos:?}",
    );
    assert_eq!(pos["finished"], false);
    assert_eq!(pos["totalPages"], 32);
    assert!(pos["modified"].is_string(), "modified timestamp present");
}
