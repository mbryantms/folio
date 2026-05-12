//! Saved smart views — M3 integration coverage.
//!
//! Exercises the user-scoped CRUD path, pin lifecycle (cap enforcement,
//! reorder, idempotent unpin), system-view RBAC, lazy first-touch seed,
//! filter compile against junction-table data, and the admin endpoints
//! with audit-log assertions.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    issue_genre::ActiveModel as IssueGenreAM,
    library,
    series::{ActiveModel as SeriesAM, normalize_name},
    series_genre::ActiveModel as SeriesGenreAM,
    user::Entity as UserEntity,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, QueryOrder, Set};
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

/// Force a non-admin role on a user. Project default makes the first
/// registered user an admin (see CLAUDE.md). Tests that need a regular
/// user as the first registration use this to bring the role back down.
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

/// Insert one library + `count` series with the named `genre`. Each
/// series gets one issue too so the per-issue junction has a row to
/// rollup from. Returns (lib_id, vec<(series_id, name)>).
async fn seed_series_with_genre(
    app: &TestApp,
    lib_name: &str,
    genre: &str,
    series_names: &[&str],
) -> (Uuid, Vec<(Uuid, String)>) {
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

    let mut out = Vec::new();
    for name in series_names {
        let series_id = Uuid::now_v7();
        SeriesAM {
            id: Set(series_id),
            library_id: Set(lib_id),
            name: Set((*name).into()),
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
            slug: Set(format!("{lib_name}-{name}")),
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

        // One active issue so the series exists for filter queries.
        let issue_id = format!("{:0>62}{:02x}", series_id.simple(), 0u8);
        IssueAM {
            id: Set(issue_id.clone()),
            library_id: Set(lib_id),
            series_id: Set(series_id),
            slug: Set(format!("{name}-1")),
            file_path: Set(format!("/tmp/{lib_name}/{name}.cbz")),
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
            genre: Set(Some(genre.into())),
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

        // Junction tables: tests don't run the scanner, so populate
        // directly. Mirrors what `metadata_rollup::replace_issue_metadata`
        // would have written.
        IssueGenreAM {
            issue_id: Set(issue_id.clone()),
            genre: Set(genre.into()),
        }
        .insert(&db)
        .await
        .unwrap();
        SeriesGenreAM {
            series_id: Set(series_id),
            genre: Set(genre.into()),
        }
        .insert(&db)
        .await
        .unwrap();

        out.push((series_id, (*name).to_owned()));
    }
    (lib_id, out)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_seeds_system_views_pinned_on_first_touch() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "first@example.com").await;

    let (status, json) = http(&app, Method::GET, "/me/saved-views", Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = json["items"].as_array().unwrap();
    let names: Vec<String> = items
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    // M3 originals (auto-pinned) + M9 templates (available-to-pin) +
    // home-rails M1 system views (auto-pinned).
    assert!(names.iter().any(|n| n == "Recently Added"));
    assert!(names.iter().any(|n| n == "Recently Updated"));
    assert!(names.iter().any(|n| n == "Just Finished"));
    // M1 of markers + collections renamed the M9 "Want to Read"
    // filter template to "Unstarted" so the name is free for the
    // per-user manual collection landing in M3.
    assert!(names.iter().any(|n| n == "Unstarted"));
    assert!(names.iter().any(|n| n == "Stale"));
    assert!(names.iter().any(|n| n == "Continue reading"));
    assert!(names.iter().any(|n| n == "On deck"));
    // Markers + Collections M3: the per-user Want to Read collection
    // is auto-seeded on first GET /me/saved-views too (the sidebar
    // depends on this surface, not /me/collections).
    assert!(names.iter().any(|n| n == "Want to Read"));
    // M3 originals (auto_pin=true) + new home rails (auto_pin=true) auto-pin
    // for new users; the M9 templates land in the manager unpinned.
    let pinned_names: Vec<String> = items
        .iter()
        .filter(|i| i["pinned"].as_bool() == Some(true))
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    let mut sorted = pinned_names.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![
            "Continue reading".to_owned(),
            "On deck".to_owned(),
            "Recently Added".to_owned(),
            "Recently Updated".to_owned(),
        ],
        "exactly the M3 originals + new rails should auto-pin: {pinned_names:?}"
    );
    // Every system row (`user_id IS NULL`) must report is_system=true;
    // the per-user Want to Read collection is the only exception.
    for it in items {
        let is_system = it["is_system"].as_bool().unwrap_or(false);
        let is_wtr = it["system_key"].as_str() == Some("want_to_read");
        assert!(
            is_system || is_wtr,
            "row should be system or want_to_read: {it:#?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_filter_view_and_run_results() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "alice@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let (_lib, _series) =
        seed_series_with_genre(&app, "horror-lib", "Horror", &["Hellboy", "Locke and Key"]).await;
    let (_lib2, _series2) = seed_series_with_genre(&app, "scifi-lib", "Sci-Fi", &["Saga"]).await;

    let body = serde_json::json!({
        "kind": "filter_series",
        "name": "Horror Picks",
        "filter": {
            "match_mode": "all",
            "conditions": [
                { "group_id": 0, "field": "genres", "op": "includes_any", "value": ["Horror"] }
            ]
        },
        "sort_field": "name",
        "sort_order": "asc",
        "result_limit": 50,
    });
    let (status, view) = http(
        &app,
        Method::POST,
        "/me/saved-views",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "view: {view:#?}");
    let view_id = view["id"].as_str().unwrap();

    let url = format!("/me/saved-views/{view_id}/results");
    let (status, results) = http(&app, Method::GET, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::OK, "results: {results:#?}");
    let items = results["items"].as_array().unwrap();
    let names: Vec<String> = items
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(names.contains(&"Hellboy".to_owned()));
    assert!(names.contains(&"Locke and Key".to_owned()));
    assert!(!names.contains(&"Saga".to_owned()), "Saga is Sci-Fi");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_rejects_invalid_filter() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "bob@example.com").await;

    let body = serde_json::json!({
        "kind": "filter_series",
        "name": "Bad",
        "filter": {
            "match_mode": "all",
            "conditions": [
                { "group_id": 0, "field": "genres", "op": "gt", "value": 5 }
            ]
        },
        "sort_field": "created_at",
        "sort_order": "desc",
        "result_limit": 12,
    });
    let (status, body) = http(
        &app,
        Method::POST,
        "/me/saved-views",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body:#?}");
    assert_eq!(body["error"]["code"], "filter_invalid");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pin_cap_enforced() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "cap@example.com").await;
    // Trigger the lazy seed so the 4 auto-pinned system views land first.
    let _ = http(&app, Method::GET, "/me/saved-views", Some(&auth), None).await;
    // 12 - 4 (auto-pinned system rails) = 8 user pins reach the cap.
    for i in 0..8 {
        let body = serde_json::json!({
            "kind": "filter_series",
            "name": format!("View {i}"),
            "filter": { "match_mode": "all", "conditions": [] },
            "sort_field": "created_at",
            "sort_order": "desc",
            "result_limit": 12,
        });
        let (status, view) = http(
            &app,
            Method::POST,
            "/me/saved-views",
            Some(&auth),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "i={i} view={view:#?}");
        let view_id = view["id"].as_str().unwrap();
        let pin_url = format!("/me/saved-views/{view_id}/pin");
        let (status, _) = http(&app, Method::POST, &pin_url, Some(&auth), None).await;
        assert_eq!(status, StatusCode::OK, "pin {i}");
    }
    // The 13th pin should hit the cap.
    let body = serde_json::json!({
        "kind": "filter_series",
        "name": "Overflow",
        "filter": { "match_mode": "all", "conditions": [] },
        "sort_field": "created_at",
        "sort_order": "desc",
        "result_limit": 12,
    });
    let (_, overflow) = http(
        &app,
        Method::POST,
        "/me/saved-views",
        Some(&auth),
        Some(body),
    )
    .await;
    let view_id = overflow["id"].as_str().unwrap();
    let pin_url = format!("/me/saved-views/{view_id}/pin");
    let (status, body) = http(&app, Method::POST, &pin_url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body:#?}");
    assert_eq!(body["error"]["code"], "pin_cap_reached");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reorder_rewrites_positions() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "reorder@example.com").await;
    // Trigger the lazy seed.
    let _ = http(&app, Method::GET, "/me/saved-views", Some(&auth), None).await;

    // Create two more, pin them.
    let mut new_ids = Vec::new();
    for n in ["Alpha", "Beta"] {
        let body = serde_json::json!({
            "kind": "filter_series",
            "name": n,
            "filter": { "match_mode": "all", "conditions": [] },
            "sort_field": "created_at",
            "sort_order": "desc",
            "result_limit": 12,
        });
        let (_, v) = http(
            &app,
            Method::POST,
            "/me/saved-views",
            Some(&auth),
            Some(body),
        )
        .await;
        let id = v["id"].as_str().unwrap().to_owned();
        new_ids.push(id.clone());
        let pin_url = format!("/me/saved-views/{id}/pin");
        http(&app, Method::POST, &pin_url, Some(&auth), None).await;
    }

    // Read the current pin order.
    let (_, before) = http(
        &app,
        Method::GET,
        "/me/saved-views?pinned=true",
        Some(&auth),
        None,
    )
    .await;
    let before_ids: Vec<String> = before["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect();

    // Reverse the pin order.
    let reverse: Vec<String> = before_ids.iter().rev().cloned().collect();
    let body = serde_json::json!({ "view_ids": reverse });
    let (status, _) = http(
        &app,
        Method::POST,
        "/me/saved-views/reorder",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, after) = http(
        &app,
        Method::GET,
        "/me/saved-views?pinned=true",
        Some(&auth),
        None,
    )
    .await;
    let after_ids: Vec<String> = after["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect();
    let expected: Vec<String> = before_ids.iter().rev().cloned().collect();
    assert_eq!(after_ids, expected);

    // Positions are contiguous (no gaps).
    for (i, item) in after["items"].as_array().unwrap().iter().enumerate() {
        assert_eq!(item["pinned_position"].as_i64(), Some(i as i64));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_cannot_delete_system_view() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "user@example.com").await;
    let (_, list) = http(&app, Method::GET, "/me/saved-views", Some(&auth), None).await;
    let system_id = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["is_system"].as_bool() == Some(true))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let url = format!("/me/saved-views/{system_id}");
    let (status, _) = http(&app, Method::DELETE, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_can_create_system_view() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    promote_to_admin(&app, admin.user_id).await;

    let body = serde_json::json!({
        "kind": "filter_series",
        "name": "Just Finished",
        "filter": {
            "match_mode": "all",
            "conditions": [
                { "group_id": 0, "field": "read_progress", "op": "equals", "value": 100 }
            ]
        },
        "sort_field": "last_read",
        "sort_order": "desc",
        "result_limit": 12,
    });
    let (status, view) = http(
        &app,
        Method::POST,
        "/admin/saved-views",
        Some(&admin),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "view: {view:#?}");
    assert!(view["user_id"].is_null(), "system view => user_id null");
    assert_eq!(view["is_system"].as_bool(), Some(true));

    // Audit log captures it.
    let db = Database::connect(&app.db_url).await.unwrap();
    let entries = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.saved_view.create"))
        .order_by_desc(entity::audit_log::Column::CreatedAt)
        .all(&db)
        .await
        .unwrap();
    assert!(!entries.is_empty(), "audit row recorded");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn regular_user_cannot_admin_create() {
    let app = TestApp::spawn().await;
    // First-registered-user-is-admin default: register one we'll discard
    // so the second registration is a true regular user.
    let _first = register(&app, "first@example.com").await;
    let auth = register(&app, "regular@example.com").await;
    demote_to_user(&app, auth.user_id).await;
    let body = serde_json::json!({
        "kind": "filter_series",
        "name": "Sneaky",
        "filter": { "match_mode": "all", "conditions": [] },
        "sort_field": "created_at",
        "sort_order": "desc",
        "result_limit": 12,
    });
    let (status, _) = http(
        &app,
        Method::POST,
        "/admin/saved-views",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_runs_dsl_without_persisting() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "preview@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let _ = seed_series_with_genre(&app, "preview-lib", "Mystery", &["Sherlock"]).await;

    let body = serde_json::json!({
        "filter": {
            "match_mode": "all",
            "conditions": [
                { "group_id": 0, "field": "genres", "op": "includes_any", "value": ["Mystery"] }
            ]
        },
        "sort_field": "name",
        "sort_order": "asc",
        "result_limit": 50,
    });
    let (status, results) = http(
        &app,
        Method::POST,
        "/me/saved-views/preview",
        Some(&auth),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<String> = results["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(names.contains(&"Sherlock".to_owned()));

    // Preview must not have persisted a saved_view row.
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_owned = entity::saved_view::Entity::find()
        .filter(entity::saved_view::Column::UserId.eq(auth.user_id))
        .all(&db)
        .await
        .unwrap();
    assert!(user_owned.is_empty(), "preview must not write a row");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sidebar_toggle_round_trips_independently_of_pin() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "sidebar@example.com").await;
    // Trigger seed; pin the Recently Added system view comes auto-pinned.
    let (_, list) = http(&app, Method::GET, "/me/saved-views", Some(&auth), None).await;
    let recently_added = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "Recently Added")
        .unwrap();
    assert_eq!(recently_added["pinned"].as_bool(), Some(true));
    assert_eq!(recently_added["show_in_sidebar"].as_bool(), Some(false));
    let view_id = recently_added["id"].as_str().unwrap().to_owned();

    // Add to sidebar without changing pin state.
    let url = format!("/me/saved-views/{view_id}/sidebar");
    let (status, _) = http(&app, Method::POST, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Read back: pinned should still be true, show_in_sidebar now true.
    let (_, after) = http(
        &app,
        Method::GET,
        "/me/saved-views?show_in_sidebar=true",
        Some(&auth),
        None,
    )
    .await;
    let items = after["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"].as_str().unwrap(), view_id);
    assert_eq!(items[0]["pinned"].as_bool(), Some(true));
    assert_eq!(items[0]["show_in_sidebar"].as_bool(), Some(true));

    // Removing from sidebar (?show=false) flips just that flag.
    let off_url = format!("/me/saved-views/{view_id}/sidebar?show=false");
    let (status, _) = http(&app, Method::POST, &off_url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, all) = http(&app, Method::GET, "/me/saved-views", Some(&auth), None).await;
    let after = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"].as_str() == Some(&view_id))
        .unwrap();
    assert_eq!(after["pinned"].as_bool(), Some(true));
    assert_eq!(after["show_in_sidebar"].as_bool(), Some(false));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sidebar_only_view_doesnt_consume_pin_cap() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "side-only@example.com").await;
    // Trigger seed.
    let _ = http(&app, Method::GET, "/me/saved-views", Some(&auth), None).await;

    // Create a user view, add to sidebar without pinning, verify it
    // doesn't show under pinned filter and doesn't bump the cap counter.
    let body = serde_json::json!({
        "kind": "filter_series",
        "name": "Sidebar only",
        "filter": { "match_mode": "all", "conditions": [] },
        "sort_field": "created_at",
        "sort_order": "desc",
        "result_limit": 12,
    });
    let (_, v) = http(
        &app,
        Method::POST,
        "/me/saved-views",
        Some(&auth),
        Some(body),
    )
    .await;
    let id = v["id"].as_str().unwrap().to_owned();

    // Sidebar without pin.
    let url = format!("/me/saved-views/{id}/sidebar");
    let (status, _) = http(&app, Method::POST, &url, Some(&auth), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Pinned filter must not include this row.
    let (_, pinned) = http(
        &app,
        Method::GET,
        "/me/saved-views?pinned=true",
        Some(&auth),
        None,
    )
    .await;
    assert!(
        !pinned["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"].as_str() == Some(&id)),
        "sidebar-only view should not appear under ?pinned=true",
    );

    // Sidebar filter must include it.
    let (_, in_sidebar) = http(
        &app,
        Method::GET,
        "/me/saved-views?show_in_sidebar=true",
        Some(&auth),
        None,
    )
    .await;
    assert!(
        in_sidebar["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"].as_str() == Some(&id)),
    );
}
