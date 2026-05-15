//! Navigation customization M1 — sidebar layout integration tests.
//!
//! Coverage:
//!   - GET returns the default layout (built-ins + libraries + saved
//!     views with `show_in_sidebar=true`) when the user has no
//!     overrides.
//!   - Library visibility is gated by `library_user_access` (admins see
//!     all, non-admins only their grants).
//!   - PATCH writes overrides and the next GET reflects them
//!     (visibility + position).
//!   - PATCH with an empty payload clears overrides → next GET returns
//!     defaults again.
//!   - PATCH rejects unknown `kind` and duplicate `(kind, ref_id)`
//!     pairs.
//!   - Default home pin order seeds Continue Reading → On Deck →
//!     Recently Added → Recently Updated.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{library, library_user_access, user::Entity as UserEntity};
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

async fn demote_to_user(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = UserEntity::find_by_id(user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::user::ActiveModel = user.into();
    am.role = Set("user".into());
    am.update(&db).await.unwrap();
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

async fn seed_library(app: &TestApp, name: &str) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(name.into()),
        root_path: Set(format!("/tmp/{name}-{lib_id}")),
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

async fn grant_library_access(app: &TestApp, user_id: Uuid, lib_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(user_id),
        library_id: Set(lib_id),
        role: Set("reader".into()),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_layout_contains_builtins_and_libraries() {
    let app = TestApp::spawn().await;
    // First registration ⇒ admin per project convention, so they see
    // every library without explicit access grants.
    let admin = register(&app, "admin@example.com").await;
    seed_library(&app, "Comics").await;
    seed_library(&app, "Manga").await;

    let (status, json) = http(&app, Method::GET, "/me/sidebar-layout", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    let entries = json["entries"].as_array().unwrap();

    let by_kind: Vec<(&str, &str)> = entries
        .iter()
        .map(|e| (e["kind"].as_str().unwrap(), e["ref_id"].as_str().unwrap()))
        .collect();

    // Built-ins in declared order.
    let builtins: Vec<&&str> = by_kind
        .iter()
        .filter_map(|(k, r)| if *k == "builtin" { Some(r) } else { None })
        .collect();
    assert_eq!(
        builtins,
        vec![&"home", &"bookmarks", &"collections", &"want_to_read"],
        "default builtin order"
    );

    // Libraries appear, alphabetical, with "All Libraries" synthetic
    // entry first so the client groups it in the Libraries section
    // rather than under Browse.
    let lib_labels: Vec<String> = entries
        .iter()
        .filter(|e| e["kind"] == "library")
        .map(|e| e["label"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(lib_labels, vec!["All Libraries", "Comics", "Manga"]);

    // All entries are visible by default.
    for e in entries {
        assert_eq!(e["visible"], true, "default visible: {e:#?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_admin_only_sees_granted_libraries() {
    let app = TestApp::spawn().await;
    // Burn one admin registration so the user we test gets the
    // restricted role.
    let _admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    demote_to_user(&app, user.user_id).await;
    let lib_a = seed_library(&app, "A-lib").await;
    let _lib_b = seed_library(&app, "B-lib").await;
    grant_library_access(&app, user.user_id, lib_a).await;

    let (status, json) = http(&app, Method::GET, "/me/sidebar-layout", Some(&user), None).await;
    assert_eq!(status, StatusCode::OK);
    let entries = json["entries"].as_array().unwrap();
    let lib_labels: Vec<String> = entries
        .iter()
        .filter(|e| e["kind"] == "library")
        .map(|e| e["label"].as_str().unwrap().to_owned())
        .collect();
    // "All Libraries" is always present (synthetic); only the granted
    // real library follows it.
    assert_eq!(
        lib_labels,
        vec!["All Libraries", "A-lib"],
        "only granted lib shows"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_overrides_visibility_and_position() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "alice@example.com").await;

    // Hide "bookmarks" and move "home" to position 99 so it lands last.
    let body = serde_json::json!({
        "entries": [
            { "kind": "builtin", "ref_id": "bookmarks", "visible": false, "position": 1 },
            { "kind": "builtin", "ref_id": "home", "visible": true, "position": 99 },
        ]
    });
    let (status, json) = http(
        &app,
        Method::PATCH,
        "/me/sidebar-layout",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "patch failed: {json:#?}");

    // Verify by reading back.
    let (status, json) = http(&app, Method::GET, "/me/sidebar-layout", Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let entries = json["entries"].as_array().unwrap();

    let bookmarks = entries
        .iter()
        .find(|e| e["kind"] == "builtin" && e["ref_id"] == "bookmarks")
        .expect("bookmarks present");
    assert_eq!(bookmarks["visible"], false, "bookmarks hidden after PATCH");

    // Home should now be the last entry by position.
    let home = entries
        .iter()
        .find(|e| e["kind"] == "builtin" && e["ref_id"] == "home")
        .expect("home present");
    let home_pos = home["position"].as_i64().unwrap();
    let max_pos = entries
        .iter()
        .map(|e| e["position"].as_i64().unwrap())
        .max()
        .unwrap();
    assert_eq!(
        home_pos, max_pos,
        "home should be at max position after override"
    );
    assert_eq!(home_pos, 99);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_empty_clears_overrides() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bob@example.com").await;

    // First override.
    let body = serde_json::json!({
        "entries": [
            { "kind": "builtin", "ref_id": "bookmarks", "visible": false, "position": 1 },
        ]
    });
    let (status, _) = http(
        &app,
        Method::PATCH,
        "/me/sidebar-layout",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Now clear with empty payload.
    let body = serde_json::json!({ "entries": [] });
    let (status, _) = http(
        &app,
        Method::PATCH,
        "/me/sidebar-layout",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // GET should reflect defaults (bookmarks visible again).
    let (_, json) = http(&app, Method::GET, "/me/sidebar-layout", Some(&auth), None).await;
    let bookmarks = json["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "builtin" && e["ref_id"] == "bookmarks")
        .expect("bookmarks present");
    assert_eq!(bookmarks["visible"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_rejects_invalid_kind() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "carol@example.com").await;

    let body = serde_json::json!({
        "entries": [
            { "kind": "made-up", "ref_id": "x", "visible": true, "position": 0 },
        ]
    });
    let (status, _) = http(
        &app,
        Method::PATCH,
        "/me/sidebar-layout",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_rejects_duplicate_ref() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "dan@example.com").await;

    let body = serde_json::json!({
        "entries": [
            { "kind": "builtin", "ref_id": "home", "visible": true, "position": 0 },
            { "kind": "builtin", "ref_id": "home", "visible": false, "position": 5 },
        ]
    });
    let (status, _) = http(
        &app,
        Method::PATCH,
        "/me/sidebar-layout",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_home_pin_order_is_curated() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "eve@example.com").await;

    // First touch seeds auto-pin system views into user_view_pins. The
    // top-down pin order is curated: Continue Reading → On Deck →
    // Recently Added → Recently Updated.
    let (status, json) = http(&app, Method::GET, "/me/saved-views", Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = json["items"].as_array().unwrap();
    let mut pinned: Vec<(i64, String)> = items
        .iter()
        .filter(|i| i["pinned"].as_bool() == Some(true))
        .map(|i| {
            (
                i["pinned_position"].as_i64().unwrap(),
                i["name"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    pinned.sort_by_key(|(p, _)| *p);
    let names: Vec<String> = pinned.into_iter().map(|(_, n)| n).collect();
    assert_eq!(
        names,
        vec![
            "Continue reading".to_owned(),
            "On deck".to_owned(),
            "Recently Added".to_owned(),
            "Recently Updated".to_owned(),
        ],
        "fresh user pin order should be curated top-down",
    );
}

// ───── multi-page rails M4 coverage ─────

async fn create_page(app: &TestApp, auth: &Authed, name: &str) -> String {
    let (status, body) = http(
        app,
        Method::POST,
        "/me/pages",
        Some(auth),
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create page {name}: {body:#?}");
    body["id"].as_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_pages_appear_after_libraries_by_default() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "pages-layout@example.com").await;
    let marvel = create_page(&app, &auth, "Marvel").await;
    let indie = create_page(&app, &auth, "Indie").await;
    seed_library(&app, "Comics").await;

    let (status, json) = http(&app, Method::GET, "/me/sidebar-layout", Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let entries = json["entries"].as_array().unwrap();
    let by_kind_ref: Vec<(String, String)> = entries
        .iter()
        .map(|e| {
            (
                e["kind"].as_str().unwrap().to_owned(),
                e["ref_id"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let pos = |kind: &str, refid: &str| -> usize {
        by_kind_ref
            .iter()
            .position(|(k, r)| k == kind && r == refid)
            .unwrap_or_else(|| panic!("missing {kind}:{refid} in {by_kind_ref:#?}"))
    };
    // Default order: Browse builtins → Libraries → Pages.
    let book = pos("builtin", "bookmarks");
    let libs_header = pos("header", "default:libraries");
    let pages_header = pos("header", "default:pages");
    let p_marvel = pos("page", &marvel);
    let p_indie = pos("page", &indie);
    assert!(book < libs_header, "Bookmarks comes before Libraries");
    assert!(libs_header < pages_header, "Libraries comes before Pages");
    assert!(pages_header < p_marvel && pages_header < p_indie);

    // Custom pages render with /pages/{slug} href and LayoutGrid icon.
    let marvel_entry = entries
        .iter()
        .find(|e| e["ref_id"].as_str() == Some(&marvel))
        .unwrap();
    assert_eq!(marvel_entry["href"], "/pages/marvel");
    assert_eq!(marvel_entry["icon"], "LayoutGrid");
    assert_eq!(marvel_entry["label"], "Marvel");
    assert_eq!(marvel_entry["visible"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn home_label_reflects_renamed_system_page() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "home-rename@example.com").await;

    let (_, pages) = http(&app, Method::GET, "/me/pages", Some(&auth), None).await;
    let home_id = pages
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["is_system"] == true)
        .map(|p| p["id"].as_str().unwrap().to_owned())
        .unwrap();
    let (status, _) = http(
        &app,
        Method::PATCH,
        &format!("/me/pages/{home_id}"),
        Some(&auth),
        Some(serde_json::json!({ "name": "Library" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, layout) = http(&app, Method::GET, "/me/sidebar-layout", Some(&auth), None).await;
    let entries = layout["entries"].as_array().unwrap();
    let home = entries
        .iter()
        .find(|e| e["kind"] == "builtin" && e["ref_id"] == "home")
        .unwrap();
    assert_eq!(home["label"], "Library");
    // href stays `/` — the route, not the label, is what's stable.
    assert_eq!(home["href"], "/");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_accepts_kind_page_override() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "page-override@example.com").await;
    let page_id = create_page(&app, &auth, "Custom").await;

    // Hide the custom page entry via an override row.
    let body = serde_json::json!({
        "entries": [
            { "kind": "page", "ref_id": page_id, "visible": false, "position": 5 },
        ]
    });
    let (status, _) = http(
        &app,
        Method::PATCH,
        "/me/sidebar-layout",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, json) = http(&app, Method::GET, "/me/sidebar-layout", Some(&auth), None).await;
    let entry = json["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "page" && e["ref_id"] == page_id)
        .unwrap()
        .clone();
    assert_eq!(entry["visible"], false);
    assert_eq!(entry["position"], 5);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleted_page_drops_from_layout() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "page-delete@example.com").await;
    let page_id = create_page(&app, &auth, "ToDelete").await;

    // Confirm the page is in the layout.
    let (_, before) = http(&app, Method::GET, "/me/sidebar-layout", Some(&auth), None).await;
    assert!(
        before["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "page" && e["ref_id"] == page_id)
    );

    // Delete the page; layout drops the row even if a stale override
    // would otherwise resurrect it.
    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/me/pages/{page_id}"),
        Some(&auth),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, after) = http(&app, Method::GET, "/me/sidebar-layout", Some(&auth), None).await;
    assert!(
        !after["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "page" && e["ref_id"] == page_id),
        "deleted page should not surface as a sidebar entry"
    );
}

// ───── header + spacer coverage ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_layout_includes_section_headers() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "headers-default@example.com").await;
    seed_library(&app, "Comics").await;

    let (_, layout) = http(&app, Method::GET, "/me/sidebar-layout", Some(&auth), None).await;
    let entries = layout["entries"].as_array().unwrap();
    let headers: Vec<(String, String)> = entries
        .iter()
        .filter(|e| e["kind"] == "header")
        .map(|e| {
            (
                e["ref_id"].as_str().unwrap().to_owned(),
                e["label"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    // No saved view in sidebar yet so the "Saved views" default header
    // doesn't appear; the other two do.
    assert!(
        headers
            .iter()
            .any(|(r, l)| r == "default:browse" && l == "Browse")
    );
    assert!(
        headers
            .iter()
            .any(|(r, l)| r == "default:libraries" && l == "Libraries")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_accepts_custom_header_and_spacer() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "headers-patch@example.com").await;

    let body = serde_json::json!({
        "entries": [
            {
                "kind": "header",
                "ref_id": "00000000-0000-0000-0000-000000000123",
                "label": "Reading list",
                "visible": true,
                "position": 0
            },
            {
                "kind": "spacer",
                "ref_id": "00000000-0000-0000-0000-000000000456",
                "visible": true,
                "position": 1
            },
        ]
    });
    let (status, _) = http(
        &app,
        Method::PATCH,
        "/me/sidebar-layout",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, layout) = http(&app, Method::GET, "/me/sidebar-layout", Some(&auth), None).await;
    let entries = layout["entries"].as_array().unwrap();
    let custom_header = entries
        .iter()
        .find(|e| e["kind"] == "header" && e["ref_id"] == "00000000-0000-0000-0000-000000000123")
        .unwrap();
    assert_eq!(custom_header["label"], "Reading list");
    let spacer = entries.iter().find(|e| e["kind"] == "spacer").unwrap();
    assert_eq!(spacer["ref_id"], "00000000-0000-0000-0000-000000000456");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_rejects_header_without_label() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "headers-empty@example.com").await;

    let body = serde_json::json!({
        "entries": [
            {
                "kind": "header",
                "ref_id": "00000000-0000-0000-0000-000000000789",
                "visible": true,
                "position": 0
            },
        ]
    });
    let (status, json) = http(
        &app,
        Method::PATCH,
        "/me/sidebar-layout",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "validation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn label_override_renames_default_entries() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "label-override@example.com").await;

    // Override the default "Libraries" header label.
    let body = serde_json::json!({
        "entries": [
            {
                "kind": "header",
                "ref_id": "default:libraries",
                "label": "My shelves",
                "visible": true,
                "position": 10
            },
        ]
    });
    let (status, _) = http(
        &app,
        Method::PATCH,
        "/me/sidebar-layout",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, layout) = http(&app, Method::GET, "/me/sidebar-layout", Some(&auth), None).await;
    let entry = layout["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "header" && e["ref_id"] == "default:libraries")
        .unwrap();
    assert_eq!(entry["label"], "My shelves");
}
