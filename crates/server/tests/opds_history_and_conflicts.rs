//! Integration tests for M5 of opds-sync-1.0 — read-history feed
//! (`/opds/v1/history`) + explicit-write conflict-resolution semantics.
//!
//! History tests:
//!  - `/opds/v1/history` returns ONLY issues with `finished = true`.
//!  - Entries are ordered by `progress_record.updated_at DESC` (most
//!    recently finished first).
//!  - ACL filter excludes issues in libraries the user can't see.
//!
//! Conflict tests:
//!  - Sequential explicit writes from different devices land
//!    last-writer-wins on `page`, `finished`, and `device` columns.
//!  - The response body echoes the SERVER's resolved row (not the
//!    request) — proves callers can trust the response as the
//!    authoritative post-write state.
//!  - `finished` is sticky on per-page writes that omit it: a mid-issue
//!    page write after a finished row preserves `finished = true`.

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
    library_user_access::ActiveModel as LibraryUserAccessAM,
    progress_record::ActiveModel as ProgressAM,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
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
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn grant_library_access(db: &DatabaseConnection, user_id: Uuid, lib_id: Uuid) {
    let now = Utc::now().fixed_offset();
    LibraryUserAccessAM {
        library_id: Set(lib_id),
        user_id: Set(user_id),
        role: Set("reader".into()),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
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
    finished: bool,
    when: chrono::DateTime<chrono::FixedOffset>,
) {
    ProgressAM {
        user_id: Set(user_id),
        issue_id: Set(issue_id.into()),
        last_page: Set(last_page),
        percent: Set(if finished { 1.0 } else { 0.5 }),
        finished: Set(finished),
        finished_at: Set(if finished { Some(when) } else { None }),
        updated_at: Set(when),
        device: Set(None),
        is_backfill: Set(false),
    }
    .insert(db)
    .await
    .unwrap();
}

// ───────────────────────── /opds/v1/history ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_returns_only_finished_issues() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "hist-finished@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "History").await;
    let done = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("done.cbz"),
        b"hist-done",
        20,
    )
    .await;
    let in_progress = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("ip.cbz"),
        b"hist-ip",
        20,
    )
    .await;
    let untouched = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("u.cbz"),
        b"hist-u",
        20,
    )
    .await;
    let when = Utc::now().fixed_offset();
    seed_progress(&db, auth.user_id, &done, 19, true, when).await;
    seed_progress(&db, auth.user_id, &in_progress, 7, false, when).await;
    // `untouched` has no progress row at all.

    let resp = get_cookie(&app, "/opds/v1/history", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains(&format!("urn:issue:{done}")),
        "finished issue appears: {body}"
    );
    assert!(
        !body.contains(&format!("urn:issue:{in_progress}")),
        "in-progress issue excluded"
    );
    assert!(
        !body.contains(&format!("urn:issue:{untouched}")),
        "untouched issue excluded"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_orders_newest_finish_first() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "hist-order@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Order").await;
    let older = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("older.cbz"),
        b"hist-older",
        20,
    )
    .await;
    let newer = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("newer.cbz"),
        b"hist-newer",
        20,
    )
    .await;
    let t_older = chrono::DateTime::parse_from_rfc3339("2026-03-01T12:00:00Z").unwrap();
    let t_newer = chrono::DateTime::parse_from_rfc3339("2026-05-15T12:00:00Z").unwrap();
    seed_progress(&db, auth.user_id, &older, 19, true, t_older).await;
    seed_progress(&db, auth.user_id, &newer, 19, true, t_newer).await;

    let resp = get_cookie(&app, "/opds/v1/history", &auth).await;
    let body = body_text(resp.into_body()).await;
    let pos_newer = body
        .find(&format!("urn:issue:{newer}"))
        .expect("newer issue present");
    let pos_older = body
        .find(&format!("urn:issue:{older}"))
        .expect("older issue present");
    assert!(
        pos_newer < pos_older,
        "newer-finish issue appears before older-finish issue"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_enforces_library_acl() {
    let app = TestApp::spawn().await;
    // First user is auto-admin; create a non-admin reader and grant
    // them access to ONE library while seeding a finished issue in a
    // SECOND library they can't see.
    let _admin = register(&app, "hist-admin@example.com").await;
    let reader = register(&app, "hist-reader@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let visible_root = tmp.path().join("visible-lib");
    let hidden_root = tmp.path().join("hidden-lib");
    std::fs::create_dir_all(&visible_root).unwrap();
    std::fs::create_dir_all(&hidden_root).unwrap();

    let visible_lib = seed_library(&db, &visible_root).await;
    let hidden_lib = seed_library(&db, &hidden_root).await;
    grant_library_access(&db, reader.user_id, visible_lib).await;

    let visible_series = seed_series(&db, visible_lib, "Visible").await;
    let hidden_series = seed_series(&db, hidden_lib, "Hidden").await;
    let visible_issue = seed_issue(
        &db,
        visible_lib,
        visible_series,
        &visible_root.join("v.cbz"),
        b"hist-v",
        20,
    )
    .await;
    let hidden_issue = seed_issue(
        &db,
        hidden_lib,
        hidden_series,
        &hidden_root.join("h.cbz"),
        b"hist-h",
        20,
    )
    .await;
    // Both are "finished" for the reader's user_id, but the reader
    // can't actually see the hidden library — the history feed must
    // filter the hidden one out even though the row exists.
    let when = Utc::now().fixed_offset();
    seed_progress(&db, reader.user_id, &visible_issue, 19, true, when).await;
    seed_progress(&db, reader.user_id, &hidden_issue, 19, true, when).await;

    let resp = get_cookie(&app, "/opds/v1/history", &reader).await;
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains(&format!("urn:issue:{visible_issue}")),
        "visible issue appears"
    );
    assert!(
        !body.contains(&format!("urn:issue:{hidden_issue}")),
        "hidden-library issue is filtered out"
    );
}

// ───────────────────────── conflict-resolution ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sequential_writes_from_different_devices_are_last_writer_wins() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "conflict-lww@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Conflict").await;
    let issue_id = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"conflict-a",
        50,
    )
    .await;
    let token = mint_progress_token(&app, &auth, "conflict-lww").await;

    // Tablet writes page=30
    let r1 = put_progress_bearer(
        &app,
        &token,
        &issue_id,
        serde_json::json!({"page": 30, "device": "Chunky/iPad"}),
    )
    .await;
    assert_eq!(r1.status(), StatusCode::OK);

    // Phone then writes page=10 (a backwards bookmark deep-link). The
    // server accepts the regression intentionally — explicit writes are
    // last-writer-wins so legitimate backwards moves (rewind, mark-
    // unread, jump-to-bookmark) work without an override.
    let r2 = put_progress_bearer(
        &app,
        &token,
        &issue_id,
        serde_json::json!({"page": 10, "device": "Panels/iPhone"}),
    )
    .await;
    assert_eq!(r2.status(), StatusCode::OK);

    // Inspect the stored row directly.
    use sea_orm::{ColumnTrait, QueryFilter};
    let row = entity::progress_record::Entity::find()
        .filter(entity::progress_record::Column::UserId.eq(auth.user_id))
        .filter(entity::progress_record::Column::IssueId.eq(issue_id.clone()))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.last_page, 10, "second write wins on last_page");
    assert_eq!(
        row.device.as_deref(),
        Some("Panels/iPhone"),
        "second write's device tag is preserved as the most-recent writer"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_response_echoes_server_resolved_row() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "conflict-resp@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Resp").await;
    let issue_id = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"conflict-resp",
        20,
    )
    .await;
    let token = mint_progress_token(&app, &auth, "conflict-resp").await;

    let resp = put_progress_bearer(
        &app,
        &token,
        &issue_id,
        serde_json::json!({"page": 11, "device": "Phone"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    // Response carries the resolved post-write row — issue_id, page,
    // position, finished, updated_at. percent + position are derived
    // from page/page_count so the client can confirm its conversion.
    assert_eq!(body["issue_id"], issue_id);
    assert_eq!(body["page"], 11);
    assert_eq!(body["finished"], false);
    let percent = body["percent"].as_f64().unwrap();
    let position = body["position"].as_f64().unwrap();
    assert!(
        (percent - 0.55).abs() < 0.0001,
        "percent = 11/20 = 0.55, got {percent}"
    );
    assert!(
        (position - percent).abs() < f64::EPSILON,
        "position aliases percent: {position} vs {percent}"
    );
    assert!(
        body["updated_at"].as_str().is_some(),
        "updated_at present in response"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finished_is_sticky_on_subsequent_per_page_writes() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "conflict-sticky@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Sticky").await;
    let issue_id = seed_issue(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"conflict-sticky",
        20,
    )
    .await;
    let token = mint_progress_token(&app, &auth, "conflict-sticky").await;

    // 1) Mark finished at last page.
    let r1 = put_progress_bearer(
        &app,
        &token,
        &issue_id,
        serde_json::json!({"page": 19, "finished": true, "device": "A"}),
    )
    .await;
    assert_eq!(r1.status(), StatusCode::OK);
    // 2) Subsequent mid-issue bookmark deep-link (page=5, no `finished`
    //    field). Sticky-finished semantics MUST preserve the row's
    //    finished=true — only an explicit `finished: false` clears it.
    let r2 = put_progress_bearer(
        &app,
        &token,
        &issue_id,
        serde_json::json!({"page": 5, "device": "B"}),
    )
    .await;
    assert_eq!(r2.status(), StatusCode::OK);
    let body = body_json(r2.into_body()).await;
    assert_eq!(body["page"], 5, "page advances to 5");
    assert_eq!(
        body["finished"], true,
        "finished stays true when caller omits the field"
    );
    // 3) Explicit clear works.
    let r3 = put_progress_bearer(
        &app,
        &token,
        &issue_id,
        serde_json::json!({"page": 5, "finished": false, "device": "B"}),
    )
    .await;
    assert_eq!(r3.status(), StatusCode::OK);
    let body3 = body_json(r3.into_body()).await;
    assert_eq!(
        body3["finished"], false,
        "explicit `finished: false` clears the flag"
    );
}
