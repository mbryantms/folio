//! Multi-page rails M2 — page CRUD integration coverage.
//!
//! Exercises `GET/POST/PATCH/DELETE /me/pages` and `POST /me/pages/reorder`:
//! system-page seed, slug allocation + disambiguation, system-page
//! protection, 20-page cap, FK-cascade on delete, cross-user isolation.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    saved_view::ActiveModel as SavedViewAM, user_page, user_view_pin::ActiveModel as UserViewPinAM,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, Set};
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

fn id(v: &serde_json::Value) -> Uuid {
    Uuid::parse_str(v["id"].as_str().unwrap()).unwrap()
}

// ───── tests ─────

#[tokio::test]
async fn list_includes_system_home_for_fresh_user() {
    let app = TestApp::spawn().await;
    let user = register(&app, "list-home@example.com").await;

    let (status, body) = http(&app, Method::GET, "/me/pages", Some(&user), None).await;
    assert_eq!(status, StatusCode::OK);
    let arr = body.as_array().unwrap();
    assert_eq!(
        arr.len(),
        1,
        "fresh user should have exactly one page (Home)"
    );
    assert_eq!(arr[0]["is_system"], true);
    assert_eq!(arr[0]["slug"], "home");
    assert_eq!(arr[0]["name"], "Home");
    assert_eq!(arr[0]["pin_count"], 0);
}

#[tokio::test]
async fn create_then_list_returns_both() {
    let app = TestApp::spawn().await;
    let user = register(&app, "create-list@example.com").await;

    let (status, body) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "Marvel" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["slug"], "marvel");
    assert_eq!(body["is_system"], false);
    assert_eq!(body["pin_count"], 0);

    let (status, body) = http(&app, Method::GET, "/me/pages", Some(&user), None).await;
    assert_eq!(status, StatusCode::OK);
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    // Home first (position 0), Marvel second (position 1).
    assert_eq!(arr[0]["is_system"], true);
    assert_eq!(arr[1]["name"], "Marvel");
    assert!(arr[1]["position"].as_i64().unwrap() > arr[0]["position"].as_i64().unwrap());
}

#[tokio::test]
async fn create_rejects_empty_and_long_names() {
    let app = TestApp::spawn().await;
    let user = register(&app, "validation@example.com").await;

    let (status, body) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation");

    let too_long: String = "a".repeat(81);
    let (status, _) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": too_long })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_disambiguates_collision_slug() {
    let app = TestApp::spawn().await;
    let user = register(&app, "slug@example.com").await;

    let (_, a) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "Marvel" })),
    )
    .await;
    assert_eq!(a["slug"], "marvel");

    let (_, b) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "Marvel" })),
    )
    .await;
    assert_eq!(b["slug"], "marvel-2");

    let (_, c) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "Marvel" })),
    )
    .await;
    assert_eq!(c["slug"], "marvel-3");
}

#[tokio::test]
async fn create_enforces_twenty_custom_page_cap() {
    let app = TestApp::spawn().await;
    let user = register(&app, "cap@example.com").await;

    for i in 0..20 {
        let (status, _) = http(
            &app,
            Method::POST,
            "/me/pages",
            Some(&user),
            Some(serde_json::json!({ "name": format!("Page {i}") })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "page {i} should succeed");
    }
    let (status, body) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "Overflow" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "page_cap_reached");
}

#[tokio::test]
async fn rename_regenerates_slug() {
    let app = TestApp::spawn().await;
    let user = register(&app, "rename@example.com").await;

    let (_, created) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "Marvel" })),
    )
    .await;
    let page_id = id(&created);

    let (status, body) = http(
        &app,
        Method::PATCH,
        &format!("/me/pages/{page_id}"),
        Some(&user),
        Some(serde_json::json!({ "name": "Indie Comics" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "Indie Comics");
    assert_eq!(body["slug"], "indie-comics");
}

#[tokio::test]
async fn rename_system_page_keeps_home_slug() {
    let app = TestApp::spawn().await;
    let user = register(&app, "system-rename@example.com").await;

    let (_, pages) = http(&app, Method::GET, "/me/pages", Some(&user), None).await;
    let home = pages.as_array().unwrap()[0].clone();
    assert_eq!(home["is_system"], true);
    let home_id = id(&home);

    let (status, body) = http(
        &app,
        Method::PATCH,
        &format!("/me/pages/{home_id}"),
        Some(&user),
        Some(serde_json::json!({ "name": "Library" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "Library");
    assert_eq!(body["slug"], "home", "system page slug must stay 'home'");
    assert_eq!(body["is_system"], true);
}

#[tokio::test]
async fn delete_system_page_returns_409() {
    let app = TestApp::spawn().await;
    let user = register(&app, "system-delete@example.com").await;

    let (_, pages) = http(&app, Method::GET, "/me/pages", Some(&user), None).await;
    let home_id = id(&pages.as_array().unwrap()[0]);

    let (status, body) = http(
        &app,
        Method::DELETE,
        &format!("/me/pages/{home_id}"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "system_page");

    // Still there.
    let (_, after) = http(&app, Method::GET, "/me/pages", Some(&user), None).await;
    assert_eq!(after.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn delete_custom_page_cascades_pin_rows() {
    let app = TestApp::spawn().await;
    let user = register(&app, "cascade@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let (_, created) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "Marvel" })),
    )
    .await;
    let page_id = id(&created);

    // Seed a saved view + a pin row on this page so we can prove the
    // cascade removes pin rows when the page goes away.
    let view_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    SavedViewAM {
        id: Set(view_id),
        user_id: Set(Some(user.user_id)),
        kind: Set("filter_series".into()),
        system_key: Set(None),
        name: Set("Cascade Test".into()),
        description: Set(None),
        custom_year_start: Set(None),
        custom_year_end: Set(None),
        custom_tags: Set(Vec::new()),
        match_mode: Set(Some("all".into())),
        conditions: Set(Some(serde_json::json!([]))),
        sort_field: Set(Some("created_at".into())),
        sort_order: Set(Some("desc".into())),
        result_limit: Set(Some(12)),
        cbl_list_id: Set(None),
        auto_pin: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    UserViewPinAM {
        user_id: Set(user.user_id),
        page_id: Set(page_id),
        view_id: Set(view_id),
        position: Set(0),
        pinned: Set(true),
        show_in_sidebar: Set(false),
        icon: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    let pin_count_before = entity::user_view_pin::Entity::find()
        .filter(entity::user_view_pin::Column::PageId.eq(page_id))
        .all(&db)
        .await
        .unwrap()
        .len();
    assert_eq!(pin_count_before, 1);

    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/me/pages/{page_id}"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let pin_count_after = entity::user_view_pin::Entity::find()
        .filter(entity::user_view_pin::Column::PageId.eq(page_id))
        .all(&db)
        .await
        .unwrap()
        .len();
    assert_eq!(pin_count_after, 0, "pin rows should cascade");
    let pages_after = user_page::Entity::find()
        .filter(user_page::Column::UserId.eq(user.user_id))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(pages_after.len(), 1, "system page survives");
}

#[tokio::test]
async fn reorder_rewrites_positions() {
    let app = TestApp::spawn().await;
    let user = register(&app, "reorder@example.com").await;

    let (_, a) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "Alpha" })),
    )
    .await;
    let (_, b) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "Bravo" })),
    )
    .await;
    let (_, c) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "Charlie" })),
    )
    .await;
    let a_id = id(&a);
    let b_id = id(&b);
    let c_id = id(&c);

    // Fetch the system page id too — reorder must list every owned page.
    let (_, pages) = http(&app, Method::GET, "/me/pages", Some(&user), None).await;
    let home_id = id(pages
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["is_system"] == true)
        .unwrap());

    // Reverse the custom-page order; Home stays first.
    let (status, _) = http(
        &app,
        Method::POST,
        "/me/pages/reorder",
        Some(&user),
        Some(serde_json::json!({
            "page_ids": [home_id, c_id, b_id, a_id],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, after) = http(&app, Method::GET, "/me/pages", Some(&user), None).await;
    let ordered_ids: Vec<String> = after
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        ordered_ids,
        vec![
            home_id.to_string(),
            c_id.to_string(),
            b_id.to_string(),
            a_id.to_string(),
        ]
    );
}

#[tokio::test]
async fn reorder_rejects_partial_or_duplicate_set() {
    let app = TestApp::spawn().await;
    let user = register(&app, "reorder-validation@example.com").await;
    let (_, a) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "A" })),
    )
    .await;
    let a_id = id(&a);

    // Missing the system page.
    let (status, body) = http(
        &app,
        Method::POST,
        "/me/pages/reorder",
        Some(&user),
        Some(serde_json::json!({ "page_ids": [a_id] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation");

    // Duplicate.
    let (status, body) = http(
        &app,
        Method::POST,
        "/me/pages/reorder",
        Some(&user),
        Some(serde_json::json!({ "page_ids": [a_id, a_id] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation");
}

#[tokio::test]
async fn cross_user_isolation() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice@example.com").await;
    let bob = register(&app, "bob@example.com").await;

    let (_, page) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&alice),
        Some(serde_json::json!({ "name": "Alice page" })),
    )
    .await;
    let alice_page_id = id(&page);

    // Bob cannot see Alice's page in his list.
    let (_, bob_pages) = http(&app, Method::GET, "/me/pages", Some(&bob), None).await;
    let bob_arr = bob_pages.as_array().unwrap();
    assert_eq!(bob_arr.len(), 1, "Bob only sees his own Home page");
    assert!(bob_arr.iter().all(|p| p["id"] != alice_page_id.to_string()));

    // Bob cannot rename it.
    let (status, _) = http(
        &app,
        Method::PATCH,
        &format!("/me/pages/{alice_page_id}"),
        Some(&bob),
        Some(serde_json::json!({ "name": "Hijacked" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Bob cannot delete it.
    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/me/pages/{alice_page_id}"),
        Some(&bob),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rename_to_same_name_is_noop() {
    // Idempotency check — keeps slug intact when name doesn't actually
    // change (so a save-on-blur UI doesn't churn slugs each time).
    let app = TestApp::spawn().await;
    let user = register(&app, "noop@example.com").await;
    let (_, created) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "Stable" })),
    )
    .await;
    let page_id = id(&created);
    let original_slug = created["slug"].as_str().unwrap().to_owned();

    let (status, body) = http(
        &app,
        Method::PATCH,
        &format!("/me/pages/{page_id}"),
        Some(&user),
        Some(serde_json::json!({ "name": "Stable" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["slug"], original_slug);
}

#[tokio::test]
async fn description_round_trips_via_patch() {
    let app = TestApp::spawn().await;
    let user = register(&app, "description@example.com").await;
    let (_, created) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "Marvel" })),
    )
    .await;
    let page_id = id(&created);
    // Fresh page has no description.
    assert!(created["description"].is_null());

    // Set it.
    let (status, body) = http(
        &app,
        Method::PATCH,
        &format!("/me/pages/{page_id}"),
        Some(&user),
        Some(serde_json::json!({ "description": "Hickman + Hickman + Hickman" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["description"], "Hickman + Hickman + Hickman");

    // Trim whitespace + empty → null.
    let (_, body) = http(
        &app,
        Method::PATCH,
        &format!("/me/pages/{page_id}"),
        Some(&user),
        Some(serde_json::json!({ "description": "   " })),
    )
    .await;
    assert!(body["description"].is_null());

    // Explicit empty-string clears (the documented convention; serde
    // can't reliably distinguish a missing field from a literal null
    // without a custom deserializer, so empty-string is the signal).
    http(
        &app,
        Method::PATCH,
        &format!("/me/pages/{page_id}"),
        Some(&user),
        Some(serde_json::json!({ "description": "Back again" })),
    )
    .await;
    let (_, body) = http(
        &app,
        Method::PATCH,
        &format!("/me/pages/{page_id}"),
        Some(&user),
        Some(serde_json::json!({ "description": "" })),
    )
    .await;
    assert!(body["description"].is_null());

    // null is treated as "unchanged" — survives.
    http(
        &app,
        Method::PATCH,
        &format!("/me/pages/{page_id}"),
        Some(&user),
        Some(serde_json::json!({ "description": "Persistent" })),
    )
    .await;
    let (_, body) = http(
        &app,
        Method::PATCH,
        &format!("/me/pages/{page_id}"),
        Some(&user),
        Some(serde_json::json!({ "description": null })),
    )
    .await;
    assert_eq!(body["description"], "Persistent");

    // Long descriptions are accepted — no length cap.
    let long: String = "x".repeat(2000);
    let (status, body) = http(
        &app,
        Method::PATCH,
        &format!("/me/pages/{page_id}"),
        Some(&user),
        Some(serde_json::json!({ "description": long })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["description"].as_str().unwrap().len(), 2000);
}

#[tokio::test]
async fn sidebar_toggle_hides_and_shows_page() {
    let app = TestApp::spawn().await;
    let user = register(&app, "sidebar-toggle@example.com").await;
    let (_, created) = http(
        &app,
        Method::POST,
        "/me/pages",
        Some(&user),
        Some(serde_json::json!({ "name": "Marvel" })),
    )
    .await;
    let page_id = id(&created);
    // Visible by default.
    assert_eq!(created["show_in_sidebar"], true);

    // Hide.
    let (status, _) = http(
        &app,
        Method::POST,
        &format!("/me/pages/{page_id}/sidebar?show=false"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, pages) = http(&app, Method::GET, "/me/pages", Some(&user), None).await;
    let row = pages
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == page_id.to_string())
        .unwrap();
    assert_eq!(row["show_in_sidebar"], false);

    // Sidebar layout drops the page entry.
    let (_, layout) = http(&app, Method::GET, "/me/sidebar-layout", Some(&user), None).await;
    let entry = layout["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "page" && e["ref_id"] == page_id.to_string())
        .cloned();
    // Entry still surfaces but with visible=false.
    assert_eq!(entry.unwrap()["visible"], false);

    // Show again.
    let (status, _) = http(
        &app,
        Method::POST,
        &format!("/me/pages/{page_id}/sidebar?show=true"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, pages) = http(&app, Method::GET, "/me/pages", Some(&user), None).await;
    let row = pages
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == page_id.to_string())
        .unwrap();
    assert_eq!(row["show_in_sidebar"], true);
}

#[tokio::test]
async fn sidebar_toggle_on_system_page_returns_409() {
    let app = TestApp::spawn().await;
    let user = register(&app, "sys-toggle@example.com").await;
    let (_, pages) = http(&app, Method::GET, "/me/pages", Some(&user), None).await;
    let home_id = id(pages
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["is_system"] == true)
        .unwrap());
    let (status, body) = http(
        &app,
        Method::POST,
        &format!("/me/pages/{home_id}/sidebar?show=false"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "system_page");
}
