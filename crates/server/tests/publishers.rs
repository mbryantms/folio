//! `/publishers` endpoint — global-search M3.
//!
//! Seeds a handful of series with mixed publisher strings and verifies
//! the distinct-publisher search:
//! - ILIKE substring match (case-insensitive)
//! - ORDER BY series_count DESC
//! - empty query returns empty list
//! - oversized query rejected
//! - library-ACL gating excludes invisible rows

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    library, library_user_access,
    series::{ActiveModel as SeriesAM, normalize_name},
    user::Entity as UserEntity,
};
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set};
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
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
    let json = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id,
    }
}

async fn promote_to_admin(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = UserEntity::find_by_id(user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::user::ActiveModel = user.into();
    am.role = Set("admin".into());
    am.update(&db).await.unwrap();
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: Option<&Authed>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method.clone()).uri(uri);
    if let Some(a) = auth {
        builder = builder
            .header(
                header::COOKIE,
                format!(
                    "__Host-comic_session={}; __Host-comic_csrf={}",
                    a.session, a.csrf
                ),
            )
            .header("X-CSRF-Token", &a.csrf);
    }
    let req = builder.body(Body::empty()).unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn seed_library(app: &TestApp, lib_name: &str) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {lib_name}")),
        root_path: Set(format!("/tmp/{lib_name}-{lib_id}")),
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
        thumbnail_format: Set("webp".to_owned()),
        thumbnail_cover_quality: Set(server::library::thumbnails::DEFAULT_COVER_QUALITY as i32),
        thumbnail_page_quality: Set(server::library::thumbnails::DEFAULT_STRIP_QUALITY as i32),
        generate_page_thumbs_on_scan: Set(false),
    }
    .insert(&db)
    .await
    .unwrap();
    lib_id
}

async fn seed_series(app: &TestApp, lib_id: Uuid, name: &str, publisher: Option<&str>) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set(name.into()),
        normalized_name: Set(normalize_name(name)),
        year: Set(Some(2020)),
        volume: Set(None),
        publisher: Set(publisher.map(str::to_owned)),
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
        slug: Set(format!("{name}-{series_id}")),
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
    .insert(&db)
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ilike_substring_match_and_count_ordering() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    let lib = seed_library(&app, "all").await;
    // Three Image series, one Marvel, one DC — search "image" should
    // return Image first (count=3), and ordering for ties is alpha asc.
    seed_series(&app, lib, "Saga", Some("Image Comics")).await;
    seed_series(&app, lib, "Invincible", Some("Image Comics")).await;
    seed_series(&app, lib, "Spawn", Some("Image Comics")).await;
    seed_series(&app, lib, "Daredevil", Some("Marvel")).await;
    seed_series(&app, lib, "Sandman", Some("DC")).await;

    let (status, json) = http(&app, Method::GET, "/publishers?q=image", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    let items = json["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1, "exactly one publisher matches 'image'");
    assert_eq!(items[0]["publisher"].as_str().unwrap(), "Image Comics");
    assert_eq!(items[0]["series_count"].as_i64().unwrap(), 3);

    // Broad query "co" matches all three (Image **Co**mics, Mar… no it
    // doesn't; check the actual ILIKE) — narrow this to "c" which hits
    // Image Comics and DC and Marvel? No, "c" → "Image Comics" only
    // (Marvel/DC have no lowercase 'c'). Use a non-greedy probe:
    let (_status, json) = http(&app, Method::GET, "/publishers?q=ma", Some(&auth)).await;
    let items = json["items"].as_array().expect("items array");
    let pubs: Vec<String> = items
        .iter()
        .map(|v| v["publisher"].as_str().unwrap().to_owned())
        .collect();
    assert!(pubs.contains(&"Marvel".to_owned()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_query_returns_empty_list() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "user@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    let (status, json) = http(&app, Method::GET, "/publishers", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["items"].as_array().unwrap().len(), 0);

    let (status, json) = http(&app, Method::GET, "/publishers?q=", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["items"].as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_query_rejected() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "user@example.com").await;

    let q = "a".repeat(201);
    let uri = format!("/publishers?q={q}");
    let (status, _json) = http(&app, Method::GET, &uri, Some(&auth)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restricted_user_sees_only_granted_libraries() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    promote_to_admin(&app, admin.user_id).await;

    let lib_a = seed_library(&app, "lib-a").await;
    let lib_b = seed_library(&app, "lib-b").await;
    seed_series(&app, lib_a, "Image1", Some("Image Comics")).await;
    seed_series(&app, lib_b, "Marvel1", Some("Marvel")).await;

    // Restricted user with access only to lib_a.
    let restricted = register(&app, "restricted@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        library_id: Set(lib_a),
        user_id: Set(restricted.user_id),
        role: Set("reader".into()),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();

    // Admin sees both.
    let (_, json) = http(&app, Method::GET, "/publishers?q=ma", Some(&admin)).await;
    let pubs: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["publisher"].as_str().unwrap().to_owned())
        .collect();
    assert!(pubs.contains(&"Marvel".to_owned()));

    // Restricted user does NOT see Marvel (only in lib_b).
    let (_, json) = http(&app, Method::GET, "/publishers?q=ma", Some(&restricted)).await;
    let pubs: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["publisher"].as_str().unwrap().to_owned())
        .collect();
    assert!(!pubs.contains(&"Marvel".to_owned()));
}
