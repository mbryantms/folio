//! Markers + Collections M5 — `/me/markers` + per-issue endpoint coverage.

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
    library_user_access::ActiveModel as LibraryAccessAM,
    series::{ActiveModel as SeriesAM, normalize_name},
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

/// Seed a library + series + active issue with page_count=20. Returns
/// (library_id, series_id, issue_id).
async fn seed_issue(app: &TestApp, slug: &str) -> (Uuid, Uuid, String) {
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

/// Grant a non-admin user explicit access to `library_id`. Required
/// because the markers ACL falls back to library_user_access when the
/// caller isn't an admin.
async fn grant_library(app: &TestApp, user_id: Uuid, library_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    LibraryAccessAM {
        user_id: Set(user_id),
        library_id: Set(library_id),
        role: Set("reader".into()),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_each_kind_and_list_per_issue() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "alice@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "alpha").await;

    let bookmark = serde_json::json!({
        "issue_id": issue_id,
        "page_index": 0,
        "kind": "bookmark",
    });
    let (status, _) = http(
        &app,
        Method::POST,
        "/me/markers",
        Some(&auth),
        Some(bookmark),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let note = serde_json::json!({
        "issue_id": issue_id,
        "page_index": 1,
        "kind": "note",
        "body": "Great panel.",
    });
    let (status, _) = http(&app, Method::POST, "/me/markers", Some(&auth), Some(note)).await;
    assert_eq!(status, StatusCode::CREATED);

    // Starred bookmark — favorite is now a flag on any kind, not its
    // own kind.
    let starred = serde_json::json!({
        "issue_id": issue_id,
        "page_index": 2,
        "kind": "bookmark",
        "is_favorite": true,
    });
    let (status, _) = http(
        &app,
        Method::POST,
        "/me/markers",
        Some(&auth),
        Some(starred),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let highlight = serde_json::json!({
        "issue_id": issue_id,
        "page_index": 3,
        "kind": "highlight",
        "region": { "x": 10, "y": 20, "w": 30, "h": 15, "shape": "rect" },
    });
    let (status, _) = http(
        &app,
        Method::POST,
        "/me/markers",
        Some(&auth),
        Some(highlight),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let url = format!("/me/issues/{issue_id}/markers");
    let (status, list) = http(&app, Method::GET, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    let kinds: Vec<String> = items
        .iter()
        .map(|m| m["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(kinds.contains(&"bookmark".to_owned()));
    assert!(kinds.contains(&"note".to_owned()));
    assert!(kinds.contains(&"highlight".to_owned()));
    // Exactly one row should carry the favorite flag.
    let starred_count = items
        .iter()
        .filter(|m| m["is_favorite"].as_bool() == Some(true))
        .count();
    assert_eq!(starred_count, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_validates_shape() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bob@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "bob-lib").await;

    // Note without body → 422.
    let bad = serde_json::json!({
        "issue_id": issue_id,
        "page_index": 0,
        "kind": "note",
    });
    let (status, body) = http(&app, Method::POST, "/me/markers", Some(&auth), Some(bad)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body:#?}");
    assert_eq!(body["error"]["code"], "validation");

    // Highlight without region → 422.
    let bad = serde_json::json!({
        "issue_id": issue_id,
        "page_index": 0,
        "kind": "highlight",
    });
    let (status, _) = http(&app, Method::POST, "/me/markers", Some(&auth), Some(bad)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Unknown kind → 400.
    let bad = serde_json::json!({
        "issue_id": issue_id,
        "page_index": 0,
        "kind": "scribble",
    });
    let (status, _) = http(&app, Method::POST, "/me/markers", Some(&auth), Some(bad)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // page_index beyond page_count (20) → 422.
    let bad = serde_json::json!({
        "issue_id": issue_id,
        "page_index": 21,
        "kind": "bookmark",
    });
    let (status, _) = http(&app, Method::POST, "/me/markers", Some(&auth), Some(bad)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn region_clamping_keeps_values_in_range() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "claire@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "claire-lib").await;

    // Client sends out-of-range x and w; server clamps to [0, 100].
    let body = serde_json::json!({
        "issue_id": issue_id,
        "page_index": 0,
        "kind": "highlight",
        "region": { "x": -5, "y": 50, "w": 200, "h": 10, "shape": "rect" },
    });
    let (status, marker) = http(&app, Method::POST, "/me/markers", Some(&auth), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "marker: {marker:#?}");
    assert_eq!(marker["region"]["x"], 0.0);
    assert_eq!(marker["region"]["w"], 100.0);
    assert_eq!(marker["region"]["y"], 50.0);
    assert_eq!(marker["region"]["h"], 10.0);
    assert_eq!(marker["region"]["shape"], "rect");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_partial_diffs_preserve_invariants() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "dan@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "dan-lib").await;

    let (_, m) = http(
        &app,
        Method::POST,
        "/me/markers",
        Some(&auth),
        Some(serde_json::json!({
            "issue_id": issue_id,
            "page_index": 0,
            "kind": "note",
            "body": "initial",
        })),
    )
    .await;
    let id = m["id"].as_str().unwrap().to_owned();
    let url = format!("/me/markers/{id}");

    // Edit body.
    let (status, updated) = http(
        &app,
        Method::PATCH,
        &url,
        Some(&auth),
        Some(serde_json::json!({ "body": "revised" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["body"], "revised");

    // Try to clear note body → server rejects (per-kind invariant).
    let (status, _) = http(
        &app,
        Method::PATCH,
        &url,
        Some(&auth),
        Some(serde_json::json!({ "body": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Set color separately.
    let (status, updated) = http(
        &app,
        Method::PATCH,
        &url,
        Some(&auth),
        Some(serde_json::json!({ "color": "amber" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["color"], "amber");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_filters_by_kind_q_and_cursor() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "eve@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "eve-lib").await;

    // Seed: 3 bookmarks, 2 notes with searchable bodies.
    for p in 0..3 {
        http(
            &app,
            Method::POST,
            "/me/markers",
            Some(&auth),
            Some(serde_json::json!({
                "issue_id": issue_id,
                "page_index": p,
                "kind": "bookmark",
            })),
        )
        .await;
    }
    for (p, body) in [(3, "moonlight thoughts"), (4, "panel about lasers")] {
        http(
            &app,
            Method::POST,
            "/me/markers",
            Some(&auth),
            Some(serde_json::json!({
                "issue_id": issue_id,
                "page_index": p,
                "kind": "note",
                "body": body,
            })),
        )
        .await;
    }

    // Filter by kind=note.
    let (_, list) = http(
        &app,
        Method::GET,
        "/me/markers?kind=note",
        Some(&auth),
        None,
    )
    .await;
    assert_eq!(list["items"].as_array().unwrap().len(), 2);

    // Free-text search across body.
    let (_, list) = http(&app, Method::GET, "/me/markers?q=lasers", Some(&auth), None).await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0]["body"].as_str().unwrap().contains("lasers"));

    // Pagination: small limit returns a next_cursor; second page fills.
    let (_, page1) = http(&app, Method::GET, "/me/markers?limit=2", Some(&auth), None).await;
    let cursor = page1["next_cursor"].as_str().unwrap().to_owned();
    assert_eq!(page1["items"].as_array().unwrap().len(), 2);
    let url = format!("/me/markers?limit=2&cursor={cursor}");
    let (_, page2) = http(&app, Method::GET, &url, Some(&auth), None).await;
    assert!(!page2["items"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_user_isolation() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice2@example.com").await;
    let bob = register(&app, "bob2@example.com").await;
    promote_to_admin(&app, alice.user_id).await;
    let (lib, _series, issue_id) = seed_issue(&app, "x-lib").await;
    grant_library(&app, bob.user_id, lib).await;

    let (_, m) = http(
        &app,
        Method::POST,
        "/me/markers",
        Some(&alice),
        Some(serde_json::json!({
            "issue_id": issue_id,
            "page_index": 0,
            "kind": "bookmark",
        })),
    )
    .await;
    let id = m["id"].as_str().unwrap().to_owned();

    // Bob can see his own (empty) feed but not alice's marker.
    let url = format!("/me/issues/{issue_id}/markers");
    let (status, list) = http(&app, Method::GET, &url, Some(&bob), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(list["items"].as_array().unwrap().is_empty());

    // Bob can't patch alice's marker by id either.
    let url = format!("/me/markers/{id}");
    let (status, _) = http(
        &app,
        Method::PATCH,
        &url,
        Some(&bob),
        Some(serde_json::json!({ "color": "red" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Or delete it.
    let (status, _) = http(&app, Method::DELETE, &url, Some(&bob), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_issue_endpoint_acl_blocks_unauthorized_user() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "owner@example.com").await;
    let outsider = register(&app, "outsider@example.com").await;
    promote_to_admin(&app, admin.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "private").await;
    // outsider is a non-admin without an explicit grant for this lib.

    let url = format!("/me/issues/{issue_id}/markers");
    let (status, _) = http(&app, Method::GET, &url, Some(&outsider), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn csrf_required_on_mutations() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "kara@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "k-lib").await;

    let req = Request::builder()
        .method(Method::POST)
        .uri("/me/markers")
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "issue_id": issue_id,
                "page_index": 0,
                "kind": "bookmark",
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_hydrates_series_and_issue_fields() {
    // The /bookmarks index renders thumbnails + "Jump to page" links,
    // so /me/markers needs to ship series + issue identity inline. The
    // per-issue endpoint and POST response keep the bare shape — those
    // callers already have the surrounding context.
    let app = TestApp::spawn().await;
    let auth = register(&app, "hydra@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "hydra-series").await;

    let (status, created) = http(
        &app,
        Method::POST,
        "/me/markers",
        Some(&auth),
        Some(serde_json::json!({
            "issue_id": issue_id,
            "page_index": 0,
            "kind": "bookmark",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    // POST response: hydrated fields omitted via skip_serializing_if.
    assert!(created.get("series_name").is_none());
    assert!(created.get("series_slug").is_none());

    let (status, list) = http(&app, Method::GET, "/me/markers", Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let m = &items[0];
    assert_eq!(m["series_name"], "hydra-series");
    assert_eq!(m["series_slug"], "hydra-series");
    assert_eq!(m["issue_slug"], "hydra-series-1");
    assert_eq!(m["issue_number"], "1");

    let per_issue_url = format!("/me/issues/{issue_id}/markers");
    let (status, per_issue) = http(&app, Method::GET, &per_issue_url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    // Per-issue endpoint: hydrated fields omitted.
    let it = &per_issue["items"][0];
    assert!(it.get("series_name").is_none());
    assert!(it.get("issue_slug").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn favorite_flag_round_trips_and_filters() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "fav@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "fav-lib").await;

    // One plain bookmark and one starred bookmark.
    let (_, plain) = http(
        &app,
        Method::POST,
        "/me/markers",
        Some(&auth),
        Some(serde_json::json!({
            "issue_id": issue_id, "page_index": 0, "kind": "bookmark",
        })),
    )
    .await;
    assert_eq!(plain["is_favorite"], false);

    let (_, starred) = http(
        &app,
        Method::POST,
        "/me/markers",
        Some(&auth),
        Some(serde_json::json!({
            "issue_id": issue_id, "page_index": 1, "kind": "bookmark",
            "is_favorite": true,
        })),
    )
    .await;
    let starred_id = starred["id"].as_str().unwrap().to_owned();
    assert_eq!(starred["is_favorite"], true);

    // Filter by is_favorite=true narrows to the starred row.
    let (_, list) = http(
        &app,
        Method::GET,
        "/me/markers?is_favorite=true",
        Some(&auth),
        None,
    )
    .await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], starred_id);

    // PATCH toggles the flag off; subsequent filter returns empty.
    let url = format!("/me/markers/{starred_id}");
    let (status, after) = http(
        &app,
        Method::PATCH,
        &url,
        Some(&auth),
        Some(serde_json::json!({ "is_favorite": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after["is_favorite"], false);

    let (_, list) = http(
        &app,
        Method::GET,
        "/me/markers?is_favorite=true",
        Some(&auth),
        None,
    )
    .await;
    assert!(list["items"].as_array().unwrap().is_empty());

    // The deprecated `kind='favorite'` value is rejected outright.
    let (status, _) = http(
        &app,
        Method::POST,
        "/me/markers",
        Some(&auth),
        Some(serde_json::json!({
            "issue_id": issue_id, "page_index": 2, "kind": "favorite",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tags_round_trip_and_filter_by_all_any() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "tags@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "tags-lib").await;

    // Three markers with overlapping tag sets:
    //   m1 = [funny, panel-art]
    //   m2 = [thread-idea, panel-art]
    //   m3 = [funny, thread-idea]
    for (page, tags) in [
        (0, vec!["Funny", "panel-art"]),
        (1, vec!["thread-idea", "Panel-Art"]),
        (2, vec!["funny", "thread-idea"]),
    ] {
        http(
            &app,
            Method::POST,
            "/me/markers",
            Some(&auth),
            Some(serde_json::json!({
                "issue_id": issue_id,
                "page_index": page,
                "kind": "bookmark",
                "tags": tags,
            })),
        )
        .await;
    }

    // Server normalizes (lowercase + dedupe), so the tag index sees
    // exactly three distinct tags across all three rows.
    let (status, tags) = http(&app, Method::GET, "/me/markers/tags", Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = tags["items"].as_array().unwrap();
    let names: Vec<&str> = items.iter().map(|t| t["tag"].as_str().unwrap()).collect();
    assert!(names.contains(&"funny"));
    assert!(names.contains(&"panel-art"));
    assert!(names.contains(&"thread-idea"));

    // AND: only m1 has BOTH funny and panel-art (default tag_match).
    let (_, list) = http(
        &app,
        Method::GET,
        "/me/markers?tags=funny,panel-art",
        Some(&auth),
        None,
    )
    .await;
    let pages: Vec<i64> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["page_index"].as_i64().unwrap())
        .collect();
    assert_eq!(pages, vec![0]);

    // ANY: union — all three should match because each has at least one.
    let (_, list) = http(
        &app,
        Method::GET,
        "/me/markers?tags=funny,panel-art&tag_match=any",
        Some(&auth),
        None,
    )
    .await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);

    // Bad tag_match → 400.
    let (status, _) = http(
        &app,
        Method::GET,
        "/me/markers?tags=funny&tag_match=somehow",
        Some(&auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn count_returns_per_user_total() {
    // The sidebar badge polls this every 60s, so it has to be both
    // scoped to the calling user and cheap (single COUNT(*)).
    let app = TestApp::spawn().await;
    let alice = register(&app, "count-alice@example.com").await;
    let bob = register(&app, "count-bob@example.com").await;
    promote_to_admin(&app, alice.user_id).await;
    promote_to_admin(&app, bob.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "count-lib").await;

    let (status, count) = http(&app, Method::GET, "/me/markers/count", Some(&alice), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(count["total"], 0);

    for p in 0..3 {
        http(
            &app,
            Method::POST,
            "/me/markers",
            Some(&alice),
            Some(serde_json::json!({
                "issue_id": issue_id,
                "page_index": p,
                "kind": "bookmark",
            })),
        )
        .await;
    }

    let (_, count) = http(&app, Method::GET, "/me/markers/count", Some(&alice), None).await;
    assert_eq!(count["total"], 3);

    // Bob's count is isolated from alice's.
    let (_, count) = http(&app, Method::GET, "/me/markers/count", Some(&bob), None).await;
    assert_eq!(count["total"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_removes_marker() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "leo@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib, _series, issue_id) = seed_issue(&app, "leo-lib").await;

    let (_, m) = http(
        &app,
        Method::POST,
        "/me/markers",
        Some(&auth),
        Some(serde_json::json!({
            "issue_id": issue_id,
            "page_index": 0,
            "kind": "bookmark",
        })),
    )
    .await;
    let id = m["id"].as_str().unwrap().to_owned();

    let url = format!("/me/markers/{id}");
    let (status, _) = http(&app, Method::DELETE, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = http(&app, Method::DELETE, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "second delete is 404");
}
