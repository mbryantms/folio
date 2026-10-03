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

async fn create_view(app: &TestApp, auth: &Authed, sort_field: &str, sort_order: &str) -> String {
    let (status, view) = http(
        app,
        Method::POST,
        "/api/me/saved-views",
        auth,
        Some(json!({
            "kind": "filter_issues",
            "name": "All annuals",
            "filter": {"match_mode": "all", "conditions": [
                {"field": "special_type", "op": "is", "value": "Annual"}
            ]},
            "sort_field": sort_field,
            "sort_order": sort_order,
            "result_limit": 12,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{view}");
    view["id"].as_str().unwrap().to_owned()
}

async fn page(app: &TestApp, auth: &Authed, id: &str, cursor: Option<&str>) -> Value {
    let uri = match cursor {
        Some(c) => format!("/api/me/saved-views/{id}/issue-results?limit=2&cursor={c}"),
        None => format!("/api/me/saved-views/{id}/issue-results?limit=2"),
    };
    let (status, page) = http(app, Method::GET, &uri, auth, None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    page
}

async fn walk(app: &TestApp, auth: &Authed, id: &str) -> Vec<String> {
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let p = page(app, auth, id, cursor.as_deref()).await;
        seen.extend(ids(&p));
        match p["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => return seen,
        }
    }
}

/// Keyset paging (owner decision 2026-09-30): issues inserted between page
/// fetches — one behind the cursor, one ahead of it — must neither shift
/// a row onto the next page twice nor push one past it. Offset paging
/// would repeat #2 here.
#[tokio::test]
async fn issue_results_keyset_survives_mid_scroll_inserts() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "pager@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let fx = seed(&app, tmp.path(), auth.user_id).await;
    let id = create_view(&app, &auth, "name", "asc").await;

    let first = page(&app, &auth, &id, None).await;
    assert_eq!(
        ids(&first),
        vec![fx.annual_2019_unread.clone(), fx.annual_2019_read.clone()]
    );
    assert_eq!(first["total"], 4, "total rides the first page");
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();

    // Mid-scroll inserts into the same series: #1.5 lands behind the
    // cursor, #3.5 ahead of it.
    let db = Database::connect(&app.db_url).await.unwrap();
    let (lib, series) = {
        use sea_orm::FromQueryResult;
        #[derive(FromQueryResult)]
        struct R {
            library_id: Uuid,
            series_id: Uuid,
        }
        let r = R::find_by_statement(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT library_id, series_id FROM issues WHERE id = $1",
            [fx.annual_2019_unread.clone().into()],
        ))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
        (r.library_id, r.series_id)
    };
    let behind = seed_issue(
        &db,
        lib,
        series,
        &tmp.path().join("b15.cbz"),
        b"annual-15",
        1.5,
    )
    .await;
    set_issue(&db, &behind, 2019, Some("Annual"), None).await;
    let ahead = seed_issue(
        &db,
        lib,
        series,
        &tmp.path().join("b35.cbz"),
        b"annual-35",
        3.5,
    )
    .await;
    set_issue(&db, &ahead, 2019, Some("Annual"), None).await;

    let mut seen = ids(&first);
    let mut cursor = Some(cursor);
    while let Some(c) = cursor {
        let p = page(&app, &auth, &id, Some(&c)).await;
        assert!(
            p.get("total").is_none(),
            "total only on the first page: {p}"
        );
        seen.extend(ids(&p));
        cursor = p["next_cursor"].as_str().map(str::to_owned);
    }
    assert_eq!(
        seen,
        vec![
            fx.annual_2019_unread.clone(),
            fx.annual_2019_read.clone(),
            fx.annual_2019_started.clone(),
            ahead,
            fx.annual_2018.clone(),
        ],
        "no row skipped, none repeated, the row behind the cursor not shown"
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

/// Every issue-view sort pages completely and in the same order as one
/// big page — including the nullable `year` key (NULLS LAST) and ties.
#[tokio::test]
async fn issue_results_keyset_walks_every_sort() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "sorts@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let fx = seed(&app, tmp.path(), auth.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    // One annual with no year: sorts last in both directions.
    db.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "UPDATE issues SET year = NULL WHERE id = $1",
        [fx.annual_2019_read.clone().into()],
    ))
    .await
    .unwrap();

    for (sort, order) in [
        ("name", "asc"),
        ("name", "desc"),
        ("year", "asc"),
        ("year", "desc"),
        ("created_at", "desc"),
        ("updated_at", "asc"),
    ] {
        let id = create_view(&app, &auth, sort, order).await;
        let walked = walk(&app, &auth, &id).await;
        let (_, all) = http(
            &app,
            Method::GET,
            &format!("/api/me/saved-views/{id}/issue-results?limit=50"),
            &auth,
            None,
        )
        .await;
        assert_eq!(walked, ids(&all), "{sort} {order}");
        assert_eq!(walked.len(), 4, "{sort} {order}");
        if sort == "year" {
            assert_eq!(
                walked.last(),
                Some(&fx.annual_2019_read),
                "NULLS LAST ({order})"
            );
        }
    }
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

// ───── WP-5.7: has_notes / has_bookmarks / has_highlights ─────

async fn add_marker(db: &DatabaseConnection, user: Uuid, issue_id: &str, kind: &str) {
    db.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "INSERT INTO markers (id, user_id, series_id, issue_id, page_index, kind, \
             is_favorite, tags, region, body, created_at, updated_at, hidden_from_log) \
         SELECT $1, $2, i.series_id, i.id, 0, $4, false, ARRAY[]::text[], \
             CASE WHEN $4 = 'highlight' \
                  THEN '{\"x\":1,\"y\":1,\"w\":5,\"h\":5,\"shape\":\"rect\"}'::jsonb END, \
             CASE WHEN $4 = 'note' THEN 'a note' END, now(), now(), false \
         FROM issues i WHERE i.id = $3",
        [
            Uuid::now_v7().into(),
            user.into(),
            issue_id.into(),
            kind.into(),
        ],
    ))
    .await
    .unwrap();
}

async fn preview_ids(app: &TestApp, auth: &Authed, path: &str, conditions: Value) -> Value {
    let (status, res) = http(
        app,
        Method::POST,
        path,
        auth,
        Some(json!({
            "filter": {"match_mode": "all", "conditions": conditions},
            "sort_field": "name",
            "sort_order": "asc",
            "result_limit": 50,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    res
}

fn names(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn marker_filters_are_scoped_to_the_viewing_user() {
    let app = TestApp::spawn().await;
    let me = register(&app, "annotator@example.com").await;
    let other = register(&app, "other@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::connect(&app.db_url).await.unwrap();
    let fx = seed(&app, tmp.path(), me.user_id).await;
    // A second series the caller never annotates.
    let lib: Uuid = {
        use sea_orm::FromQueryResult;
        #[derive(FromQueryResult)]
        struct R {
            library_id: Uuid,
        }
        R::find_by_statement(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT library_id FROM issues WHERE id = $1",
            [fx.regular_2019.clone().into()],
        ))
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .library_id
    };
    let quiet = seed_series(&db, lib, "Quiet").await;
    let quiet_issue =
        seed_issue(&db, lib, quiet, &tmp.path().join("q1.cbz"), b"quiet-1", 1.0).await;

    add_marker(&db, me.user_id, &fx.annual_2019_unread, "note").await;
    add_marker(&db, me.user_id, &fx.annual_2018, "bookmark").await;
    add_marker(&db, me.user_id, &fx.regular_2019, "highlight").await;
    // Another user's annotations must never make a row match for `me`.
    add_marker(&db, other.user_id, &quiet_issue, "note").await;
    add_marker(&db, other.user_id, &fx.annual_2019_read, "note").await;

    let issues = "/api/me/saved-views/preview-issues";
    let series = "/api/me/saved-views/preview";
    let flag = |field: &str, op: &str| json!([{"field": field, "op": op}]);

    let r = preview_ids(&app, &me, issues, flag("has_notes", "is_true")).await;
    assert_eq!(ids(&r), vec![fx.annual_2019_unread.clone()]);
    let r = preview_ids(&app, &me, issues, flag("has_bookmarks", "is_true")).await;
    assert_eq!(ids(&r), vec![fx.annual_2018.clone()]);
    let r = preview_ids(&app, &me, issues, flag("has_highlights", "is_true")).await;
    assert_eq!(ids(&r), vec![fx.regular_2019.clone()]);
    let r = preview_ids(&app, &me, issues, flag("has_notes", "is_false")).await;
    let without_notes = ids(&r);
    assert!(!without_notes.contains(&fx.annual_2019_unread));
    assert!(without_notes.contains(&fx.annual_2019_read));
    assert!(without_notes.contains(&quiet_issue));

    // Series level: Batman has my note; Quiet only has someone else's.
    let r = preview_ids(&app, &me, series, flag("has_notes", "is_true")).await;
    assert_eq!(names(&r), vec!["Batman"]);
    let r = preview_ids(&app, &me, series, flag("has_notes", "is_false")).await;
    assert_eq!(names(&r), vec!["Quiet"]);

    // A note on a removed issue no longer counts for its series.
    db.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "UPDATE issues SET removed_at = now() WHERE id = $1",
        [fx.annual_2019_unread.clone().into()],
    ))
    .await
    .unwrap();
    let r = preview_ids(&app, &me, series, flag("has_notes", "is_true")).await;
    assert!(names(&r).is_empty(), "{r}");
}

// ───── WP-8.4: has_favorites ─────

#[tokio::test]
async fn has_favorites_matches_favorite_kind_or_starred_markers() {
    let app = TestApp::spawn().await;
    let me = register(&app, "stars@example.com").await;
    let other = register(&app, "other-stars@example.com").await;
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::connect(&app.db_url).await.unwrap();
    let fx = seed(&app, tmp.path(), me.user_id).await;

    // A `favorite`-kind marker, a starred note, an unstarred bookmark,
    // and someone else's favourite.
    add_marker(&db, me.user_id, &fx.annual_2018, "favorite").await;
    add_marker(&db, me.user_id, &fx.regular_2019, "note").await;
    db.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "UPDATE markers SET is_favorite = true WHERE user_id = $1 AND issue_id = $2",
        [me.user_id.into(), fx.regular_2019.clone().into()],
    ))
    .await
    .unwrap();
    add_marker(&db, me.user_id, &fx.annual_2019_unread, "bookmark").await;
    add_marker(&db, other.user_id, &fx.annual_2019_read, "favorite").await;

    let issues = "/api/me/saved-views/preview-issues";
    let series = "/api/me/saved-views/preview";
    let flag = |op: &str| json!([{"field": "has_favorites", "op": op}]);

    let r = preview_ids(&app, &me, issues, flag("is_true")).await;
    let mut got = ids(&r);
    got.sort();
    let mut want = vec![fx.annual_2018.clone(), fx.regular_2019.clone()];
    want.sort();
    assert_eq!(got, want, "{r}");

    let r = preview_ids(&app, &me, issues, flag("is_false")).await;
    let without = ids(&r);
    assert!(without.contains(&fx.annual_2019_unread), "{r}");
    // Another user's favourite never counts for `me`.
    assert!(without.contains(&fx.annual_2019_read), "{r}");
    assert!(!without.contains(&fx.annual_2018), "{r}");

    let r = preview_ids(&app, &me, series, flag("is_true")).await;
    assert_eq!(names(&r), vec!["Batman"]);

    // Saved as an issue view, it round-trips through validation.
    let (status, body) = http(
        &app,
        Method::POST,
        "/api/me/saved-views",
        &me,
        Some(json!({
            "kind": "filter_issues",
            "name": "Starred issues",
            "filter": {"match_mode": "all", "conditions": flag("is_true")},
            "sort_field": "name",
            "sort_order": "asc",
            "result_limit": 12,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

// ───── WP-8.4: issue views in the OPDS personal feeds ─────

async fn get_raw(app: &TestApp, uri: &str, auth: &Authed) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

/// The Atom document parses end to end (the OPDS 1.x suites check
/// content by substring; this also proves it is well-formed XML).
fn assert_well_formed_xml(xml: &str) {
    let mut reader = quick_xml::reader::Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(e) => panic!("malformed XML at {}: {e}\n{xml}", reader.buffer_position()),
        }
    }
}

#[tokio::test]
async fn issue_views_appear_in_the_opds_personal_feeds() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "opds-issues@example.com").await;
    let stranger = register(&app, "opds-stranger@example.com").await;
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
    let id = view["id"].as_str().unwrap().to_owned();
    let (status, _) = http(
        &app,
        Method::POST,
        &format!("/api/me/saved-views/{id}/pin"),
        &auth,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Same result set the web rail shows.
    let (_, web) = http(
        &app,
        Method::GET,
        &format!("/api/me/saved-views/{id}/issue-results"),
        &auth,
        None,
    )
    .await;
    let want = ids(&web);
    assert_eq!(want, vec![fx.annual_2019_unread.clone()]);

    // ── OPDS 1.x ──
    let (s, nav) = get_raw(&app, "/opds/v1/views", &auth).await;
    assert_eq!(s, StatusCode::OK);
    assert_well_formed_xml(&nav);
    assert!(
        nav.contains(&format!(r#"href="/opds/v1/views/{id}""#)),
        "{nav}"
    );
    let (s, acq) = get_raw(&app, &format!("/opds/v1/views/{id}"), &auth).await;
    assert_eq!(s, StatusCode::OK, "{acq}");
    assert_well_formed_xml(&acq);
    assert!(acq.contains("<title>Unread annuals 2019</title>"), "{acq}");
    assert_eq!(acq.matches("<entry>").count(), 1, "{acq}");
    assert!(acq.contains(&fx.annual_2019_unread), "{acq}");
    assert!(!acq.contains(&fx.annual_2018), "{acq}");
    assert!(
        acq.contains(r#"rel="http://opds-spec.org/acquisition""#),
        "issue entries are acquisitions: {acq}"
    );
    let (s, page) = get_raw(&app, "/opds/v1/pages/home", &auth).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        page.contains(&format!(r#"href="/opds/v1/views/{id}""#)),
        "{page}"
    );

    // ── OPDS 2.0 ──
    let (s, nav) = http(&app, Method::GET, "/opds/v2/views", &auth, None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        nav["navigation"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["href"] == format!("/opds/v2/views/{id}")),
        "{nav}"
    );
    let (s, feed) = http(
        &app,
        Method::GET,
        &format!("/opds/v2/views/{id}"),
        &auth,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{feed}");
    assert_eq!(feed["metadata"]["title"], "Unread annuals 2019");
    assert_eq!(feed["links"][0]["rel"], "self");
    let got: Vec<String> = feed["publications"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            p["metadata"]["identifier"]
                .as_str()
                .unwrap()
                .trim_start_matches("urn:folio:issue:")
                .to_owned()
        })
        .collect();
    assert_eq!(got, want);
    let (s, page) = http(&app, Method::GET, "/opds/v2/pages/home", &auth, None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        page["navigation"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["href"] == format!("/opds/v2/views/{id}")),
        "{page}"
    );

    // Private to its owner on both protocols.
    let (s, _) = get_raw(&app, &format!("/opds/v1/views/{id}"), &stranger).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = get_raw(&app, &format!("/opds/v2/views/{id}"), &stranger).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}
