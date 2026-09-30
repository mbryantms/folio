//! Issue-level smart views (WP-5.4) — integration coverage.
//!
//! `kind = 'filter_issues'` views compile against `issues` (joined to the
//! parent series) and are served by `/me/saved-views/{id}/issue-results`
//! and `/me/saved-views/preview-issues`. The anchor scenario is the
//! roadmap's "unread annuals 2019" rail: pinned to the home page, it
//! returns exactly the caller's unread 2019 annuals.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{seed_issue, seed_library, seed_progress, seed_series};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

async fn body_json(b: Body) -> Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
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
    (status, body_json(resp.into_body()).await)
}

/// Stamp issue-level metadata the seed builder leaves NULL.
async fn set_issue(
    db: &DatabaseConnection,
    id: &str,
    year: i32,
    special_type: Option<&str>,
    story_arc: Option<&str>,
) {
    db.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "UPDATE issues SET year = $2, special_type = $3, story_arc = $4 WHERE id = $1",
        [
            id.into(),
            year.into(),
            special_type.map(str::to_owned).into(),
            story_arc.map(str::to_owned).into(),
        ],
    ))
    .await
    .unwrap();
}

struct Fixture {
    annual_2019_unread: String,
    annual_2019_read: String,
    annual_2019_started: String,
    annual_2018: String,
    regular_2019: String,
}

async fn seed(app: &TestApp, tmp: &std::path::Path, user_id: Uuid) -> Fixture {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = seed_library(&db, tmp).await;
    let s = seed_series(&db, lib, "Batman").await;
    let mk = |n: u8| tmp.join(format!("b{n}.cbz"));
    let a = seed_issue(&db, lib, s, &mk(1), b"annual-a", 1.0).await;
    let b = seed_issue(&db, lib, s, &mk(2), b"annual-b", 2.0).await;
    let c = seed_issue(&db, lib, s, &mk(3), b"annual-c", 3.0).await;
    let d = seed_issue(&db, lib, s, &mk(4), b"annual-d", 4.0).await;
    let e = seed_issue(&db, lib, s, &mk(5), b"regular-e", 5.0).await;
    set_issue(&db, &a, 2019, Some("Annual"), Some("Knightfall")).await;
    set_issue(&db, &b, 2019, Some("Annual"), None).await;
    set_issue(&db, &c, 2019, Some("Annual"), Some("  ")).await;
    set_issue(&db, &d, 2018, Some("Annual"), None).await;
    set_issue(&db, &e, 2019, None, None).await;
    seed_progress(&db, user_id, &b, 19, 1.0, true).await;
    seed_progress(&db, user_id, &c, 5, 0.25, false).await;
    Fixture {
        annual_2019_unread: a,
        annual_2019_read: b,
        annual_2019_started: c,
        annual_2018: d,
        regular_2019: e,
    }
}

fn ids(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect()
}

fn unread_annuals_2019() -> Value {
    json!({
        "match_mode": "all",
        "conditions": [
            {"field": "special_type", "op": "is", "value": "Annual"},
            {"field": "year", "op": "equals", "value": 2019},
            {"field": "read_status", "op": "is", "value": "unread"},
        ]
    })
}

#[tokio::test]
async fn unread_annuals_2019_rail_renders_pinned() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "reader@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let fx = seed(&app, tmp.path(), auth.user_id).await;

    let (status, view) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &auth,
        Some(json!({
            "kind": "filter_issues",
            "name": "Unread annuals 2019",
            "filter": unread_annuals_2019(),
            "sort_field": "name",
            "sort_order": "asc",
            "result_limit": 12,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{view}");
    assert_eq!(view["kind"], "filter_issues");
    let id = view["id"].as_str().unwrap().to_owned();

    // Rails accept issue views: pin it to Home and it shows in the pinned list.
    let (status, _) = http(
        &app,
        Method::POST,
        &format!("/api/me/saved-views/{id}/pin"),
        &auth,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, pinned) = http(
        &app,
        Method::GET,
        "/api/me/saved-views?pinned=true",
        &auth,
        None,
    )
    .await;
    assert!(
        pinned["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == id.as_str() && v["kind"] == "filter_issues")
    );

    // The rail body fetch.
    let (status, res) = http(
        &app,
        Method::GET,
        &format!("/api/me/saved-views/{id}/issue-results"),
        &auth,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(ids(&res), vec![fx.annual_2019_unread.clone()]);
    let card = &res["items"][0];
    assert_eq!(card["series_name"], "Batman");
    assert_eq!(card["special_type"], "Annual");
    assert!(card["cover_url"].as_str().unwrap().ends_with("/thumb"));
    assert!(res["next_cursor"].is_null());
    let excluded = [
        &fx.annual_2019_read,
        &fx.annual_2019_started,
        &fx.annual_2018,
        &fx.regular_2019,
    ];
    for x in excluded {
        assert!(!ids(&res).contains(x));
    }

    // The series-results endpoint refuses issue views rather than lying.
    let (status, err) = http(
        &app,
        Method::GET,
        &format!("/api/me/saved-views/{id}/results"),
        &auth,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(err["error"]["code"], "unsupported_view_kind");
}

#[tokio::test]
async fn issue_results_paginate_with_opaque_cursor() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "pager@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let fx = seed(&app, tmp.path(), auth.user_id).await;
    let (_, view) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &auth,
        Some(json!({
            "kind": "filter_issues",
            "name": "All annuals",
            "filter": {"match_mode": "all", "conditions": [
                {"field": "special_type", "op": "is", "value": "Annual"}
            ]},
            "sort_field": "name",
            "sort_order": "asc",
            "result_limit": 12,
        })),
    )
    .await;
    let id = view["id"].as_str().unwrap();
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let uri = match &cursor {
            Some(c) => format!("/api/me/saved-views/{id}/issue-results?limit=3&cursor={c}"),
            None => format!("/api/me/saved-views/{id}/issue-results?limit=3"),
        };
        let (status, page) = http(&app, Method::GET, &uri, &auth, None).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        seen.extend(ids(&page));
        match page["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    // Series name then issue number: #1..#4 are the annuals.
    assert_eq!(
        seen,
        vec![
            fx.annual_2019_unread,
            fx.annual_2019_read,
            fx.annual_2019_started,
            fx.annual_2018
        ]
    );
    let (status, _) = http(
        &app,
        Method::GET,
        &format!("/api/me/saved-views/{id}/issue-results?cursor=garbage"),
        &auth,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn preview_issues_supports_is_empty_and_rating() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "preview@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let fx = seed(&app, tmp.path(), auth.user_id).await;

    // Blank story arcs count as empty; only #1 carries a real arc.
    let (status, res) = http(
        &app,
        Method::POST,
        "/api/me/saved-views/preview-issues",
        &auth,
        Some(json!({
            "filter": {"match_mode": "all", "conditions": [
                {"field": "special_type", "op": "is_not_empty"},
                {"field": "story_arc", "op": "is_empty"},
            ]},
            "sort_field": "name",
            "sort_order": "asc",
            "result_limit": 50,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(
        ids(&res),
        vec![
            fx.annual_2019_read.clone(),
            fx.annual_2019_started.clone(),
            fx.annual_2018.clone()
        ]
    );

    // Rating reads the caller's own per-issue rating.
    let db = Database::connect(&app.db_url).await.unwrap();
    db.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "INSERT INTO user_ratings (user_id, target_type, target_id, rating, created_at, updated_at) \
         VALUES ($1, 'issue', $2, 4.5, now(), now())",
        [auth.user_id.into(), fx.regular_2019.clone().into()],
    ))
    .await
    .unwrap();
    let (status, res) = http(
        &app,
        Method::POST,
        "/api/me/saved-views/preview-issues",
        &auth,
        Some(json!({
            "filter": {"match_mode": "all", "conditions": [
                {"field": "rating", "op": "gte", "value": 4}
            ]},
            "sort_field": "created_at",
            "sort_order": "desc",
            "result_limit": 50,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(ids(&res), vec![fx.regular_2019]);
}

#[tokio::test]
async fn issue_views_validate_entity_fields_and_sorts() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "validate@example.com").await;

    // Series-only rollup on an issue view.
    let (status, err) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &auth,
        Some(json!({
            "kind": "filter_issues",
            "name": "bad",
            "filter": {"match_mode": "all", "conditions": [
                {"field": "unread_issues", "op": "gt", "value": 2}
            ]},
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(err["error"]["code"], "filter_invalid");

    // Issue-only field on a series view.
    let (status, _) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &auth,
        Some(json!({
            "kind": "filter_series",
            "name": "bad",
            "filter": {"match_mode": "all", "conditions": [
                {"field": "special_type", "op": "is", "value": "TPB"}
            ]},
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Per-user sort isn't available on issue views — create and PATCH.
    let (status, _) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &auth,
        Some(json!({
            "kind": "filter_issues",
            "name": "bad",
            "filter": {"match_mode": "all", "conditions": []},
            "sort_field": "read_progress",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, view) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &auth,
        Some(json!({
            "kind": "filter_issues",
            "name": "ok",
            "filter": {"match_mode": "all", "conditions": []},
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = view["id"].as_str().unwrap();
    let (status, _) = http(
        &app,
        Method::PATCH,
        &format!("/api/me/saved-views/{id}"),
        &auth,
        Some(json!({"sort_field": "last_read"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn issue_results_respect_library_acl() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    seed(&app, tmp.path(), admin.user_id).await;
    // Second registration is a plain user with no library grants.
    let user = register(&app, "nogrant@example.com").await;
    let (_, view) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &user,
        Some(json!({
            "kind": "filter_issues",
            "name": "Everything",
            "filter": {"match_mode": "all", "conditions": []},
        })),
    )
    .await;
    let id = view["id"].as_str().unwrap();
    let (status, res) = http(
        &app,
        Method::GET,
        &format!("/api/me/saved-views/{id}/issue-results"),
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(ids(&res).is_empty(), "{res}");
    // The owner's view is private to them.
    let (status, _) = http(
        &app,
        Method::GET,
        &format!("/api/me/saved-views/{id}/issue-results"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
