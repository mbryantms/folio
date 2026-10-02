//! Roadmap WP-8.4 — `POST /me/markers/restore`, the single-request Undo
//! for marker deletes, and the `marker_write` burst it made safe to cut.
//!
//! Restore re-inserts the snapshots a client captured before deleting:
//! one request for any number of markers, keeping each one's id,
//! created_at, page hash, colour, region, selection, tags and note body.
//! Rows are always written for the caller, ids that still exist are never
//! overwritten, and issues the caller can't see are skipped.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{SeriesSeed, seed_issue, seed_library};
use entity::{library_user_access, marker};
use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, Set};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
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
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    Authed {
        session,
        csrf,
        user_id: Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap(),
    }
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: &Authed,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf);
    let req = match body {
        Some(b) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

struct Fixture {
    app: TestApp,
    admin: Authed,
    lib: Uuid,
    issue: String,
    _tmp: tempfile::TempDir,
}

/// First registered user is the admin (sees every library).
async fn setup() -> Fixture {
    let app = TestApp::spawn().await;
    let admin = register(&app, "restorer@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = SeriesSeed::new(lib, "Undoable").insert(&db).await;
    let issue = seed_issue(&db, lib, series, &tmp.path().join("u.cbz"), b"u", 1.0).await;
    Fixture {
        app,
        admin,
        lib,
        issue,
        _tmp: tmp,
    }
}

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// Create a spread of markers through the API and return the snapshots
/// the web client would hold (the per-issue list, which includes
/// `page_hash`). One marker gets a page hash written directly — the test
/// archive isn't a real CBZ, so capture stores none.
async fn seed_markers(fx: &Fixture) -> Vec<Value> {
    let bodies = [
        json!({"issue_id": fx.issue, "page_index": 0, "kind": "bookmark", "color": "red"}),
        json!({"issue_id": fx.issue, "page_index": 3, "kind": "note",
               "body": "remember this panel", "tags": ["Plot", "twist"], "is_favorite": true}),
        json!({"issue_id": fx.issue, "page_index": 5, "kind": "highlight",
               "region": {"x": 10.0, "y": 20.0, "w": 30.0, "h": 15.0, "shape": "text"},
               "selection": {"text": "BLAM", "ocr_confidence": 0.9},
               "color": "#336699", "tags": ["sfx"]}),
        json!({"issue_id": fx.issue, "page_index": 7, "kind": "favorite"}),
    ];
    for b in bodies {
        let (s, v) = http(&fx.app, Method::POST, "/api/me/markers", &fx.admin, Some(b)).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
    }
    let db = &fx.app.state().db;
    marker::Entity::update_many()
        .col_expr(
            marker::Column::PageHash,
            sea_orm::sea_query::Expr::value(HASH_A),
        )
        .filter(marker::Column::IssueId.eq(fx.issue.clone()))
        .filter(marker::Column::PageIndex.eq(5))
        .exec(db)
        .await
        .unwrap();
    let (s, list) = http(
        &fx.app,
        Method::GET,
        &format!("/api/me/issues/{}/markers", fx.issue),
        &fx.admin,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    list["items"].as_array().unwrap().clone()
}

async fn bulk_delete(fx: &Fixture, snapshots: &[Value]) {
    let ids: Vec<&str> = snapshots
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    let (s, v) = http(
        &fx.app,
        Method::POST,
        "/api/me/markers/bulk-delete",
        &fx.admin,
        Some(json!({ "marker_ids": ids })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["deleted"], snapshots.len());
}

/// Fields the restore must keep, compared snapshot-to-row.
const KEPT: &[&str] = &[
    "id",
    "user_id",
    "series_id",
    "issue_id",
    "page_index",
    "kind",
    "is_favorite",
    "tags",
    "region",
    "selection",
    "body",
    "color",
    "page_hash",
    "created_at",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_brings_back_every_marker_in_one_request() {
    let fx = setup().await;
    let snapshots = seed_markers(&fx).await;
    assert_eq!(snapshots.len(), 4);
    assert!(
        snapshots.iter().any(|m| m["page_hash"] == HASH_A),
        "the per-issue list exposes page_hash"
    );
    bulk_delete(&fx, &snapshots).await;

    let (s, v) = http(
        &fx.app,
        Method::POST,
        "/api/me/markers/restore",
        &fx.admin,
        Some(json!({ "markers": snapshots })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["skipped"], 0, "{v}");
    let restored = v["restored"].as_array().unwrap();
    assert_eq!(restored.len(), 4);

    // Every kept field round-trips, in request order.
    for (snap, row) in snapshots.iter().zip(restored) {
        for f in KEPT {
            assert_eq!(snap[f], row[f], "field {f}: {snap:#} vs {row:#}");
        }
    }
    // And the database agrees (not just the response).
    let (_, list) = http(
        &fx.app,
        Method::GET,
        &format!("/api/me/issues/{}/markers", fx.issue),
        &fx.admin,
        None,
    )
    .await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    for snap in &snapshots {
        let row = items
            .iter()
            .find(|m| m["id"] == snap["id"])
            .expect("restored row");
        for f in KEPT {
            assert_eq!(snap[f], row[f], "field {f}");
        }
    }

    // A repeated Undo is a no-op: every id exists, nothing changes.
    let (s, v) = http(
        &fx.app,
        Method::POST,
        "/api/me/markers/restore",
        &fx.admin,
        Some(json!({ "markers": snapshots })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["restored"].as_array().unwrap().len(), 0);
    assert_eq!(v["skipped"], 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_never_takes_over_another_users_marker() {
    let fx = setup().await;
    let snapshots = seed_markers(&fx).await;
    let other = register(&fx.app, "intruder@example.com").await;
    let db = Database::connect(&fx.app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(other.user_id),
        library_id: Set(fx.lib),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();

    // The admin's markers are live: replaying their ids as another user
    // is skipped, and the rows keep their owner and content.
    let mut forged = snapshots[1].clone();
    forged["body"] = json!("overwritten");
    let (s, v) = http(
        &fx.app,
        Method::POST,
        "/api/me/markers/restore",
        &other,
        Some(json!({ "markers": [forged] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["skipped"], 1);
    let id = Uuid::parse_str(snapshots[1]["id"].as_str().unwrap()).unwrap();
    let row = marker::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.user_id, fx.admin.user_id);
    assert_eq!(row.body.as_deref(), Some("remember this panel"));

    // A snapshot whose id is free is written for the *caller*, whatever
    // `user_id` the payload claims.
    let mut fresh = snapshots[0].clone();
    fresh["id"] = json!(Uuid::now_v7());
    fresh["user_id"] = json!(fx.admin.user_id);
    let (s, v) = http(
        &fx.app,
        Method::POST,
        "/api/me/markers/restore",
        &other,
        Some(json!({ "markers": [fresh] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["restored"][0]["user_id"], json!(other.user_id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_skips_issues_the_caller_cannot_see() {
    let fx = setup().await;
    let snapshots = seed_markers(&fx).await;
    bulk_delete(&fx, &snapshots).await;
    // A second, non-admin user with no grant on the library.
    let outsider = register(&fx.app, "outsider@example.com").await;
    let (s, v) = http(
        &fx.app,
        Method::POST,
        "/api/me/markers/restore",
        &outsider,
        Some(json!({ "markers": snapshots })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["restored"].as_array().unwrap().len(), 0);
    assert_eq!(v["skipped"], 4);
    let db = Database::connect(&fx.app.db_url).await.unwrap();
    let n = marker::Entity::find()
        .filter(marker::Column::IssueId.eq(fx.issue.clone()))
        .all(&db)
        .await
        .unwrap()
        .len();
    assert_eq!(n, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_validates_every_snapshot() {
    let fx = setup().await;
    let base = json!({
        "id": Uuid::now_v7(),
        "issue_id": fx.issue,
        "page_index": 0,
        "kind": "bookmark",
    });
    let check = |field: &'static str, patch: Value| {
        let mut m = base.clone();
        for (k, v) in patch.as_object().unwrap() {
            m[k] = v.clone();
        }
        (field, m)
    };
    for (field, m) in [
        check("markers[0].color", json!({"color": "chartreuse"})),
        check("markers[0].page_hash", json!({"page_hash": "XYZ"})),
        check("markers[0].kind", json!({"kind": "sticker"})),
    ] {
        let (s, v) = http(
            &fx.app,
            Method::POST,
            "/api/me/markers/restore",
            &fx.admin,
            Some(json!({ "markers": [m] })),
        )
        .await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
        let fields: Vec<&str> = v["error"]["details"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["field"].as_str().unwrap())
            .collect();
        assert!(fields.contains(&field), "{field} not in {fields:?}");
    }
    // A note without a body fails the per-kind rule like create does.
    let mut note = base.clone();
    note["kind"] = json!("note");
    let (s, v) = http(
        &fx.app,
        Method::POST,
        "/api/me/markers/restore",
        &fx.admin,
        Some(json!({ "markers": [note] })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    // More than the bulk-delete cap of 500.
    let many: Vec<Value> = (0..501)
        .map(|_| {
            let mut m = base.clone();
            m["id"] = json!(Uuid::now_v7());
            m
        })
        .collect();
    let (s, _) = http(
        &fx.app,
        Method::POST,
        "/api/me/markers/restore",
        &fx.admin,
        Some(json!({ "markers": many })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_costs_one_token_and_the_burst_is_sixty() {
    let fx = setup().await;
    let snapshots = seed_markers(&fx).await; // 4 writes
    bulk_delete(&fx, &snapshots).await; // 1 write
    // Restoring four markers is ONE write against the bucket.
    let (s, _) = http(
        &fx.app,
        Method::POST,
        "/api/me/markers/restore",
        &fx.admin,
        Some(json!({ "markers": snapshots })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // Drain the rest with empty restores: the bucket trips near 60 total
    // writes (plus whatever refilled at 10/s while the loop ran), far
    // below the old 600 burst.
    let mut allowed = 6;
    let mut limited = None;
    for _ in 0..600 {
        let (s, b) = http(
            &fx.app,
            Method::POST,
            "/api/me/markers/restore",
            &fx.admin,
            Some(json!({ "markers": [] })),
        )
        .await;
        if s == StatusCode::TOO_MANY_REQUESTS {
            limited = Some(b);
            break;
        }
        assert_eq!(s, StatusCode::OK);
        allowed += 1;
    }
    let body = limited.expect("marker write bucket never tripped");
    assert_eq!(body["error"]["code"], "rate_limited");
    assert!(allowed >= 60, "tripped after only {allowed} writes");
    assert!(allowed < 300, "burst looks like the old 600: {allowed}");
}
