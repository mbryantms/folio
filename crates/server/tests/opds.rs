//! Integration tests for the OPDS 1.x surface (M1 + M2).
//!
//! M1: navigation shape, paginated series list, ACL leak guard, search
//! shape, per-extension MIME types, Bearer (app-password) auth, HTTP Basic
//! auth carrying an app-password, the JWT-via-Basic footgun guard, and the
//! `WWW-Authenticate: Basic` challenge on bare 401s.
//!
//! M2: first/previous/next/last pagination link rels, paginated per-series
//! feed, Range/206 support + 416 on malformed Range, OpenSearch description
//! document, audit-log row per download.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use base64::Engine;
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use tower::ServiceExt;
use uuid::Uuid;

// ─────────────────────────── auth + http helpers ───────────────────────────

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

/// Issue an app-password for the authenticated user and return the plaintext.
async fn mint_app_password(app: &TestApp, auth: &Authed, label: &str) -> String {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/me/app-passwords")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookies())
                .header("x-csrf-token", &auth.csrf)
                .body(Body::from(format!(r#"{{"label":"{label}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json: serde_json::Value =
        serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
    json["plaintext"].as_str().unwrap().to_owned()
}

async fn get_with_auth(app: &TestApp, uri: &str, auth: Header<'_>) -> Response<Body> {
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    match auth {
        Header::Cookie(c) => builder = builder.header(header::COOKIE, c),
        Header::Bearer(t) => builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}")),
        Header::Basic(b) => builder = builder.header(header::AUTHORIZATION, format!("Basic {b}")),
        Header::None => {}
    }
    app.router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

enum Header<'a> {
    Cookie(String),
    Bearer(&'a str),
    Basic(String),
    None,
}

// ─────────────────────────── fixture helpers ───────────────────────────

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
    }
    .insert(db)
    .await
    .unwrap();
    id
}

/// Seed an issue row pointing at `file_path`. The caller supplies the
/// payload so each fixture file produces a distinct BLAKE3 hash (used as
/// the issue's primary key).
async fn seed_issue_with_file(
    db: &DatabaseConnection,
    lib_id: Uuid,
    series_id: Uuid,
    file_path: &std::path::Path,
    payload: &[u8],
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
        page_count: Set(Some(1)),
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

// ─────────────────────────── tests ───────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_navigation_shape() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "root-nav@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    let resp = get_with_auth(&app, "/opds/v1", Header::Cookie(auth.cookies())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
    assert!(
        ct.to_str().unwrap().starts_with("application/atom+xml"),
        "atom content-type, got {ct:?}"
    );
    let body = body_text(resp.into_body()).await;
    assert!(body.contains(r#"href="/opds/v1/series""#));
    assert!(body.contains(r#"href="/opds/v1/recent""#));
    assert!(body.contains(r#"rel="search""#));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_list_paginates() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "page@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    for i in 0..60 {
        seed_series(&db, lib_id, &format!("Series {i:03}")).await;
    }

    let resp = get_with_auth(&app, "/opds/v1/series", Header::Cookie(auth.cookies())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let entry_count = body.matches("<entry>").count();
    assert_eq!(entry_count, 50, "page 1 returns 50 entries");
    assert!(
        body.contains(r#"rel="next""#) && body.contains(r#"page=2"#),
        "page 1 includes next link"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_detail_acl() {
    let app = TestApp::spawn().await;
    // First user becomes admin automatically.
    let _admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Forbidden").await;

    let resp = get_with_auth(
        &app,
        &format!("/opds/v1/series/{series_id}"),
        Header::Cookie(reader.cookies()),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "ACL leak: must 404 not 403"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_shape() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "search@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    seed_series(&db, lib_id, "Batman: Year One").await;
    seed_series(&db, lib_id, "Superman").await;

    // Note: existing OPDS search uses Postgres LIKE (case-sensitive) — the
    // audit flags expanding scope as a separate M2 follow-up. Match the
    // capitalisation of the seeded row here.
    let resp = get_with_auth(
        &app,
        "/opds/v1/search?q=Batman",
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(ct.starts_with("application/atom+xml"));
    let body = body_text(resp.into_body()).await;
    assert!(body.contains("Batman: Year One"));
    assert!(!body.contains("Superman"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_mime_branches() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "mime@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Mimes").await;
    let cbz = tmp.path().join("issue.cbz");
    let cbr = tmp.path().join("issue.cbr");
    let cbz_id = seed_issue_with_file(&db, lib_id, series_id, &cbz, b"cbz-bytes").await;
    let cbr_id = seed_issue_with_file(&db, lib_id, series_id, &cbr, b"cbr-bytes").await;

    let cbz_resp = get_with_auth(
        &app,
        &format!("/opds/v1/issues/{cbz_id}/file"),
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(cbz_resp.status(), StatusCode::OK);
    assert_eq!(
        cbz_resp
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap(),
        "application/vnd.comicbook+zip"
    );

    let cbr_resp = get_with_auth(
        &app,
        &format!("/opds/v1/issues/{cbr_id}/file"),
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(cbr_resp.status(), StatusCode::OK);
    assert_eq!(
        cbr_resp
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap(),
        "application/vnd.comicbook-rar"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bearer_auth_ok() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bearer@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let token = mint_app_password(&app, &auth, "chunky").await;

    let resp = get_with_auth(&app, "/opds/v1/recent", Header::Bearer(&token)).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn basic_auth_ok() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "basic@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let token = mint_app_password(&app, &auth, "kybook").await;
    let creds = base64::engine::general_purpose::STANDARD.encode(format!(":{token}"));

    let resp = get_with_auth(&app, "/opds/v1/recent", Header::Basic(creds)).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn basic_auth_jwt_rejected() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "footgun@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    // The session cookie value is the access JWT (eyJ…). Folding it into
    // Basic must be rejected — Basic is for `app_…` tokens only.
    let creds = base64::engine::general_purpose::STANDARD.encode(format!("user:{}", auth.session));

    let resp = get_with_auth(&app, "/opds/v1/recent", Header::Basic(creds)).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthenticated_includes_www_authenticate() {
    let app = TestApp::spawn().await;

    let resp = get_with_auth(&app, "/opds/v1/recent", Header::None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let challenge = resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("WWW-Authenticate present on 401");
    assert!(
        challenge.to_str().unwrap().contains("Basic"),
        "challenge advertises Basic scheme"
    );
}

// ─────────────────────────── M2 ───────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_list_emits_first_previous_next_last() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "pagelinks@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    for i in 0..60 {
        seed_series(&db, lib_id, &format!("Series {i:03}")).await;
    }

    // Page 1: first + next + last; no previous.
    let resp = get_with_auth(
        &app,
        "/opds/v1/series?page=1",
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(body.contains(r#"rel="first" href="/opds/v1/series?page=1""#));
    assert!(body.contains(r#"rel="next" href="/opds/v1/series?page=2""#));
    assert!(body.contains(r#"rel="last" href="/opds/v1/series?page=2""#));
    assert!(!body.contains(r#"rel="previous""#));

    // Page 2: previous + last; no next.
    let resp = get_with_auth(
        &app,
        "/opds/v1/series?page=2",
        Header::Cookie(auth.cookies()),
    )
    .await;
    let body = body_text(resp.into_body()).await;
    assert!(body.contains(r#"rel="previous" href="/opds/v1/series?page=1""#));
    assert!(body.contains(r#"rel="last" href="/opds/v1/series?page=2""#));
    assert!(!body.contains(r#"rel="next""#));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_one_paginates_at_50() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "perseries@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Long Run").await;
    for i in 0..55 {
        let file = tmp.path().join(format!("issue-{i:03}.cbz"));
        seed_issue_with_file(
            &db,
            lib_id,
            series_id,
            &file,
            format!("payload-{i:03}").as_bytes(),
        )
        .await;
    }

    let resp = get_with_auth(
        &app,
        &format!("/opds/v1/series/{series_id}?page=1"),
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert_eq!(body.matches("<entry>").count(), 50, "page 1 capped at 50");
    assert!(body.contains(&format!(
        r#"rel="next" href="/opds/v1/series/{series_id}?page=2""#
    )));
    assert!(body.contains(&format!(
        r#"rel="last" href="/opds/v1/series/{series_id}?page=2""#
    )));

    let resp = get_with_auth(
        &app,
        &format!("/opds/v1/series/{series_id}?page=2"),
        Header::Cookie(auth.cookies()),
    )
    .await;
    let body = body_text(resp.into_body()).await;
    assert_eq!(
        body.matches("<entry>").count(),
        5,
        "page 2 has the remaining 5"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_supports_range() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "range@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Ranged").await;
    let file = tmp.path().join("range.cbz");
    // Use 16 bytes so the byte arithmetic stays obvious.
    let payload = b"0123456789ABCDEF";
    let id = seed_issue_with_file(&db, lib_id, series_id, &file, payload).await;

    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/opds/v1/issues/{id}/file"))
        .header(header::COOKIE, auth.cookies())
        .header(header::RANGE, "bytes=4-9")
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp.headers().get(header::CONTENT_RANGE).unwrap(),
        "bytes 4-9/16"
    );
    assert_eq!(resp.headers().get(header::CONTENT_LENGTH).unwrap(), "6");
    assert_eq!(resp.headers().get(header::ACCEPT_RANGES).unwrap(), "bytes");
    assert_eq!(body_bytes(resp.into_body()).await, b"456789");

    // Open-ended `bytes=12-` returns the tail.
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/opds/v1/issues/{id}/file"))
        .header(header::COOKIE, auth.cookies())
        .header(header::RANGE, "bytes=12-")
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp.headers().get(header::CONTENT_RANGE).unwrap(),
        "bytes 12-15/16"
    );
    assert_eq!(body_bytes(resp.into_body()).await, b"CDEF");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_range_malformed_returns_416() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bad-range@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Bad Range").await;
    let file = tmp.path().join("bad.cbz");
    let id = seed_issue_with_file(&db, lib_id, series_id, &file, b"shortpayload").await;

    // Past end of resource.
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/opds/v1/issues/{id}/file"))
        .header(header::COOKIE, auth.cookies())
        .header(header::RANGE, "bytes=999-1500")
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    let content_range = resp.headers().get(header::CONTENT_RANGE).unwrap();
    assert!(content_range.to_str().unwrap().starts_with("bytes */"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_description_doc_shape() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "desc@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    let resp = get_with_auth(&app, "/opds/v1/search.xml", Header::Cookie(auth.cookies())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        ct.starts_with("application/opensearchdescription+xml"),
        "expected OpenSearch MIME, got {ct}"
    );
    let body = body_text(resp.into_body()).await;
    assert!(body.contains("<OpenSearchDescription"));
    assert!(body.contains("template="));
    assert!(body.contains("{searchTerms}"));
    assert!(body.contains("/opds/v1/search?q="));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_enrichment_present() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "metadata@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Metadata Series").await;
    let file = tmp.path().join("meta.cbz");
    let issue_id = seed_issue_with_file(&db, lib_id, series_id, &file, b"meta-bytes").await;

    // Backfill the metadata fields seed_issue_with_file leaves as None.
    let row = entity::issue::Entity::find_by_id(&issue_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::issue::ActiveModel = row.into();
    am.language_code = Set(Some("en".into()));
    am.publisher = Set(Some("Marvel".into()));
    am.year = Set(Some(2020));
    am.month = Set(Some(5));
    am.day = Set(Some(4));
    am.writer = Set(Some("Stan Lee, Steve Ditko".into()));
    am.genre = Set(Some("Superhero, Adventure".into()));
    am.tags = Set(Some("classic".into()));
    am.update(&db).await.unwrap();

    let resp = get_with_auth(
        &app,
        &format!("/opds/v1/series/{series_id}"),
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;

    assert!(
        body.contains(r#"xmlns:dc="http://purl.org/dc/terms/""#),
        "feed declares the dc namespace"
    );
    assert!(body.contains(&format!(
        "<dc:identifier>urn:folio:issue:{issue_id}</dc:identifier>"
    )));
    assert!(body.contains("<dc:language>en</dc:language>"));
    assert!(body.contains("<dc:publisher>Marvel</dc:publisher>"));
    assert!(body.contains("<dc:issued>2020-05-04</dc:issued>"));
    assert!(
        body.contains("<author><name>Stan Lee</name></author>"),
        "writer CSV is split and the first field becomes the author"
    );
    assert!(body.contains(r#"term="Superhero""#));
    assert!(body.contains(r#"term="Adventure""#));
    assert!(body.contains(r#"term="classic""#));
    // Distinct image rels: thumbnail (webp) AND full-size (jpeg).
    assert!(body.contains(r#"rel="http://opds-spec.org/image/thumbnail""#));
    assert!(body.contains(r#"rel="http://opds-spec.org/image" href="/issues/"#));
    // Deep-link back into the JSON API.
    assert!(body.contains(r#"rel="related" href="/series/"#));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_writes_audit_log() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "audit@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Audit Me").await;
    let file = tmp.path().join("audit.cbz");
    let id = seed_issue_with_file(&db, lib_id, series_id, &file, b"audit-bytes").await;

    let resp = get_with_auth(
        &app,
        &format!("/opds/v1/issues/{id}/file"),
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    use sea_orm::ColumnTrait;
    use sea_orm::QueryFilter;
    let rows = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("opds.download"))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "exactly one audit row per download");
    assert_eq!(rows[0].target_type.as_deref(), Some("issue"));
    assert_eq!(rows[0].target_id.as_deref(), Some(id.as_str()));
    assert_eq!(rows[0].actor_id, auth.user_id);
}
