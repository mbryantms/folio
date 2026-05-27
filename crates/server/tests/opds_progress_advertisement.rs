//! Integration tests for M4 of opds-sync-1.0 — write-back
//! advertisement + the `position` (fractional) request field on
//! `PUT /opds/v1/issues/{id}/progress`.
//!
//! Verifies:
//!  - `/opds/v1` root catalog advertises the sync rel with a templated href.
//!  - `/opds/v2` root catalog carries the same rel inside `links[]` with
//!    a `profile` URL.
//!  - The PUT handler accepts `{position: 0.5}` and resolves it to the
//!    integer page via `round(position * page_count)`.
//!  - A request with neither `page` nor `position` is rejected with 400.

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
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, Set};
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
    #[allow(dead_code)]
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

async fn mint_progress_token(app: &TestApp, auth: &Authed, label: &str) -> String {
    let body = format!(r#"{{"label":"{label}","scope":"read+progress"}}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/me/app-passwords")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookies())
                .header("x-csrf-token", &auth.csrf)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json: serde_json::Value = body_json(resp.into_body()).await;
    json["plaintext"].as_str().unwrap().to_owned()
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

async fn put_progress_bearer(
    app: &TestApp,
    token: &str,
    issue_id: &str,
    body: serde_json::Value,
) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/opds/v1/issues/{issue_id}/progress"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
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
        allow_archive_writeback: Set(false),
        metadata_writeback_enabled: Set(false),
        archive_backup_retain_count: Set(1),
        archive_backup_retain_days: Set(30),
        metadata_publisher_blacklist: Set(serde_json::json!([])),
        filename_ignore_leading_numbers: Set(false),
        filename_assume_issue_one: Set(false),
        metadata_auto_apply_strong_matches: Set(false),
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
        sort_name: Set(None),
        year_end: Set(None),
        series_type: Set(None),
        aliases: Set(serde_json::json!([])),
        deck: Set(None),
        publisher_id: Set(None),
        imprint_id: Set(None),
        last_metadata_sync_at: Set(None),
        metadata_sync_paused: Set(false),
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
        preserve_canonical_order: Set(false),
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
        deck: Set(None),
        store_date: Set(None),
        foc_date: Set(None),
        price: Set(None),
        sku: Set(None),
        staff_rating: Set(None),
        aliases: Set(serde_json::json!([])),
        last_metadata_sync_at: Set(None),
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
        last_rewrite_at: Set(None),
        last_rewrite_kind: Set(None),
        cover_page_index: Set(0),
    }
    .insert(db)
    .await
    .unwrap();
    hash
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v1_root_advertises_progress_sync_link() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "m4-v1-root@example.com").await;
    let resp = get_cookie(&app, "/opds/v1", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains(r#"rel="http://opds-spec.org/sync""#),
        "v1 root carries the sync rel: {body}"
    );
    assert!(
        body.contains(r#"href="/opds/v1/issues/{issue_id}/progress""#),
        "templated href on the sync link: {body}"
    );
    assert!(
        body.contains(r#"type="application/json""#),
        "sync link advertises JSON wire format: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_root_advertises_progress_sync_link_with_profile() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "m4-v2-root@example.com").await;
    let resp = get_cookie(&app, "/opds/v2", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let links = body["links"].as_array().expect("root links[] present");
    let sync = links
        .iter()
        .find(|l| l["rel"] == "http://opds-spec.org/sync")
        .expect("v2 root has sync rel");
    assert_eq!(
        sync["href"], "/opds/v1/issues/{issue_id}/progress",
        "v2 sync href is the same templated URL as v1"
    );
    assert_eq!(sync["templated"], true);
    assert_eq!(
        sync["profile"], "https://folio.bryhome.live/spec/progress-write-v1",
        "profile anchors the documented wire format"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_accepts_position_field_and_resolves_to_page() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "m4-position@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Pos").await;
    let issue_id = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"m4-pos",
        32,
    )
    .await;
    let token = mint_progress_token(&app, &auth, "m4-pos").await;

    // position=0.5 with page_count=32 → round(16.0) = 16
    let resp = put_progress_bearer(
        &app,
        &token,
        &issue_id,
        serde_json::json!({ "position": 0.5 }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["page"], 16, "position 0.5 of 32 pages → page 16");
    let pos = body["position"].as_f64().unwrap();
    assert!(
        (pos - 0.5).abs() < 0.0001,
        "response carries position alias: {pos}"
    );
    let pct = body["percent"].as_f64().unwrap();
    assert!(
        (pct - 0.5).abs() < 0.0001,
        "percent matches position: {pct}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_rejects_missing_both_page_and_position() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "m4-empty@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Empty").await;
    let issue_id = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("e.cbz"),
        b"m4-empty",
        10,
    )
    .await;
    let token = mint_progress_token(&app, &auth, "m4-empty").await;

    // No page, no position — neither offered. Server rejects 400.
    let resp = put_progress_bearer(
        &app,
        &token,
        &issue_id,
        serde_json::json!({ "device": "test" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "validation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_position_with_unknown_page_count_is_rejected() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "m4-no-count@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "NoCount").await;
    // page_count=0 → server can't compute round(position * 0). 400.
    let issue_id = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("nc.cbz"),
        b"m4-no-count",
        0,
    )
    .await;
    let token = mint_progress_token(&app, &auth, "m4-no-count").await;

    let resp = put_progress_bearer(
        &app,
        &token,
        &issue_id,
        serde_json::json!({ "position": 0.5 }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(resp.into_body()).await;
    let msg = body["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("page_count"),
        "rejection message names the missing precondition: {msg}"
    );
}
