//! Integration tests for OPDS progress sync (M7).
//!
//! Covers:
//!  - scope enforcement: `read`-scope app password → 403 on progress PUT
//!  - `read+progress`-scope app password → 200; row visible via /progress
//!  - cookie session always passes (scope check skipped for interactive auth)
//!  - KOReader sync shim: percentage → page conversion, hash → issue.id
//!    lookup, mismatched body.document rejection, unknown hash → 401
//!  - audit log: every progress write lands one `opds.progress.write` row

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
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Database, DatabaseConnection, EntityTrait, QueryFilter, Set,
};
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

async fn mint_token(app: &TestApp, auth: &Authed, label: &str, scope: Option<&str>) -> String {
    let body = match scope {
        Some(s) => format!(r#"{{"label":"{label}","scope":"{s}"}}"#),
        None => format!(r#"{{"label":"{label}"}}"#),
    };
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
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json: serde_json::Value = body_json(resp.into_body()).await;
    if let Some(expected) = scope {
        assert_eq!(json["scope"].as_str().unwrap(), expected);
    } else {
        assert_eq!(
            json["scope"].as_str().unwrap(),
            "read",
            "default scope is read"
        );
    }
    json["plaintext"].as_str().unwrap().to_owned()
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

async fn put_progress(
    app: &TestApp,
    bearer: &str,
    issue_id: &str,
    body: serde_json::Value,
) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/opds/v1/issues/{issue_id}/progress"))
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn put_progress_cookie(
    app: &TestApp,
    auth: &Authed,
    issue_id: &str,
    body: serde_json::Value,
) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/opds/v1/issues/{issue_id}/progress"))
                .header(header::COOKIE, auth.cookies())
                .header("x-csrf-token", &auth.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_scope_token_rejected_with_403() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "rscope@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "S").await;
    let issue_id = seed_issue(&db, lib_id, series_id, &tmp.path().join("a.cbz"), b"a", 10).await;

    // Default scope is `read`.
    let token = mint_token(&app, &auth, "read-only", None).await;
    let resp = put_progress(&app, &token, &issue_id, serde_json::json!({ "page": 4 })).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_progress_scope_token_writes_progress() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "rwscope@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "S").await;
    let issue_id = seed_issue(&db, lib_id, series_id, &tmp.path().join("a.cbz"), b"a", 10).await;

    let token = mint_token(&app, &auth, "kindle", Some("read+progress")).await;
    let resp = put_progress(
        &app,
        &token,
        &issue_id,
        serde_json::json!({ "page": 7, "finished": false, "device": "Chunky/iPad" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["page"], 7);
    assert_eq!(json["finished"], false);
    // 7/10 = 0.7
    assert!(
        (json["percent"].as_f64().unwrap() - 0.7).abs() < 0.001,
        "got percent {}",
        json["percent"]
    );

    // The row is visible via /progress (the standard reader pathway).
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/progress")
                .header(header::COOKIE, auth.cookies())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    let records = json["records"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["issue_id"].as_str().unwrap(), issue_id);
    assert_eq!(records[0]["page"], 7);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cookie_session_bypasses_scope_check() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cscope@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "S").await;
    let issue_id = seed_issue(&db, lib_id, series_id, &tmp.path().join("a.cbz"), b"a", 10).await;

    let resp = put_progress_cookie(&app, &auth, &issue_id, serde_json::json!({ "page": 3 })).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "interactive auth has implicit full capability"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opds_v2_progress_endpoint_works() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "v2prog@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "S").await;
    let issue_id = seed_issue(&db, lib_id, series_id, &tmp.path().join("a.cbz"), b"a", 10).await;

    let token = mint_token(&app, &auth, "v2", Some("read+progress")).await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/opds/v2/issues/{issue_id}/progress"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"page":2}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn koreader_shim_converts_percentage_to_page() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ko@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "S").await;
    let issue_id = seed_issue(&db, lib_id, series_id, &tmp.path().join("a.cbz"), b"a", 100).await;

    let token = mint_token(&app, &auth, "koreader", Some("read+progress")).await;
    // KOReader sends Basic credentials in practice. Test both surfaces:
    // Bearer (modern) and Basic (legacy).
    let creds = base64::engine::general_purpose::STANDARD.encode(format!("ignored:{token}"));
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/opds/v1/syncs/progress/{issue_id}"))
                .header(header::AUTHORIZATION, format!("Basic {creds}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "document": issue_id,
                        "progress": "/body/DocFragment[1]/body/p[3]/text().0",
                        "percentage": 0.42,
                        "device": "KOReader",
                        "device_id": "abc-123"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["document"].as_str().unwrap(), issue_id);
    assert!(json["timestamp"].is_number());

    // 0.42 * 100 = 42.
    let prog = entity::progress_record::Entity::find_by_id((auth.user_id, issue_id.clone()))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prog.last_page, 42);
    assert!(!prog.finished);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn koreader_shim_marks_finished_at_100pct() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ko-fin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "S").await;
    let issue_id = seed_issue(&db, lib_id, series_id, &tmp.path().join("a.cbz"), b"a", 50).await;

    let token = mint_token(&app, &auth, "kofin", Some("read+progress")).await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/opds/v1/syncs/progress/{issue_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"percentage": 1.0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let prog = entity::progress_record::Entity::find_by_id((auth.user_id, issue_id.clone()))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(prog.finished, "1.0 percentage marks finished");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn koreader_shim_rejects_unknown_hash_with_401() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ko-bad@example.com").await;

    let token = mint_token(&app, &auth, "kobad", Some("read+progress")).await;
    let bogus = "0".repeat(64);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/opds/v1/syncs/progress/{bogus}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"percentage": 0.5}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn koreader_shim_rejects_mismatched_document_body() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ko-mm@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "S").await;
    let issue_id = seed_issue(&db, lib_id, series_id, &tmp.path().join("a.cbz"), b"a", 10).await;
    let token = mint_token(&app, &auth, "ko-mm", Some("read+progress")).await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/opds/v1/syncs/progress/{issue_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"document":"different","percentage":0.5}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn progress_write_audit_logged() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "audit@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib_id = seed_library(&db, tmp.path()).await;
    let series_id = seed_series(&db, lib_id, "S").await;
    let issue_id = seed_issue(&db, lib_id, series_id, &tmp.path().join("a.cbz"), b"a", 10).await;
    let token = mint_token(&app, &auth, "audit", Some("read+progress")).await;

    let resp = put_progress(&app, &token, &issue_id, serde_json::json!({ "page": 1 })).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let rows = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("opds.progress.write"))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor_id, auth.user_id);
    assert_eq!(rows[0].target_type.as_deref(), Some("issue"));
    assert_eq!(rows[0].target_id.as_deref(), Some(issue_id.as_str()));
}
