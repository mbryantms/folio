//! Integration tests for `GET /me/export` (roadmap WP-2.1).
//!
//! Seeds two users, gives user A one row of every exportable kind, and
//! asserts A's document carries each section with the right counts and
//! identity keys — and that B's document is empty for every section
//! (ownership is scoped to the caller, never the library).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{
    seed_collection, seed_collection_entry_issue, seed_issue, seed_library, seed_progress,
    seed_series,
};
use entity::{
    marker, rail_dismissal, reading_session, saved_view, user, user_page, user_rating,
    user_sidebar_entry, user_view_pin,
};
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use serde_json::{Value, json};
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

async fn body_json(b: Body) -> Value {
    serde_json::from_slice(&to_bytes(b, usize::MAX).await.unwrap()).unwrap()
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
    let json = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session,
        csrf,
        user_id,
    }
}

async fn get_export(app: &TestApp, auth: &Authed) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/me/export")
                .header(header::COOKIE, auth.cookies())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Everything user A owns after seeding, so assertions can compare ids.
struct Seeded {
    lib_id: Uuid,
    series_id: Uuid,
    issue_id: String,
    content_hash: String,
    collection_id: Uuid,
    view_id: Uuid,
    page_id: Uuid,
    marker_id: Uuid,
    session_id: Uuid,
}

async fn seed_user_a(db: &DatabaseConnection, tmp: &std::path::Path, user_id: Uuid) -> Seeded {
    let now = Utc::now().fixed_offset();
    let lib_id = seed_library(db, tmp).await;
    let series_id = seed_series(db, lib_id, "Exportable").await;
    let issue_id = seed_issue(db, lib_id, series_id, &tmp.join("a.cbz"), b"export-a", 1.0).await;
    let content_hash = entity::issue::Entity::find_by_id(issue_id.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .content_hash;

    // progress
    seed_progress(db, user_id, &issue_id, 7, 0.35, false).await;

    // marker (a note with region + tags)
    let marker_id = Uuid::now_v7();
    marker::ActiveModel {
        id: Set(marker_id),
        user_id: Set(user_id),
        series_id: Set(series_id),
        issue_id: Set(issue_id.clone()),
        page_index: Set(3),
        kind: Set("note".into()),
        is_favorite: Set(true),
        tags: Set(vec!["quote".into(), "arc".into()]),
        region: Set(Some(
            json!({"x": 1.0, "y": 2.0, "w": 10.0, "h": 5.0, "shape": "rect"}),
        )),
        selection: Set(Some(json!({"text": "captured"}))),
        body: Set(Some("a note body".into())),
        color: Set(Some("yellow".into())),
        created_at: Set(now),
        updated_at: Set(now),
        hidden_from_log: Set(false),
    }
    .insert(db)
    .await
    .unwrap();

    // collection + entry, plus Want to Read
    let collection_id = seed_collection(db, user_id, "Favourites").await;
    seed_collection_entry_issue(db, collection_id, 0, &issue_id).await;
    let wtr_id = Uuid::now_v7();
    saved_view::ActiveModel {
        id: Set(wtr_id),
        user_id: Set(Some(user_id)),
        kind: Set("collection".into()),
        system_key: Set(Some("want_to_read".into())),
        name: Set("Want to Read".into()),
        description: Set(None),
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
        preserve_canonical_order: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    seed_collection_entry_issue(db, wtr_id, 0, &issue_id).await;

    // filter saved view
    let view_id = Uuid::now_v7();
    saved_view::ActiveModel {
        id: Set(view_id),
        user_id: Set(Some(user_id)),
        kind: Set("filter_series".into()),
        system_key: Set(None),
        name: Set("Unread Marvel".into()),
        description: Set(None),
        custom_year_start: Set(None),
        custom_year_end: Set(None),
        custom_tags: Set(vec!["marvel".into()]),
        match_mode: Set(Some("all".into())),
        conditions: Set(Some(json!([
            {"group_id": "g1", "field": "publisher", "op": "eq", "value": "Marvel"}
        ]))),
        sort_field: Set(Some("name".into())),
        sort_order: Set(Some("asc".into())),
        result_limit: Set(Some(24)),
        cbl_list_id: Set(None),
        auto_pin: Set(false),
        preserve_canonical_order: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();

    // ratings: one issue, one series
    for (target_type, target_id, rating) in [
        ("issue", issue_id.clone(), 4.5),
        ("series", series_id.to_string(), 3.0),
    ] {
        user_rating::ActiveModel {
            user_id: Set(user_id),
            target_type: Set(target_type.into()),
            target_id: Set(target_id),
            rating: Set(rating),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();
    }

    // custom page + a pin of the filter view on it
    let page_id = Uuid::now_v7();
    user_page::ActiveModel {
        id: Set(page_id),
        user_id: Set(user_id),
        name: Set("Weekend".into()),
        slug: Set("weekend".into()),
        is_system: Set(false),
        position: Set(1),
        description: Set(Some("Saturday reading".into())),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    user_view_pin::ActiveModel {
        user_id: Set(user_id),
        page_id: Set(page_id),
        view_id: Set(view_id),
        position: Set(0),
        pinned: Set(true),
        show_in_sidebar: Set(false),
        icon: Set(Some("sparkles".into())),
    }
    .insert(db)
    .await
    .unwrap();

    // sidebar override
    user_sidebar_entry::ActiveModel {
        user_id: Set(user_id),
        kind: Set("builtin".into()),
        ref_id: Set("bookmarks".into()),
        visible: Set(true),
        position: Set(2),
        label: Set(Some("Pins".into())),
    }
    .insert(db)
    .await
    .unwrap();

    // rail dismissal
    rail_dismissal::ActiveModel {
        user_id: Set(user_id),
        target_kind: Set("issue".into()),
        target_id: Set(issue_id.clone()),
        dismissed_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();

    // reading session
    let session_id = Uuid::now_v7();
    reading_session::ActiveModel {
        id: Set(session_id),
        user_id: Set(user_id),
        issue_id: Set(issue_id.clone()),
        series_id: Set(series_id),
        client_session_id: Set("client-1".into()),
        started_at: Set(now),
        ended_at: Set(Some(now)),
        last_heartbeat_at: Set(now),
        active_ms: Set(120_000),
        distinct_pages_read: Set(8),
        page_turns: Set(9),
        start_page: Set(0),
        end_page: Set(7),
        furthest_page: Set(7),
        device: Set(Some("desktop".into())),
        view_mode: Set(Some("single".into())),
        client_meta: Set(json!({})),
        hidden_from_log: Set(false),
    }
    .insert(db)
    .await
    .unwrap();

    // keybinds on the users row
    let mut am: user::ActiveModel = user::Entity::find_by_id(user_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.keybinds = Set(json!({"next_page": "l"}));
    am.theme = Set(Some("amber".into()));
    am.update(db).await.unwrap();

    Seeded {
        lib_id,
        series_id,
        issue_id,
        content_hash,
        collection_id,
        view_id,
        page_id,
        marker_id,
        session_id,
    }
}

fn assert_issue_ref(r: &Value, s: &Seeded) {
    assert_eq!(r["issue_id"], s.issue_id, "issue_id: {r}");
    assert_eq!(r["content_hash"], s.content_hash, "content_hash: {r}");
    assert_eq!(r["series_id"], s.series_id.to_string(), "series_id: {r}");
    assert_eq!(r["series_name"], "Exportable", "series_name: {r}");
    assert!(r["issue_number"].is_string(), "issue_number: {r}");
    assert!(
        r.get("series_year").is_some(),
        "series_year key present: {r}"
    );
    assert_eq!(r["library_slug"], s.lib_id.to_string(), "library_slug: {r}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_contains_every_section_the_user_owns() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::connect(&app.db_url).await.unwrap();

    let a = register(&app, "a@example.com").await;
    let b = register(&app, "b@example.com").await;
    let seeded = seed_user_a(&db, tmp.path(), a.user_id).await;
    // Give B some unrelated data so "empty" isn't trivially "no rows in DB".
    seed_progress(&db, b.user_id, &seeded.issue_id, 1, 0.05, false).await;

    let resp = get_export(&app, &a).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let disposition = resp
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        disposition.starts_with("attachment; filename=\"folio-export-")
            && disposition.ends_with(".json\""),
        "unexpected Content-Disposition: {disposition}"
    );
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(ct.starts_with("application/json"), "content-type: {ct}");

    let doc = body_json(resp.into_body()).await;

    // ── envelope ──
    assert_eq!(doc["format"], "folio-user-export");
    assert_eq!(doc["version"], 1);
    assert!(doc["exported_at"].is_string());
    assert_eq!(doc["user"]["id"], a.user_id.to_string());
    assert_eq!(doc["user"]["email"], "a@example.com");

    let s = &doc["sections"];

    // ── progress ──
    let progress = s["progress"].as_array().unwrap();
    assert_eq!(progress.len(), 1);
    assert_issue_ref(&progress[0]["issue"], &seeded);
    assert_eq!(progress[0]["last_page"], 7);
    assert_eq!(progress[0]["finished"], false);
    assert_eq!(progress[0]["is_backfill"], false);
    assert!(progress[0].get("finished_at").is_some());
    assert!(progress[0].get("device").is_some());
    assert!(progress[0]["updated_at"].is_string());

    // ── markers ──
    let markers = s["markers"].as_array().unwrap();
    assert_eq!(markers.len(), 1);
    let m = &markers[0];
    assert_eq!(m["id"], seeded.marker_id.to_string());
    assert_issue_ref(&m["issue"], &seeded);
    assert_eq!(m["kind"], "note");
    assert_eq!(m["page_index"], 3);
    assert_eq!(m["is_favorite"], true);
    assert_eq!(m["tags"], json!(["quote", "arc"]));
    assert_eq!(m["region"]["shape"], "rect");
    assert_eq!(m["selection"]["text"], "captured");
    assert_eq!(m["body"], "a note body");
    assert_eq!(m["color"], "yellow");

    // ── collections + want_to_read ──
    let collections = s["collections"].as_array().unwrap();
    assert_eq!(
        collections.len(),
        1,
        "want_to_read must not be in collections"
    );
    assert_eq!(collections[0]["id"], seeded.collection_id.to_string());
    assert_eq!(collections[0]["name"], "Favourites");
    assert!(collections[0]["system_key"].is_null());
    let entries = collections[0]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["entry_kind"], "issue");
    assert_issue_ref(&entries[0]["issue"], &seeded);
    assert!(entries[0]["series"].is_null());

    let wtr = &s["want_to_read"];
    assert_eq!(wtr["system_key"], "want_to_read");
    assert_eq!(wtr["entries"].as_array().unwrap().len(), 1);
    assert_issue_ref(&wtr["entries"][0]["issue"], &seeded);

    // ── saved views ──
    let views = s["saved_views"].as_array().unwrap();
    assert_eq!(views.len(), 1, "collections must not leak into saved_views");
    assert_eq!(views[0]["id"], seeded.view_id.to_string());
    assert_eq!(views[0]["kind"], "filter_series");
    assert_eq!(views[0]["conditions"][0]["field"], "publisher");
    assert_eq!(views[0]["match_mode"], "all");
    assert_eq!(views[0]["result_limit"], 24);

    // ── ratings ──
    let ratings = s["ratings"].as_array().unwrap();
    assert_eq!(ratings.len(), 2);
    let issue_rating = ratings
        .iter()
        .find(|r| r["target_type"] == "issue")
        .expect("issue rating");
    assert_issue_ref(&issue_rating["issue"], &seeded);
    assert_eq!(issue_rating["rating"], 4.5);
    let series_rating = ratings
        .iter()
        .find(|r| r["target_type"] == "series")
        .expect("series rating");
    assert_eq!(
        series_rating["series"]["series_id"],
        seeded.series_id.to_string()
    );
    assert_eq!(series_rating["series"]["series_name"], "Exportable");
    assert_eq!(
        series_rating["series"]["library_slug"],
        seeded.lib_id.to_string()
    );
    assert!(series_rating["issue"].is_null());

    // ── custom pages + pins ──
    let pages = s["custom_pages"].as_array().unwrap();
    let weekend = pages
        .iter()
        .find(|p| p["id"] == seeded.page_id.to_string())
        .expect("custom page exported");
    assert_eq!(weekend["slug"], "weekend");
    assert_eq!(weekend["is_system"], false);
    let pins = weekend["pins"].as_array().unwrap();
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0]["view_id"], seeded.view_id.to_string());
    assert_eq!(pins[0]["view_kind"], "filter_series");
    assert_eq!(pins[0]["view_name"], "Unread Marvel");
    assert_eq!(pins[0]["icon"], "sparkles");

    // ── sidebar ──
    let sidebar = s["sidebar"].as_array().unwrap();
    assert_eq!(sidebar.len(), 1);
    assert_eq!(sidebar[0]["kind"], "builtin");
    assert_eq!(sidebar[0]["ref_id"], "bookmarks");
    assert_eq!(sidebar[0]["label"], "Pins");

    // ── rail dismissals ──
    let dismissals = s["rail_dismissals"].as_array().unwrap();
    assert_eq!(dismissals.len(), 1);
    assert_eq!(dismissals[0]["target_kind"], "issue");
    assert_eq!(dismissals[0]["target_id"], seeded.issue_id);

    // ── reading log ──
    let log = s["reading_log"].as_array().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["id"], seeded.session_id.to_string());
    assert_issue_ref(&log[0]["issue"], &seeded);
    assert_eq!(log[0]["active_ms"], 120_000);
    assert_eq!(log[0]["device"], "desktop");

    // ── preferences ──
    assert_eq!(s["preferences"]["keybinds"]["next_page"], "l");
    assert_eq!(s["preferences"]["theme"], "amber");
    assert!(s["preferences"]["timezone"].is_string());

    // ── user B sees none of A's rows ──
    let resp = get_export(&app, &b).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let doc_b = body_json(resp.into_body()).await;
    assert_eq!(doc_b["user"]["id"], b.user_id.to_string());
    let sb = &doc_b["sections"];
    let progress_b = sb["progress"].as_array().unwrap();
    assert_eq!(progress_b.len(), 1, "B keeps B's own progress row");
    assert_eq!(progress_b[0]["last_page"], 1);
    for section in [
        "markers",
        "collections",
        "saved_views",
        "ratings",
        "sidebar",
        "rail_dismissals",
        "reading_log",
    ] {
        assert_eq!(
            sb[section].as_array().map(Vec::len),
            Some(0),
            "section {section} should be empty for B: {}",
            sb[section]
        );
    }
    assert!(sb["want_to_read"].is_null(), "B never seeded Want to Read");
    assert!(
        sb["custom_pages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["id"] != seeded.page_id.to_string()),
        "A's custom page leaked into B's export"
    );
    assert_eq!(sb["preferences"]["keybinds"], json!({}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_requires_auth() {
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/me/export")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body = body_json(resp.into_body()).await;
    assert!(
        body["error"]["code"].is_string(),
        "canonical envelope: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_is_json_even_when_the_user_owns_nothing() {
    let app = TestApp::spawn().await;
    let a = register(&app, "empty@example.com").await;
    let resp = get_export(&app, &a).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let doc = body_json(resp.into_body()).await;
    assert_eq!(doc["format"], "folio-user-export");
    for section in [
        "progress",
        "markers",
        "collections",
        "saved_views",
        "ratings",
        "custom_pages",
        "sidebar",
        "rail_dismissals",
        "reading_log",
    ] {
        assert!(
            doc["sections"][section].is_array(),
            "section {section} must be an array: {}",
            doc["sections"][section]
        );
    }
    assert!(doc["sections"]["preferences"].is_object());
}
