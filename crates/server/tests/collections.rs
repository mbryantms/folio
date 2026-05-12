//! Markers + Collections M2 — collections CRUD + entries integration.

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
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, Database, Set};
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
    #[allow(dead_code)]
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

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: Option<&Authed>,
    body: Option<serde_json::Value>,
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
    let req = if let Some(b) = body {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// Insert a library, one series, and one active issue belonging to it.
/// Returns the IDs so tests can ref them as entries.
async fn seed_series_with_issue(app: &TestApp, slug: &str) -> (Uuid, Uuid, String) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let series_id = Uuid::now_v7();
    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), 0u8);
    let now = Utc::now().fixed_offset();

    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {slug}")),
        root_path: Set(format!("/tmp/{slug}-{lib_id}")),
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

    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set(slug.into()),
        normalized_name: Set(normalize_name(slug)),
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
        slug: Set(slug.into()),
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

    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(format!("{slug}-1")),
        file_path: Set(format!("/tmp/{slug}.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
        title: Set(None),
        sort_number: Set(Some(1.0)),
        number_raw: Set(Some("1".into())),
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

    (lib_id, series_id, issue_id)
}

async fn create_collection(app: &TestApp, auth: &Authed, name: &str) -> Uuid {
    let (status, v) = http(
        app,
        Method::POST,
        "/me/collections",
        Some(auth),
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {v:#?}");
    Uuid::parse_str(v["id"].as_str().unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_seeds_want_to_read_idempotently() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "alice@example.com").await;

    let (status, items) = http(&app, Method::GET, "/me/collections", Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let arr = items.as_array().unwrap();
    // Exactly one row on a fresh user — the seeded Want to Read.
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "Want to Read");
    assert_eq!(arr[0]["system_key"], "want_to_read");
    assert_eq!(arr[0]["kind"], "collection");
    // M3 surface: WTR is reachable via the hardcoded Browse entry in
    // `main-nav.ts`, so the auto-seed leaves `show_in_sidebar = false`
    // to avoid a duplicate row under the "Saved views" sidebar
    // section. The user can opt in via /settings/views.
    assert_eq!(arr[0]["show_in_sidebar"], false);

    // Second call doesn't duplicate it.
    let (_, items2) = http(&app, Method::GET, "/me/collections", Some(&auth), None).await;
    let arr2 = items2.as_array().unwrap();
    assert_eq!(arr2.len(), 1);
    assert_eq!(arr2[0]["id"], arr[0]["id"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_patch_delete_round_trip() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bob@example.com").await;

    let (status, view) = http(
        &app,
        Method::POST,
        "/me/collections",
        Some(&auth),
        Some(serde_json::json!({ "name": "My Capes", "description": "Cape comics." })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {view:#?}");
    let id = view["id"].as_str().unwrap().to_owned();
    assert_eq!(view["name"], "My Capes");
    assert_eq!(view["description"], "Cape comics.");
    assert_eq!(view["kind"], "collection");

    // PATCH name + clear description by sending empty string (codebase
    // convention — the trim/filter path turns "" into None).
    let url = format!("/me/collections/{id}");
    let (status, patched) = http(
        &app,
        Method::PATCH,
        &url,
        Some(&auth),
        Some(serde_json::json!({ "name": "Capes & Crooks", "description": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "patch: {patched:#?}");
    assert_eq!(patched["name"], "Capes & Crooks");
    assert!(patched["description"].is_null());

    // DELETE removes it.
    let (status, _) = http(&app, Method::DELETE, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = http(
        &app,
        Method::GET,
        &format!("{url}/entries"),
        Some(&auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn want_to_read_cannot_be_deleted() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "claire@example.com").await;
    // Seed WTR.
    let (_, items) = http(&app, Method::GET, "/me/collections", Some(&auth), None).await;
    let wtr_id = items[0]["id"].as_str().unwrap().to_owned();
    let url = format!("/me/collections/{wtr_id}");
    let (status, body) = http(&app, Method::DELETE, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body:#?}");
    assert_eq!(body["error"]["code"], "want_to_read_undeletable");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_series_and_issue_entries_mixed() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "dan@example.com").await;
    let (_lib, series_id, issue_id) = seed_series_with_issue(&app, "test-series").await;
    let cid = create_collection(&app, &auth, "Pile").await;

    // Add a series entry.
    let url = format!("/me/collections/{cid}/entries");
    let (status, body) = http(
        &app,
        Method::POST,
        &url,
        Some(&auth),
        Some(serde_json::json!({ "entry_kind": "series", "ref_id": series_id.to_string() })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "add series: {body:#?}");
    assert_eq!(body["entry_kind"], "series");
    assert_eq!(body["series"]["id"], series_id.to_string());
    assert_eq!(body["position"], 0);
    assert!(body["issue"].is_null());

    // Add an issue entry to the same collection.
    let (status, body) = http(
        &app,
        Method::POST,
        &url,
        Some(&auth),
        Some(serde_json::json!({ "entry_kind": "issue", "ref_id": issue_id })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "add issue: {body:#?}");
    assert_eq!(body["entry_kind"], "issue");
    assert_eq!(body["position"], 1);
    assert!(body["issue"].is_object());
    assert!(body["series"].is_null());

    // GET returns both, ordered by position.
    let (status, list) = http(&app, Method::GET, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["entry_kind"], "series");
    assert_eq!(items[1]["entry_kind"], "issue");
    assert_eq!(list["total"], 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_entry_idempotent_returns_409() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "eve@example.com").await;
    let (_lib, series_id, _issue) = seed_series_with_issue(&app, "ipx").await;
    let cid = create_collection(&app, &auth, "Pile").await;
    let url = format!("/me/collections/{cid}/entries");

    let body = serde_json::json!({ "entry_kind": "series", "ref_id": series_id.to_string() });
    let (status, _) = http(&app, Method::POST, &url, Some(&auth), Some(body.clone())).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, retry) = http(&app, Method::POST, &url, Some(&auth), Some(body)).await;
    assert_eq!(status, StatusCode::CONFLICT, "retry: {retry:#?}");
    assert_eq!(retry["error"]["code"], "already_in_collection");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_entry_validation_errors() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "frank@example.com").await;
    let cid = create_collection(&app, &auth, "Pile").await;
    let url = format!("/me/collections/{cid}/entries");

    // Unknown entry_kind.
    let (status, _) = http(
        &app,
        Method::POST,
        &url,
        Some(&auth),
        Some(serde_json::json!({ "entry_kind": "blob", "ref_id": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Bad UUID for series.
    let (status, _) = http(
        &app,
        Method::POST,
        &url,
        Some(&auth),
        Some(serde_json::json!({ "entry_kind": "series", "ref_id": "not-a-uuid" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Nonexistent series.
    let (status, body) = http(
        &app,
        Method::POST,
        &url,
        Some(&auth),
        Some(serde_json::json!({ "entry_kind": "series", "ref_id": Uuid::now_v7().to_string() })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_entry_clears_row() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "grace@example.com").await;
    let (_lib, series_id, _issue) = seed_series_with_issue(&app, "grace").await;
    let cid = create_collection(&app, &auth, "Pile").await;
    let url = format!("/me/collections/{cid}/entries");

    let (_, entry) = http(
        &app,
        Method::POST,
        &url,
        Some(&auth),
        Some(serde_json::json!({ "entry_kind": "series", "ref_id": series_id.to_string() })),
    )
    .await;
    let entry_id = entry["id"].as_str().unwrap().to_owned();

    let del = format!("/me/collections/{cid}/entries/{entry_id}");
    let (status, _) = http(&app, Method::DELETE, &del, Some(&auth), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, list) = http(&app, Method::GET, &url, Some(&auth), None).await;
    assert!(list["items"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reorder_rewrites_positions_in_tx() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "henry@example.com").await;
    let (_lib1, s1, _i1) = seed_series_with_issue(&app, "alpha").await;
    let (_lib2, s2, _i2) = seed_series_with_issue(&app, "bravo").await;
    let (_lib3, s3, _i3) = seed_series_with_issue(&app, "charlie").await;
    let cid = create_collection(&app, &auth, "Reorder Me").await;
    let entries_url = format!("/me/collections/{cid}/entries");

    let mut ids = Vec::new();
    for s in [s1, s2, s3] {
        let (_, e) = http(
            &app,
            Method::POST,
            &entries_url,
            Some(&auth),
            Some(serde_json::json!({ "entry_kind": "series", "ref_id": s.to_string() })),
        )
        .await;
        ids.push(e["id"].as_str().unwrap().to_owned());
    }

    // Reverse the order.
    let reorder_url = format!("/me/collections/{cid}/entries/reorder");
    let reversed: Vec<String> = ids.iter().rev().cloned().collect();
    let (status, body) = http(
        &app,
        Method::POST,
        &reorder_url,
        Some(&auth),
        Some(serde_json::json!({ "entry_ids": reversed })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "body: {body:#?}");

    let (_, list) = http(&app, Method::GET, &entries_url, Some(&auth), None).await;
    let positions: Vec<i64> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["position"].as_i64().unwrap())
        .collect();
    assert_eq!(
        positions,
        vec![0, 1, 2],
        "positions compacted after reorder"
    );
    let observed_ids: Vec<String> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(observed_ids, reversed, "reorder applied");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reorder_rejects_partial_lists() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "ivy@example.com").await;
    let (_lib1, s1, _i1) = seed_series_with_issue(&app, "x1").await;
    let (_lib2, s2, _i2) = seed_series_with_issue(&app, "x2").await;
    let cid = create_collection(&app, &auth, "Reorder").await;
    let entries_url = format!("/me/collections/{cid}/entries");

    let mut ids = Vec::new();
    for s in [s1, s2] {
        let (_, e) = http(
            &app,
            Method::POST,
            &entries_url,
            Some(&auth),
            Some(serde_json::json!({ "entry_kind": "series", "ref_id": s.to_string() })),
        )
        .await;
        ids.push(e["id"].as_str().unwrap().to_owned());
    }

    let url = format!("/me/collections/{cid}/entries/reorder");
    let (status, body) = http(
        &app,
        Method::POST,
        &url,
        Some(&auth),
        Some(serde_json::json!({ "entry_ids": [ids[0].clone()] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_user_collection_access_denied() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice2@example.com").await;
    let bob = register(&app, "bob2@example.com").await;
    let cid = create_collection(&app, &alice, "Alice's pile").await;

    // Bob can't read alice's collection entries.
    let url = format!("/me/collections/{cid}/entries");
    let (status, body) = http(&app, Method::GET, &url, Some(&bob), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body:#?}");

    // Bob can't patch it.
    let patch_url = format!("/me/collections/{cid}");
    let (status, _) = http(
        &app,
        Method::PATCH,
        &patch_url,
        Some(&bob),
        Some(serde_json::json!({ "name": "Stolen" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Bob can't delete it either.
    let (status, _) = http(&app, Method::DELETE, &patch_url, Some(&bob), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn csrf_required_on_mutations() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "kara@example.com").await;
    // Cookie-bound POST without the X-CSRF-Token header is rejected.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/me/collections")
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":"x"}"#))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_views_results_returns_empty_stub_for_collections() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "lena@example.com").await;
    let cid = create_collection(&app, &auth, "Stub").await;
    let url = format!("/me/saved-views/{cid}/results");
    let (status, body) = http(&app, Method::GET, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK, "body: {body:#?}");
    assert!(body["items"].as_array().unwrap().is_empty());
    assert_eq!(body["total"], 0);
}
