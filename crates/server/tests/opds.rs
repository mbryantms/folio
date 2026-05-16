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
    cbl_entry::ActiveModel as CblEntryAM,
    cbl_list::ActiveModel as CblListAM,
    collection_entry::ActiveModel as CollectionEntryAM,
    issue::ActiveModel as IssueAM,
    library,
    saved_view::ActiveModel as SavedViewAM,
    series::{ActiveModel as SeriesAM, normalize_name},
    series_credit::ActiveModel as SeriesCreditAM,
    series_genre::ActiveModel as SeriesGenreAM,
    user_view_pin::ActiveModel as UserViewPinAM,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Database, DatabaseConnection, EntityTrait, QueryFilter, Set,
};
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
                .uri("/api/me/app-passwords")
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

/// Variant of [`seed_issue_with_file`] that exposes `sort_number` and
/// `state`/`removed_at`. The cover-resolution tests need to control
/// these to verify that the first-by-sort issue wins and that
/// removed/inactive issues are skipped. The 8-arg signature is
/// intentional — packaging this into a builder for a four-test
/// helper would be more code, not less.
#[allow(clippy::too_many_arguments)]
async fn seed_issue_full(
    db: &DatabaseConnection,
    lib_id: Uuid,
    series_id: Uuid,
    file_path: &std::path::Path,
    payload: &[u8],
    sort_number: f64,
    state: &str,
    removed: bool,
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
        state: Set(state.into()),
        content_hash: Set(hash.clone()),
        title: Set(Some("Issue".into())),
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
        removed_at: Set(if removed { Some(now) } else { None }),
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
    assert!(body.contains(r#"rel="related" href="/api/series/"#));
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

// ─────────────────────────── M4 — personal surfaces ───────────────────────────

async fn seed_cbl_list(db: &DatabaseConnection, owner: Option<Uuid>, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    CblListAM {
        id: Set(id),
        owner_user_id: Set(owner),
        source_kind: Set("upload".into()),
        source_url: Set(None),
        catalog_source_id: Set(None),
        catalog_path: Set(None),
        github_blob_sha: Set(None),
        source_etag: Set(None),
        source_last_modified: Set(None),
        raw_sha256: Set(vec![0u8; 32]),
        raw_xml: Set("<ReadingList />".into()),
        parsed_name: Set(name.into()),
        parsed_matchers_present: Set(false),
        num_issues_declared: Set(None),
        description: Set(Some(format!("{name} description"))),
        imported_at: Set(now),
        last_refreshed_at: Set(None),
        last_match_run_at: Set(None),
        refresh_schedule: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn seed_cbl_entry(
    db: &DatabaseConnection,
    list_id: Uuid,
    position: i32,
    matched_issue_id: Option<&str>,
) {
    let now = Utc::now().fixed_offset();
    let status = if matched_issue_id.is_some() {
        "matched"
    } else {
        "missing"
    };
    CblEntryAM {
        id: Set(Uuid::now_v7()),
        cbl_list_id: Set(list_id),
        position: Set(position),
        series_name: Set("Seed".into()),
        issue_number: Set(position.to_string()),
        volume: Set(None),
        year: Set(None),
        cv_series_id: Set(None),
        cv_issue_id: Set(None),
        metron_series_id: Set(None),
        metron_issue_id: Set(None),
        matched_issue_id: Set(matched_issue_id.map(str::to_owned)),
        match_status: Set(status.into()),
        match_method: Set(None),
        match_confidence: Set(None),
        ambiguous_candidates: Set(None),
        user_resolved_at: Set(None),
        matched_at: Set(matched_issue_id.map(|_| now)),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn seed_collection(db: &DatabaseConnection, owner: Uuid, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    SavedViewAM {
        id: Set(id),
        user_id: Set(Some(owner)),
        kind: Set("collection".into()),
        system_key: Set(None),
        name: Set(name.into()),
        description: Set(Some(format!("{name} desc"))),
        custom_year_start: Set(None),
        custom_year_end: Set(None),
        custom_tags: Set(Vec::new()),
        match_mode: Set(None),
        conditions: Set(None),
        sort_field: Set(None),
        sort_order: Set(None),
        result_limit: Set(None),
        cbl_list_id: Set(None),
        auto_pin: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn seed_collection_entry(
    db: &DatabaseConnection,
    view_id: Uuid,
    position: i32,
    series_id: Option<Uuid>,
    issue_id: Option<&str>,
) {
    let kind = if series_id.is_some() {
        "series"
    } else {
        "issue"
    };
    let now = Utc::now().fixed_offset();
    CollectionEntryAM {
        id: Set(Uuid::now_v7()),
        saved_view_id: Set(view_id),
        position: Set(position),
        entry_kind: Set(kind.into()),
        series_id: Set(series_id),
        issue_id: Set(issue_id.map(str::to_owned)),
        added_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn seed_filter_view(
    db: &DatabaseConnection,
    owner: Uuid,
    name: &str,
    conditions: serde_json::Value,
    sort_field: &str,
    result_limit: i32,
) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    SavedViewAM {
        id: Set(id),
        user_id: Set(Some(owner)),
        kind: Set("filter_series".into()),
        system_key: Set(None),
        name: Set(name.into()),
        description: Set(None),
        custom_year_start: Set(None),
        custom_year_end: Set(None),
        custom_tags: Set(Vec::new()),
        match_mode: Set(Some("all".into())),
        conditions: Set(Some(conditions)),
        sort_field: Set(Some(sort_field.into())),
        sort_order: Set(Some("asc".into())),
        result_limit: Set(Some(result_limit)),
        cbl_list_id: Set(None),
        auto_pin: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn pin_view(
    db: &DatabaseConnection,
    user_id: Uuid,
    view_id: Uuid,
    pinned: bool,
    sidebar: bool,
) {
    let page_id = server::pages::system_page_id(db, user_id).await.unwrap();
    UserViewPinAM {
        user_id: Set(user_id),
        page_id: Set(page_id),
        view_id: Set(view_id),
        position: Set(0),
        pinned: Set(pinned),
        show_in_sidebar: Set(sidebar),
        icon: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_navigation_includes_personal_subsections() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "root-personal@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let resp = get_with_auth(&app, "/opds/v1", Header::Cookie(auth.cookies())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(body.contains(r#"href="/opds/v1/wtr""#));
    assert!(body.contains(r#"href="/opds/v1/lists""#));
    assert!(body.contains(r#"href="/opds/v1/collections""#));
    assert!(body.contains(r#"href="/opds/v1/views""#));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wtr_acq_feed_seeds_and_lists_added_entry() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "wtr@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "WTR Pick").await;

    // First call seeds. Confirm 200 + empty entry list.
    let resp = get_with_auth(&app, "/opds/v1/wtr", Header::Cookie(auth.cookies())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(body.contains("<title>Want to Read</title>"));
    assert_eq!(body.matches("<entry>").count(), 0, "fresh WTR is empty");

    // Add a series entry via direct DB write (uses the same shape the
    // collections.rs handler emits — independent of OPDS code paths).
    use entity::saved_view::Column as SVCol;
    let wtr = entity::saved_view::Entity::find()
        .filter(SVCol::UserId.eq(auth.user_id))
        .filter(SVCol::SystemKey.eq("want_to_read"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    seed_collection_entry(&db, wtr.id, 0, Some(series_id), None).await;

    let resp = get_with_auth(&app, "/opds/v1/wtr", Header::Cookie(auth.cookies())).await;
    let body = body_text(resp.into_body()).await;
    assert_eq!(body.matches("<entry>").count(), 1);
    assert!(body.contains(&format!(r#"href="/opds/v1/series/{series_id}""#)));
    assert!(body.contains("WTR Pick"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_list_acq_resolves_matched_issues_in_position_order() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cbl@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Run").await;
    let i0 = seed_issue_with_file(&db, lib_id, series_id, &tmp.path().join("a.cbz"), b"a").await;
    let i1 = seed_issue_with_file(&db, lib_id, series_id, &tmp.path().join("b.cbz"), b"b").await;

    let list_id = seed_cbl_list(&db, Some(auth.user_id), "My List").await;
    // Note: positions deliberately out of order in insertion to confirm
    // we sort by `position` not insertion time.
    seed_cbl_entry(&db, list_id, 1, Some(&i1)).await;
    seed_cbl_entry(&db, list_id, 0, Some(&i0)).await;
    // Unmatched entries must drop out of the acq feed.
    seed_cbl_entry(&db, list_id, 2, None).await;

    let resp = get_with_auth(
        &app,
        &format!("/opds/v1/lists/{list_id}"),
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert_eq!(body.matches("<entry>").count(), 2, "unmatched dropped");
    let pos_i0 = body.find(&format!("urn:issue:{i0}")).unwrap();
    let pos_i1 = body.find(&format!("urn:issue:{i1}")).unwrap();
    assert!(pos_i0 < pos_i1, "position 0 must come before position 1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_lists_nav_lists_user_owned_and_system() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "cbl-admin@example.com").await;
    promote_to_admin(&app, admin.user_id).await;
    let other = register(&app, "cbl-other@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let own = seed_cbl_list(&db, Some(admin.user_id), "Mine").await;
    let _theirs = seed_cbl_list(&db, Some(other.user_id), "Theirs").await;
    let system = seed_cbl_list(&db, None, "System").await;

    let resp = get_with_auth(&app, "/opds/v1/lists", Header::Cookie(admin.cookies())).await;
    let body = body_text(resp.into_body()).await;
    assert!(body.contains(&own.to_string()));
    assert!(body.contains(&system.to_string()), "system lists surface");
    assert!(!body.contains("Theirs"), "other user's lists must not leak");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collections_nav_lists_user_owned() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "col-admin@example.com").await;
    promote_to_admin(&app, admin.user_id).await;
    let other = register(&app, "col-other@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let own = seed_collection(&db, admin.user_id, "Capes").await;
    let _theirs = seed_collection(&db, other.user_id, "Cosmic").await;

    let resp = get_with_auth(
        &app,
        "/opds/v1/collections",
        Header::Cookie(admin.cookies()),
    )
    .await;
    let body = body_text(resp.into_body()).await;
    assert!(body.contains(&own.to_string()));
    // WTR is auto-seeded for the calling user and shows up first.
    assert!(body.contains("<title>Want to Read</title>"));
    assert!(body.find("Want to Read").unwrap() < body.find("Capes").unwrap());
    assert!(
        !body.contains("Cosmic"),
        "other user's collection invisible"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_acq_mixes_series_and_issues() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "mixed@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Mixed Series").await;
    let issue_id =
        seed_issue_with_file(&db, lib_id, series_id, &tmp.path().join("m.cbz"), b"m").await;

    let view_id = seed_collection(&db, auth.user_id, "Mixed").await;
    seed_collection_entry(&db, view_id, 0, Some(series_id), None).await;
    seed_collection_entry(&db, view_id, 1, None, Some(&issue_id)).await;

    let resp = get_with_auth(
        &app,
        &format!("/opds/v1/collections/{view_id}"),
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert_eq!(body.matches("<entry>").count(), 2);
    // Series entry uses subsection link into per-series feed.
    assert!(body.contains(&format!(r#"href="/opds/v1/series/{series_id}""#)));
    // Issue entry uses acquisition link to the file endpoint.
    assert!(body.contains(&format!(r#"href="/opds/v1/issues/{issue_id}/file""#)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_acq_other_user_returns_404() {
    let app = TestApp::spawn().await;
    let owner = register(&app, "co-owner@example.com").await;
    promote_to_admin(&app, owner.user_id).await;
    let snooper = register(&app, "co-snooper@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let view_id = seed_collection(&db, owner.user_id, "Private").await;

    let resp = get_with_auth(
        &app,
        &format!("/opds/v1/collections/{view_id}"),
        Header::Cookie(snooper.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn views_nav_filters_to_pinned_filter_views() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "views@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let pinned = seed_filter_view(
        &db,
        auth.user_id,
        "Pinned",
        serde_json::json!([]),
        "name",
        20,
    )
    .await;
    let sidebar = seed_filter_view(
        &db,
        auth.user_id,
        "Sidebar",
        serde_json::json!([]),
        "name",
        20,
    )
    .await;
    let invisible = seed_filter_view(
        &db,
        auth.user_id,
        "Invisible",
        serde_json::json!([]),
        "name",
        20,
    )
    .await;

    pin_view(&db, auth.user_id, pinned, true, false).await;
    pin_view(&db, auth.user_id, sidebar, false, true).await;
    // `invisible` has no pin row → must NOT appear.

    let resp = get_with_auth(&app, "/opds/v1/views", Header::Cookie(auth.cookies())).await;
    let body = body_text(resp.into_body()).await;
    assert!(body.contains(&pinned.to_string()));
    assert!(body.contains(&sidebar.to_string()));
    assert!(!body.contains(&invisible.to_string()), "unpinned filtered");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn view_acq_evaluates_filter_server_side() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "view-eval@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    seed_series(&db, lib_id, "Alpha").await;
    seed_series(&db, lib_id, "Beta").await;
    seed_series(&db, lib_id, "Gamma").await;

    // Filter: name contains 'Beta'. `op: contains` on the name (text)
    // field is the standard text-search predicate in the DSL.
    let view_id = seed_filter_view(
        &db,
        auth.user_id,
        "B-things",
        serde_json::json!([
            { "field": "name", "op": "contains", "value": "Beta" }
        ]),
        "name",
        50,
    )
    .await;

    let resp = get_with_auth(
        &app,
        &format!("/opds/v1/views/{view_id}"),
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(body.contains("Beta"));
    assert!(!body.contains("Alpha"), "filter excludes Alpha");
    assert!(!body.contains("Gamma"), "filter excludes Gamma");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn view_acq_rejects_non_filter_kind() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "wrongkind@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    // Collections are saved_views with kind='collection'. /opds/v1/views
    // only exposes filter_series kinds, so this must 404.
    let collection_id = seed_collection(&db, auth.user_id, "Not a filter").await;

    let resp = get_with_auth(
        &app,
        &format!("/opds/v1/views/{collection_id}"),
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ─────────────── M1 (opds-richer-feeds): series covers ───────────────

/// `series_list` emits a series with at least one active issue with
/// the OPDS image rels pointing at that issue's page-0 thumbnail.
/// Without these rels every client falls back to a folder icon.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_list_emits_cover_rels_for_series_with_active_issue() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "covers@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "With Cover").await;
    let issue_id = seed_issue_with_file(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"cbz-stub-a",
    )
    .await;

    let resp = get_with_auth(&app, "/opds/v1/series", Header::Cookie(auth.cookies())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    let thumb_rel = format!(
        r#"rel="http://opds-spec.org/image/thumbnail" href="/issues/{issue_id}/pages/0/thumb""#
    );
    let full_rel = format!(r#"rel="http://opds-spec.org/image" href="/issues/{issue_id}/pages/0""#);
    assert!(
        body.contains(&thumb_rel),
        "missing thumbnail rel in feed: {body}"
    );
    assert!(body.contains(&full_rel), "missing image rel: {body}");
}

/// A series with zero active issues (empty library or all-removed)
/// degrades gracefully: the entry is still emitted but with NO image
/// rels — better than a 500 or omitting the series entirely. Client
/// renders its placeholder.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_list_omits_cover_rels_for_empty_series() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "emptyseries@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "No Issues Yet").await;

    let resp = get_with_auth(&app, "/opds/v1/series", Header::Cookie(auth.cookies())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    // Series entry IS emitted...
    assert!(body.contains(&format!("urn:series:{series_id}")));
    assert!(body.contains("No Issues Yet"));
    // ...but no image rels for it.
    assert!(
        !body.contains("opds-spec.org/image"),
        "no issues = no image rels: {body}"
    );
}

/// The cover-issue selection follows `sort_number ASC` — the
/// canonical "first issue" of the series. With three issues at sort
/// 1.0 / 2.0 / 3.0, the cover must be issue-1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_cover_picks_lowest_sort_number() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "sortcover@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Three Issues").await;
    // Seed in reverse order to make sure the cover query — not insert
    // order — drives the pick.
    let issue_3 = seed_issue_full(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("c.cbz"),
        b"three",
        3.0,
        "active",
        false,
    )
    .await;
    let issue_1 = seed_issue_full(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("a.cbz"),
        b"one",
        1.0,
        "active",
        false,
    )
    .await;
    let issue_2 = seed_issue_full(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("b.cbz"),
        b"two",
        2.0,
        "active",
        false,
    )
    .await;

    let resp = get_with_auth(&app, "/opds/v1/series", Header::Cookie(auth.cookies())).await;
    let body = body_text(resp.into_body()).await;
    // Cover should be issue_1 (lowest sort_number).
    assert!(
        body.contains(&format!("href=\"/issues/{issue_1}/pages/0/thumb\"")),
        "expected issue_1 as cover, got: {body}"
    );
    assert!(
        !body.contains(&format!("href=\"/issues/{issue_2}/pages/0/thumb\"")),
        "issue_2 should not be the cover"
    );
    assert!(
        !body.contains(&format!("href=\"/issues/{issue_3}/pages/0/thumb\"")),
        "issue_3 should not be the cover"
    );
}

/// Removed or non-active issues are excluded from cover selection
/// even when they have the lowest sort_number. Otherwise a deleted
/// issue would keep haunting the series's cover slot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_cover_skips_removed_and_inactive_issues() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "skipremoved@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "With Removed").await;
    // Lowest sort_number is removed; next is non-active state; only
    // the third should be eligible as cover.
    let removed = seed_issue_full(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("rm.cbz"),
        b"removed",
        1.0,
        "active",
        true,
    )
    .await;
    let inactive = seed_issue_full(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("inactive.cbz"),
        b"inactive",
        2.0,
        "removed",
        false,
    )
    .await;
    let visible = seed_issue_full(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("ok.cbz"),
        b"visible",
        3.0,
        "active",
        false,
    )
    .await;

    let resp = get_with_auth(&app, "/opds/v1/series", Header::Cookie(auth.cookies())).await;
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains(&format!("href=\"/issues/{visible}/pages/0/thumb\"")),
        "expected visible issue as cover"
    );
    assert!(
        !body.contains(&format!("href=\"/issues/{removed}/pages/0/thumb\"")),
        "removed issue must not be picked"
    );
    assert!(
        !body.contains(&format!("href=\"/issues/{inactive}/pages/0/thumb\"")),
        "non-active issue must not be picked"
    );
}

// ─────────────── M2 (opds-richer-feeds): series metadata ───────────────

/// Patch the series row in-place to carry publisher/year/language —
/// the dimensions M2 surfaces as `<dc:publisher>` / `<dc:issued>` /
/// `<dc:language>` in OPDS output. `seed_series` defaults publisher
/// to None and language to "en"; this lets tests override.
async fn set_series_meta(
    db: &DatabaseConnection,
    series_id: Uuid,
    publisher: Option<&str>,
    year: Option<i32>,
    language: Option<&str>,
) {
    use entity::series::Entity as SeriesEntity;
    let row = SeriesEntity::find_by_id(series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::series::ActiveModel = row.into();
    am.publisher = Set(publisher.map(str::to_owned));
    am.year = Set(year);
    if let Some(l) = language {
        am.language_code = Set(l.to_owned());
    }
    am.update(db).await.unwrap();
}

async fn add_series_writer(db: &DatabaseConnection, series_id: Uuid, person: &str) {
    SeriesCreditAM {
        series_id: Set(series_id),
        role: Set("writer".into()),
        person: Set(person.into()),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn add_series_genre(db: &DatabaseConnection, series_id: Uuid, genre: &str) {
    SeriesGenreAM {
        series_id: Set(series_id),
        genre: Set(genre.into()),
    }
    .insert(db)
    .await
    .unwrap();
}

/// dc:publisher / dc:issued / dc:language land on the series entry
/// when the corresponding columns are populated. The feed `<feed>`
/// element MUST declare `xmlns:dc` or strict OPDS parsers reject
/// the document — verify that too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_entry_carries_dublin_core_metadata() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "dc-meta@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Saga").await;
    set_series_meta(&db, series_id, Some("Image Comics"), Some(2012), Some("en")).await;

    let resp = get_with_auth(&app, "/opds/v1/series", Header::Cookie(auth.cookies())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains(r#"xmlns:dc="http://purl.org/dc/terms/""#),
        "feed must declare the Dublin Core namespace: {body}"
    );
    assert!(body.contains("<dc:publisher>Image Comics</dc:publisher>"));
    assert!(body.contains("<dc:issued>2012</dc:issued>"));
    assert!(body.contains("<dc:language>en</dc:language>"));
}

/// When publisher/year are not set, the entry omits those elements
/// rather than emitting empty tags. Empty `<dc:publisher/>` would
/// break clients that treat it as a non-null string.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_entry_omits_unset_metadata() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "dc-empty@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Unset").await;
    // Clear the seed defaults (publisher already None; year defaults to 2020).
    set_series_meta(&db, series_id, None, None, Some("en")).await;

    let resp = get_with_auth(&app, "/opds/v1/series", Header::Cookie(auth.cookies())).await;
    let body = body_text(resp.into_body()).await;
    assert!(!body.contains("<dc:publisher>"));
    assert!(!body.contains("<dc:issued>"));
    // language always present (the column is non-nullable, defaults to "en").
    assert!(body.contains("<dc:language>en</dc:language>"));
}

/// Series_credits role='writer' rows surface as one `<author>` per
/// writer, ordered alphabetically for stable output. Series_genres
/// rows surface as one `<category>` chip per genre, also sorted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_entry_emits_authors_and_categories() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "facets@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Paper Girls").await;
    // Insert in non-alphabetical order to verify the helper sorts.
    add_series_writer(&db, series_id, "Cliff Chiang").await;
    add_series_writer(&db, series_id, "Brian K. Vaughan").await;
    add_series_genre(&db, series_id, "Science Fiction").await;
    add_series_genre(&db, series_id, "Coming of Age").await;

    let resp = get_with_auth(&app, "/opds/v1/series", Header::Cookie(auth.cookies())).await;
    let body = body_text(resp.into_body()).await;
    // M5 also adds a `<uri>` drill-in link inside `<author>`.
    assert!(body.contains("<author><name>Brian K. Vaughan</name>"));
    assert!(body.contains("<author><name>Cliff Chiang</name>"));
    let vaughan = body.find("Brian K. Vaughan").unwrap();
    let chiang = body.find("Cliff Chiang").unwrap();
    assert!(
        vaughan < chiang,
        "writers must be sorted alphabetically for stable output"
    );
    assert!(body.contains(r#"term="Science Fiction""#));
    assert!(body.contains(r#"term="Coming of Age""#));
    assert!(body.contains(r#"scheme="urn:folio:genre""#));
}

/// HTML-shaped summaries are emitted as `<content type="html">`;
/// plaintext summaries stay as `<summary>`. Markdown emphasis
/// markers also trigger the html branch (false positives are
/// harmless — both elements XML-escape the body identically).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_entry_promotes_rich_summary_to_content_html() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "content-html@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let plain = seed_series(&db, lib_id, "Plain Description").await;
    let rich = seed_series(&db, lib_id, "Rich Description").await;
    // Both seeded with None summary by default — patch directly.
    use entity::series::Entity as SeriesEntity;
    for (id, body) in [
        (plain, "A simple one-line description with no markup."),
        (rich, "<p>A <strong>rich</strong> description.</p>"),
    ] {
        let row = SeriesEntity::find_by_id(id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        let mut am: entity::series::ActiveModel = row.into();
        am.summary = Set(Some(body.into()));
        am.update(&db).await.unwrap();
    }

    let resp = get_with_auth(&app, "/opds/v1/series", Header::Cookie(auth.cookies())).await;
    let body = body_text(resp.into_body()).await;
    assert!(body.contains("<summary>A simple one-line description"));
    assert!(body.contains(r#"<content type="html">"#));
    assert!(body.contains("&lt;p&gt;A &lt;strong&gt;rich&lt;/strong&gt;"));
}

// ───────────── M3 (opds-richer-feeds): user Pages → OPDS ─────────────

/// Insert a custom (non-system) page for `user_id` so /opds/v1/pages
/// surfaces something other than the auto-created Home.
async fn seed_custom_page(
    db: &DatabaseConnection,
    user_id: Uuid,
    name: &str,
    slug: &str,
    position: i32,
) -> Uuid {
    use entity::user_page::ActiveModel as UserPageAM;
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    UserPageAM {
        id: Set(id),
        user_id: Set(user_id),
        name: Set(name.into()),
        slug: Set(slug.into()),
        is_system: Set(false),
        position: Set(position),
        description: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

/// Pin a saved-view to a specific page (not the system page) — the
/// existing `pin_view` helper always targets system_page_id, which
/// isn't enough for tests that need custom pages.
async fn pin_view_to_page(
    db: &DatabaseConnection,
    user_id: Uuid,
    page_id: Uuid,
    view_id: Uuid,
    position: i32,
    pinned: bool,
    sidebar: bool,
) {
    UserViewPinAM {
        user_id: Set(user_id),
        page_id: Set(page_id),
        view_id: Set(view_id),
        position: Set(position),
        pinned: Set(pinned),
        show_in_sidebar: Set(sidebar),
        icon: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
}

/// `/opds/v1/pages` lists every page the user owns in `position`
/// order. The auto-created Home page appears alongside any custom
/// pages and drills into `/opds/v1/pages/{slug}`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pages_nav_lists_user_pages_in_position_order() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "pages-list@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    // Touch system_page_id to materialise the Home row before adding
    // custom pages — otherwise lazy-creation runs at request time and
    // would interleave with our test setup.
    let _ = server::pages::system_page_id(&db, auth.user_id)
        .await
        .unwrap();
    seed_custom_page(&db, auth.user_id, "Marvel", "marvel", 1).await;
    seed_custom_page(&db, auth.user_id, "DC", "dc", 2).await;

    let resp = get_with_auth(&app, "/opds/v1/pages", Header::Cookie(auth.cookies())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(body.contains(r#"<title>My pages</title>"#));
    assert!(body.contains(r#"<title>Home</title>"#));
    assert!(body.contains(r#"<title>Marvel</title>"#));
    assert!(body.contains(r#"<title>DC</title>"#));
    // Position order: Home (0) → Marvel (1) → DC (2).
    let home = body.find("Home").unwrap();
    let marvel = body.find("Marvel").unwrap();
    let dc = body.find("DC").unwrap();
    assert!(
        home < marvel && marvel < dc,
        "position order violated: {body}"
    );
    // Drill-in link for the custom page surfaces its slug.
    assert!(body.contains(r#"href="/opds/v1/pages/marvel""#));
    assert!(body.contains(r#"href="/opds/v1/pages/dc""#));
    assert!(body.contains(r#"href="/opds/v1/pages/home""#));
}

/// Visiting a page that has no pinned views returns an empty nav
/// feed (200 OK, zero entries) — clients render an "empty section"
/// message rather than treating the absence as an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_acq_returns_empty_feed_when_no_pins() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "page-empty@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    seed_custom_page(&db, auth.user_id, "Untouched", "untouched", 1).await;

    let resp = get_with_auth(
        &app,
        "/opds/v1/pages/untouched",
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(body.contains("<title>Untouched</title>"));
    // No `<entry>` blocks for views.
    assert!(
        !body.contains("<entry>"),
        "empty feed must emit no entries: {body}"
    );
}

/// A populated page surfaces its pinned saved-views as subsection
/// nav entries in pin-position order, each linking back into the
/// existing `/opds/v1/views/{id}` handler.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_acq_renders_pinned_views_in_position_order() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "page-pins@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let page_id = seed_custom_page(&db, auth.user_id, "Curated", "curated", 1).await;
    let v_a = seed_filter_view(
        &db,
        auth.user_id,
        "Alpha",
        serde_json::json!({"all":[]}),
        "name",
        50,
    )
    .await;
    let v_b = seed_filter_view(
        &db,
        auth.user_id,
        "Bravo",
        serde_json::json!({"all":[]}),
        "name",
        50,
    )
    .await;
    // Insert pins out of position order to verify the handler sorts.
    pin_view_to_page(&db, auth.user_id, page_id, v_b, 1, true, false).await;
    pin_view_to_page(&db, auth.user_id, page_id, v_a, 0, true, false).await;

    let resp = get_with_auth(
        &app,
        "/opds/v1/pages/curated",
        Header::Cookie(auth.cookies()),
    )
    .await;
    let body = body_text(resp.into_body()).await;
    assert!(body.contains("<title>Curated</title>"));
    assert!(body.contains(&format!(r#"href="/opds/v1/views/{v_a}""#)));
    assert!(body.contains(&format!(r#"href="/opds/v1/views/{v_b}""#)));
    let alpha = body.find("Alpha").unwrap();
    let bravo = body.find("Bravo").unwrap();
    assert!(
        alpha < bravo,
        "Alpha (pin position 0) must precede Bravo (1)"
    );
}

/// Pins where neither `pinned` nor `show_in_sidebar` is set are
/// stored but inactive — the user has the view in their library but
/// isn't actively using it. Don't surface those in the page feed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_acq_filters_out_unpinned_views() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "page-unpinned@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let page_id = seed_custom_page(&db, auth.user_id, "Mixed", "mixed", 1).await;
    let active = seed_filter_view(
        &db,
        auth.user_id,
        "Active",
        serde_json::json!({"all":[]}),
        "name",
        50,
    )
    .await;
    let dormant = seed_filter_view(
        &db,
        auth.user_id,
        "Dormant",
        serde_json::json!({"all":[]}),
        "name",
        50,
    )
    .await;
    pin_view_to_page(&db, auth.user_id, page_id, active, 0, true, false).await;
    pin_view_to_page(&db, auth.user_id, page_id, dormant, 1, false, false).await;

    let resp = get_with_auth(&app, "/opds/v1/pages/mixed", Header::Cookie(auth.cookies())).await;
    let body = body_text(resp.into_body()).await;
    assert!(body.contains("Active"));
    assert!(!body.contains("Dormant"));
}

/// Pages are private per-user. A user requesting another user's
/// page slug must see 404, not a leak of name/contents.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_acq_returns_404_for_other_users_page() {
    let app = TestApp::spawn().await;
    let owner = register(&app, "page-owner@example.com").await;
    promote_to_admin(&app, owner.user_id).await;
    let intruder = register(&app, "page-intruder@example.com").await;
    promote_to_admin(&app, intruder.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    seed_custom_page(&db, owner.user_id, "Private", "private", 1).await;

    let resp = get_with_auth(
        &app,
        "/opds/v1/pages/private",
        Header::Cookie(intruder.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ─────────────── M4 (opds-richer-feeds): faceted browse ───────────────

/// Variant of [`seed_series`] that takes status + publisher so M4
/// tests can build the matrix of facet combinations they need
/// without patching each row after insert.
async fn seed_series_full(
    db: &DatabaseConnection,
    lib_id: Uuid,
    name: &str,
    status: &str,
    publisher: Option<&str>,
) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    SeriesAM {
        id: Set(id),
        library_id: Set(lib_id),
        name: Set(name.into()),
        normalized_name: Set(normalize_name(name)),
        year: Set(Some(2020)),
        volume: Set(None),
        publisher: Set(publisher.map(str::to_owned)),
        imprint: Set(None),
        status: Set(status.into()),
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

/// Unfiltered `/opds/v1/browse` returns every series the user is
/// allowed to see, advertises facet links for both groups (Status +
/// Publisher), and declares the OPDS catalog namespace so the
/// `opds:facetGroup` / `opds:activeFacet` attributes are valid XML.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browse_advertises_facet_groups() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "browse-facets@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    seed_series_full(&db, lib_id, "Marvel A", "continuing", Some("Marvel")).await;
    seed_series_full(&db, lib_id, "Marvel B", "ended", Some("Marvel")).await;
    seed_series_full(&db, lib_id, "DC A", "continuing", Some("DC Comics")).await;

    let resp = get_with_auth(&app, "/opds/v1/browse", Header::Cookie(auth.cookies())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    // Namespace declarations required for the facet attributes to
    // parse as valid XML.
    assert!(
        body.contains(r#"xmlns:opds="http://opds-spec.org/2010/catalog""#),
        "missing opds namespace: {body}"
    );
    // Status facet group surfaces with all four known values, none
    // selected initially.
    for status in ["continuing", "ended", "hiatus", "cancelled"] {
        assert!(
            body.contains(&format!(r#"href="/opds/v1/browse?status={status}""#)),
            "missing facet link for status={status}: {body}"
        );
    }
    assert!(body.contains(r#"opds:facetGroup="Status""#));
    // Publisher facet group surfaces with each distinct value.
    assert!(body.contains(r#"opds:facetGroup="Publisher""#));
    assert!(body.contains(r#"title="Marvel""#));
    assert!(body.contains(r#"title="DC Comics""#));
    // All three seeded series appear when no filter is applied.
    assert!(body.contains("Marvel A"));
    assert!(body.contains("Marvel B"));
    assert!(body.contains("DC A"));
}

/// `?status=continuing` returns only continuing series and marks
/// that status facet as `opds:activeFacet="true"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browse_status_facet_filters_and_marks_active() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "browse-status@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    seed_series_full(&db, lib_id, "Alive", "continuing", None).await;
    seed_series_full(&db, lib_id, "Done", "ended", None).await;

    let resp = get_with_auth(
        &app,
        "/opds/v1/browse?status=continuing",
        Header::Cookie(auth.cookies()),
    )
    .await;
    let body = body_text(resp.into_body()).await;
    assert!(body.contains("Alive"));
    assert!(
        !body.contains("Done"),
        "ended series must be filtered out: {body}"
    );
    // The status=continuing link is marked active AND its href now
    // clears the filter (toggle behaviour).
    assert!(
        body.contains(r#"href="/opds/v1/browse" title="Continuing" opds:facetGroup="Status" opds:activeFacet="true""#),
        "continuing facet not marked active or toggle href wrong: {body}"
    );
}

/// `?publisher=Marvel` filters to Marvel series only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browse_publisher_facet_filters() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "browse-pub@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    seed_series_full(&db, lib_id, "Daredevil", "continuing", Some("Marvel")).await;
    seed_series_full(&db, lib_id, "Batman", "continuing", Some("DC Comics")).await;

    let resp = get_with_auth(
        &app,
        "/opds/v1/browse?publisher=Marvel",
        Header::Cookie(auth.cookies()),
    )
    .await;
    let body = body_text(resp.into_body()).await;
    assert!(body.contains("Daredevil"));
    assert!(
        !body.contains("Batman"),
        "DC series must be filtered out: {body}"
    );
}

/// Stacking `?status=continuing&publisher=Marvel` returns the
/// intersection — series matching BOTH facets. Pagination links
/// preserve the facet selection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browse_facets_stack_as_and_intersection() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "browse-stack@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    seed_series_full(&db, lib_id, "Match", "continuing", Some("Marvel")).await;
    seed_series_full(&db, lib_id, "WrongPub", "continuing", Some("DC Comics")).await;
    seed_series_full(&db, lib_id, "WrongStatus", "ended", Some("Marvel")).await;

    let resp = get_with_auth(
        &app,
        "/opds/v1/browse?status=continuing&publisher=Marvel",
        Header::Cookie(auth.cookies()),
    )
    .await;
    let body = body_text(resp.into_body()).await;
    assert!(body.contains("Match"));
    assert!(!body.contains("WrongPub"));
    assert!(!body.contains("WrongStatus"));
    // Both active facet links visible; toggling Marvel clears just
    // that param, keeping status=continuing.
    assert!(body.contains(r#"href="/opds/v1/browse?status=continuing" title="Marvel""#));
}

/// Unknown status values silently fall through to "no status filter"
/// rather than 400. Lets stale facet hrefs from removed enum values
/// degrade gracefully instead of breaking navigation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browse_unknown_status_is_ignored() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "browse-bogus@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    seed_series_full(&db, lib_id, "Anything", "continuing", None).await;

    let resp = get_with_auth(
        &app,
        "/opds/v1/browse?status=bogus",
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains("Anything"),
        "unknown status should yield no filter: {body}"
    );
}

// ─── M4 follow-up: CBL/collection pins surface in /opds/v1/pages/{slug} ───

async fn seed_cbl_saved_view(
    db: &DatabaseConnection,
    owner: Uuid,
    name: &str,
    cbl_list_id: Uuid,
) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    SavedViewAM {
        id: Set(id),
        user_id: Set(Some(owner)),
        kind: Set("cbl".into()),
        system_key: Set(None),
        name: Set(name.into()),
        description: Set(None),
        custom_year_start: Set(None),
        custom_year_end: Set(None),
        custom_tags: Set(Vec::new()),
        match_mode: Set(None),
        conditions: Set(None),
        sort_field: Set(None),
        sort_order: Set(None),
        result_limit: Set(None),
        cbl_list_id: Set(Some(cbl_list_id)),
        auto_pin: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

/// Pinning a CBL-kind saved-view onto a page surfaces it in the
/// page's OPDS feed, drilling into the existing /opds/v1/lists/{id}
/// handler. The bug: M3 only matched `KIND_FILTER_SERIES`; CBL +
/// collection pins were silently dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_acq_surfaces_cbl_pins() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "page-cbl@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let page_id = seed_custom_page(&db, auth.user_id, "Reading", "reading", 1).await;
    let cbl_id = seed_cbl_list(&db, Some(auth.user_id), "Civil War").await;
    let view_id = seed_cbl_saved_view(&db, auth.user_id, "Civil War Reading Order", cbl_id).await;
    pin_view_to_page(&db, auth.user_id, page_id, view_id, 0, true, false).await;

    let resp = get_with_auth(
        &app,
        "/opds/v1/pages/reading",
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(body.contains("Civil War Reading Order"));
    // CBL pin dispatches to /opds/v1/lists/{cbl_list_id}, not
    // /opds/v1/views/{view_id}.
    assert!(
        body.contains(&format!(r#"href="/opds/v1/lists/{cbl_id}""#)),
        "CBL pin must route to /opds/v1/lists/<cbl_id>: {body}"
    );
    assert!(
        !body.contains(&format!(r#"href="/opds/v1/views/{view_id}""#)),
        "CBL pin must NOT route to /opds/v1/views/<view_id>"
    );
    assert!(body.contains(&format!("urn:cbl:{cbl_id}")));
}

// ─────────── M5 (opds-richer-feeds): aggregation feeds ───────────

async fn seed_progress(
    db: &DatabaseConnection,
    user_id: Uuid,
    issue_id: &str,
    last_page: i32,
    finished: bool,
) {
    use entity::progress_record::ActiveModel as ProgressAM;
    let now = Utc::now().fixed_offset();
    ProgressAM {
        user_id: Set(user_id),
        issue_id: Set(issue_id.into()),
        last_page: Set(last_page),
        percent: Set(if finished { 100.0 } else { 50.0 }),
        finished: Set(finished),
        updated_at: Set(now),
        device: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
}

/// `/opds/v1/continue` returns issues the user has progress on with
/// `finished = false`. Finished or zero-progress issues are excluded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continue_feed_returns_in_progress_issues_only() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "continue@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Test").await;
    let mid = seed_issue_with_file(&db, lib_id, series_id, &tmp.path().join("a.cbz"), b"a").await;
    let done = seed_issue_with_file(&db, lib_id, series_id, &tmp.path().join("b.cbz"), b"b").await;
    let _unread =
        seed_issue_with_file(&db, lib_id, series_id, &tmp.path().join("c.cbz"), b"c").await;
    seed_progress(&db, auth.user_id, &mid, 5, false).await;
    seed_progress(&db, auth.user_id, &done, 20, true).await;

    let resp = get_with_auth(&app, "/opds/v1/continue", Header::Cookie(auth.cookies())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(body.contains(&format!("urn:issue:{mid}")));
    assert!(
        !body.contains(&format!("urn:issue:{done}")),
        "finished issue must not appear in Continue reading"
    );
    assert!(body.contains("<title>Continue reading</title>"));
}

/// `/opds/v1/new-this-month` returns issues created in the last 30
/// days. Older issues are excluded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_this_month_filters_by_created_at() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "new-month@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Recent").await;
    let recent_id = seed_issue_with_file(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("recent.cbz"),
        b"new",
    )
    .await;
    let old_id =
        seed_issue_with_file(&db, lib_id, series_id, &tmp.path().join("old.cbz"), b"old").await;
    // Backdate one issue to 60 days ago so it falls outside the
    // 30-day window.
    use entity::issue::Entity as IssueEntity;
    let row = IssueEntity::find_by_id(old_id.clone())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::issue::ActiveModel = row.into();
    let sixty = (Utc::now() - chrono::Duration::days(60)).fixed_offset();
    am.created_at = Set(sixty);
    am.update(&db).await.unwrap();

    let resp = get_with_auth(
        &app,
        "/opds/v1/new-this-month",
        Header::Cookie(auth.cookies()),
    )
    .await;
    let body = body_text(resp.into_body()).await;
    assert!(body.contains(&format!("urn:issue:{recent_id}")));
    assert!(
        !body.contains(&format!("urn:issue:{old_id}")),
        "issue older than 30 days must be excluded: {body}"
    );
}

/// `/opds/v1/by-creator/{writer}` returns every series that has the
/// writer in its `series_credits`. URL-encoded writer names round-
/// trip via the path segment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn by_creator_returns_writers_series() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "by-creator@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let s_match_1 = seed_series(&db, lib_id, "Saga").await;
    let s_match_2 = seed_series(&db, lib_id, "Y The Last Man").await;
    let s_other = seed_series(&db, lib_id, "Hawkeye").await;
    add_series_writer(&db, s_match_1, "Brian K. Vaughan").await;
    add_series_writer(&db, s_match_2, "Brian K. Vaughan").await;
    add_series_writer(&db, s_other, "Matt Fraction").await;

    let resp = get_with_auth(
        &app,
        "/opds/v1/by-creator/Brian%20K.%20Vaughan",
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(body.contains("<title>By Brian K. Vaughan</title>"));
    assert!(body.contains(&format!("urn:series:{s_match_1}")));
    assert!(body.contains(&format!("urn:series:{s_match_2}")));
    assert!(
        !body.contains(&format!("urn:series:{s_other}")),
        "non-matching series must not appear: {body}"
    );
}

/// Unknown writer name → empty feed (200 OK, zero entries), not 404.
/// Lets the M2 author drill-in link always click through coherently.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn by_creator_empty_feed_for_unknown_writer() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "by-creator-empty@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    let resp = get_with_auth(
        &app,
        "/opds/v1/by-creator/Nobody",
        Header::Cookie(auth.cookies()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(body.contains("<title>By Nobody</title>"));
    assert!(!body.contains("<entry>"));
}

/// M5 drill-in plumbing: when a series has writer credits, its
/// `<author>` element carries a `<uri>` pointing at /opds/v1/by-
/// creator/{name}. The URI is URL-encoded so writer names with
/// spaces don't produce broken hrefs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_author_links_to_by_creator_feed() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "author-drill@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Pride of Baghdad").await;
    add_series_writer(&db, series_id, "Brian K. Vaughan").await;

    let resp = get_with_auth(&app, "/opds/v1/series", Header::Cookie(auth.cookies())).await;
    let body = body_text(resp.into_body()).await;
    assert!(body.contains(
        "<author><name>Brian K. Vaughan</name><uri>/opds/v1/by-creator/Brian%20K.%20Vaughan</uri></author>"
    ));
}

/// M5 follow-up: per-series feed at `/opds/v1/series/{id}` carries
/// the series metadata at the FEED root (Panels and other OPDS
/// clients display this as a header banner). The all-series list
/// already had this metadata on each entry from M2; without it on
/// the per-series feed too, the detail screen shows only "title +
/// issue grid" — observed in Panels on 2026-05-16.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_detail_feed_carries_banner_metadata() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "series-banner@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "Saga").await;
    set_series_meta(&db, series_id, Some("Image Comics"), Some(2012), Some("en")).await;
    add_series_writer(&db, series_id, "Brian K. Vaughan").await;
    add_series_genre(&db, series_id, "Science Fiction").await;
    let issue_id = seed_issue_with_file(
        &db,
        lib_id,
        series_id,
        &tmp.path().join("saga-1.cbz"),
        b"first",
    )
    .await;

    let resp = get_with_auth(
        &app,
        &format!("/opds/v1/series/{series_id}"),
        Header::Cookie(auth.cookies()),
    )
    .await;
    let body = body_text(resp.into_body()).await;
    // Feed-root metadata block — before any `<entry>`.
    let first_entry = body.find("<entry>").unwrap_or(body.len());
    let header = &body[..first_entry];
    assert!(
        header.contains("xmlns:dc="),
        "dc namespace required: {header}"
    );
    assert!(header.contains("<dc:publisher>Image Comics</dc:publisher>"));
    assert!(header.contains("<dc:issued>2012</dc:issued>"));
    assert!(header.contains("<dc:language>en</dc:language>"));
    assert!(header.contains("<author><name>Brian K. Vaughan</name>"));
    assert!(header.contains(r#"term="Science Fiction""#));
    // Cover image rels at the feed root drive the banner cover.
    assert!(header.contains(&format!(r#"href="/issues/{issue_id}/pages/0/thumb""#)));
    assert!(header.contains(&format!(r#"href="/issues/{issue_id}/pages/0""#)));
}
