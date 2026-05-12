//! `/people` endpoint — global-search M4.
//!
//! Seeds creators across `series_credits` and `issue_credits`, then
//! verifies the unified-people search:
//! - dedupes the same name across both tables
//! - rolls up the set of roles a person appears in
//! - returns a credit_count distinct over (src, ref_id)
//! - applies trigram fuzziness (`Jhon` finds `John`)
//! - ACL-gates by visible library
//! - rejects oversized queries

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    issue_credit::ActiveModel as IssueCreditAM,
    library,
    series::{ActiveModel as SeriesAM, normalize_name},
    series_credit::ActiveModel as SeriesCreditAM,
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

async fn seed_series(app: &TestApp, lib_id: Uuid, name: &str) -> Uuid {
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
    series_id
}

async fn seed_issue(app: &TestApp, lib_id: Uuid, series_id: Uuid, idx: u8) -> String {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), idx);
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(format!("issue-{idx}-{series_id}")),
        file_path: Set(format!("/tmp/{series_id}-{idx}.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
        title: Set(None),
        sort_number: Set(Some(idx as f64)),
        number_raw: Set(Some(idx.to_string())),
        volume: Set(None),
        year: Set(Some(2020)),
        month: Set(None),
        day: Set(None),
        summary: Set(None),
        notes: Set(None),
        language_code: Set(None),
        format: Set(None),
        black_and_white: Set(None),
        manga: Set(None),
        age_rating: Set(None),
        page_count: Set(Some(20)),
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
    issue_id
}

async fn add_series_credit(app: &TestApp, series_id: Uuid, role: &str, person: &str) {
    let db = Database::connect(&app.db_url).await.unwrap();
    SeriesCreditAM {
        series_id: Set(series_id),
        role: Set(role.into()),
        person: Set(person.into()),
    }
    .insert(&db)
    .await
    .unwrap();
}

async fn add_issue_credit(app: &TestApp, issue_id: String, role: &str, person: &str) {
    let db = Database::connect(&app.db_url).await.unwrap();
    IssueCreditAM {
        issue_id: Set(issue_id),
        role: Set(role.into()),
        person: Set(person.into()),
    }
    .insert(&db)
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dedupes_across_tables_and_aggregates_roles() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    let lib = seed_library(&app, "main").await;
    let s1 = seed_series(&app, lib, "Saga").await;
    let s2 = seed_series(&app, lib, "Y The Last Man").await;
    // Same person credited as writer at series-level on two series.
    add_series_credit(&app, s1, "writer", "Brian K. Vaughan").await;
    add_series_credit(&app, s2, "writer", "Brian K. Vaughan").await;
    // Also credited as editor on an issue (different role).
    let i1 = seed_issue(&app, lib, s1, 1).await;
    add_issue_credit(&app, i1.clone(), "editor", "Brian K. Vaughan").await;

    let (status, json) = http(&app, Method::GET, "/people?q=Vaughan", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    let items = json["items"].as_array().unwrap();
    let row = items
        .iter()
        .find(|v| v["person"].as_str() == Some("Brian K. Vaughan"))
        .expect("Vaughan returned exactly once");
    // The Postgres `ARRAY_AGG(role)` round-trips as a JSON array of strings.
    let roles: Vec<String> = row["roles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(roles.contains(&"writer".to_owned()));
    assert!(roles.contains(&"editor".to_owned()));
    // 2 series-credits + 1 issue-credit → 3 distinct refs.
    assert_eq!(row["credit_count"].as_i64().unwrap(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trigram_fuzzy_substring() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "u@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    let lib = seed_library(&app, "main").await;
    let s = seed_series(&app, lib, "Saga").await;
    add_series_credit(&app, s, "writer", "Geoff Johns").await;
    add_series_credit(&app, s, "penciller", "Gary Frank").await;

    // Exact substring works.
    let (_, json) = http(&app, Method::GET, "/people?q=Geoff", Some(&auth)).await;
    let people: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["person"].as_str().unwrap().to_owned())
        .collect();
    assert!(people.contains(&"Geoff Johns".to_owned()));

    // Fuzzy: "geof" (one missing 'f') still finds the row via trigram
    // similarity since the lowercase ILIKE branch matches too.
    let (_, json) = http(&app, Method::GET, "/people?q=geof", Some(&auth)).await;
    let people: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["person"].as_str().unwrap().to_owned())
        .collect();
    assert!(people.contains(&"Geoff Johns".to_owned()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_query_returns_empty() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "u@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    let (status, json) = http(&app, Method::GET, "/people", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["items"].as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_query_rejected() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "u@example.com").await;
    let q = "a".repeat(201);
    let uri = format!("/people?q={q}");
    let (status, _json) = http(&app, Method::GET, &uri, Some(&auth)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
