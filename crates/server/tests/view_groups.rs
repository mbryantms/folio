//! Views-index groups — `/me/view-groups` integration coverage.
//!
//! Exercises create / list / rename+collapse / delete / wholesale layout
//! replace, plus the invariants that matter for the drag-and-drop board:
//! a view sits in one group at a time, deleting a group ungroups its
//! members (cascade), the layout write must name every group exactly
//! once, foreign or unknown view ids are 422, and groups are isolated
//! per user.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::saved_view::ActiveModel as SavedViewAM;
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

/// Seed a user-owned filter view directly (the group endpoints only care
/// that the row exists and is visible to the caller).
async fn seed_view(app: &TestApp, owner: &Authed, name: &str) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    SavedViewAM {
        id: Set(id),
        user_id: Set(Some(owner.user_id)),
        kind: Set(entity::saved_view::SavedViewKind::FilterSeries),
        system_key: Set(None),
        name: Set(name.into()),
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
        preserve_canonical_order: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

fn ids(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect()
}

async fn create_group(
    app: &TestApp,
    user: &Authed,
    scope: &str,
    name: &str,
    view_ids: &[Uuid],
) -> serde_json::Value {
    let (status, body) = http(
        app,
        Method::POST,
        "/api/me/view-groups",
        Some(user),
        Some(serde_json::json!({ "scope": scope, "name": name, "view_ids": view_ids })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

// ───── tests ─────

#[tokio::test]
async fn list_is_empty_for_fresh_user_and_rejects_bad_scope() {
    let app = TestApp::spawn().await;
    let user = register(&app, "fresh@example.com").await;

    let (status, body) = http(
        &app,
        Method::GET,
        "/api/me/view-groups?scope=cbl",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["scope"], "cbl");
    assert_eq!(body["groups"].as_array().unwrap().len(), 0);
    assert_eq!(body["ungrouped_ids"].as_array().unwrap().len(), 0);

    let (status, body) = http(
        &app,
        Method::GET,
        "/api/me/view-groups?scope=bogus",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation");

    let (status, _) = http(
        &app,
        Method::GET,
        "/api/me/view-groups?scope=cbl",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_appends_and_lists_members_in_order() {
    let app = TestApp::spawn().await;
    let user = register(&app, "create@example.com").await;
    let a = seed_view(&app, &user, "A").await;
    let b = seed_view(&app, &user, "B").await;

    let g1 = create_group(&app, &user, "filter", "  Marvel  ", &[b, a]).await;
    assert_eq!(g1["name"], "Marvel", "name is trimmed");
    assert_eq!(g1["position"], 0);
    assert_eq!(g1["collapsed"], false);
    assert_eq!(ids(&g1["view_ids"]), vec![b.to_string(), a.to_string()]);

    let g2 = create_group(&app, &user, "filter", "DC", &[]).await;
    assert_eq!(g2["position"], 1);

    let (status, body) = http(
        &app,
        Method::GET,
        "/api/me/view-groups?scope=filter",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let groups = body["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0]["name"], "Marvel");
    assert_eq!(
        ids(&groups[0]["view_ids"]),
        vec![b.to_string(), a.to_string()]
    );
    assert_eq!(groups[1]["name"], "DC");

    // Scoped: the cbl tab sees none of these.
    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/view-groups?scope=cbl",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(body["groups"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn create_validates_name_scope_and_view_ids() {
    let app = TestApp::spawn().await;
    let user = register(&app, "validate@example.com").await;
    let other = register(&app, "other@example.com").await;
    let mine = seed_view(&app, &user, "Mine").await;
    let theirs = seed_view(&app, &other, "Theirs").await;

    for (body, why) in [
        (
            serde_json::json!({ "scope": "cbl", "name": "   " }),
            "blank name",
        ),
        (
            serde_json::json!({ "scope": "cbl", "name": "a".repeat(81) }),
            "long name",
        ),
        (
            serde_json::json!({ "scope": "nope", "name": "x" }),
            "bad scope",
        ),
        (
            serde_json::json!({ "scope": "cbl", "name": "x", "view_ids": [theirs] }),
            "foreign view",
        ),
        (
            serde_json::json!({ "scope": "cbl", "name": "x", "view_ids": [Uuid::now_v7()] }),
            "unknown view",
        ),
        (
            serde_json::json!({ "scope": "cbl", "name": "x", "view_ids": [mine, mine] }),
            "duplicate view",
        ),
    ] {
        let (status, resp) = http(
            &app,
            Method::POST,
            "/api/me/view-groups",
            Some(&user),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{why}: {resp}");
        assert_eq!(resp["error"]["code"], "validation", "{why}");
    }
}

#[tokio::test]
async fn a_view_moves_between_groups_rather_than_duplicating() {
    let app = TestApp::spawn().await;
    let user = register(&app, "move@example.com").await;
    let a = seed_view(&app, &user, "A").await;

    let g1 = create_group(&app, &user, "cbl", "One", &[a]).await;
    let g2 = create_group(&app, &user, "cbl", "Two", &[a]).await;
    assert_eq!(ids(&g2["view_ids"]), vec![a.to_string()]);

    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/view-groups?scope=cbl",
        Some(&user),
        None,
    )
    .await;
    let groups = body["groups"].as_array().unwrap();
    let one = groups.iter().find(|g| g["id"] == g1["id"]).unwrap();
    let two = groups.iter().find(|g| g["id"] == g2["id"]).unwrap();
    assert_eq!(one["view_ids"].as_array().unwrap().len(), 0, "moved out");
    assert_eq!(ids(&two["view_ids"]), vec![a.to_string()]);
}

#[tokio::test]
async fn update_renames_and_collapses() {
    let app = TestApp::spawn().await;
    let user = register(&app, "update@example.com").await;
    let other = register(&app, "update-other@example.com").await;
    let a = seed_view(&app, &user, "A").await;
    let g = create_group(&app, &user, "cbl", "Old", &[a]).await;
    let gid = g["id"].as_str().unwrap();

    let (status, body) = http(
        &app,
        Method::PATCH,
        &format!("/api/me/view-groups/{gid}"),
        Some(&user),
        Some(serde_json::json!({ "name": "New", "collapsed": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "New");
    assert_eq!(body["collapsed"], true);
    assert_eq!(
        ids(&body["view_ids"]),
        vec![a.to_string()],
        "members survive"
    );

    // Blank rename is rejected; other users see 404.
    let (status, _) = http(
        &app,
        Method::PATCH,
        &format!("/api/me/view-groups/{gid}"),
        Some(&user),
        Some(serde_json::json!({ "name": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = http(
        &app,
        Method::PATCH,
        &format!("/api/me/view-groups/{gid}"),
        Some(&other),
        Some(serde_json::json!({ "name": "Hijack" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_ungroups_members_and_is_user_scoped() {
    let app = TestApp::spawn().await;
    let user = register(&app, "delete@example.com").await;
    let other = register(&app, "delete-other@example.com").await;
    let a = seed_view(&app, &user, "A").await;
    let g = create_group(&app, &user, "cbl", "Doomed", &[a]).await;
    let gid = g["id"].as_str().unwrap();

    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/api/me/view-groups/{gid}"),
        Some(&other),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/api/me/view-groups/{gid}"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/view-groups?scope=cbl",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(body["groups"].as_array().unwrap().len(), 0);

    // The view itself is untouched and can be grouped again.
    let g2 = create_group(&app, &user, "cbl", "Again", &[a]).await;
    assert_eq!(ids(&g2["view_ids"]), vec![a.to_string()]);
}

#[tokio::test]
async fn deleting_a_view_drops_its_membership() {
    let app = TestApp::spawn().await;
    let user = register(&app, "cascade@example.com").await;
    let a = seed_view(&app, &user, "A").await;
    let b = seed_view(&app, &user, "B").await;
    let g = create_group(&app, &user, "filter", "G", &[a, b]).await;

    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/api/me/saved-views/{a}"),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/view-groups?scope=filter",
        Some(&user),
        None,
    )
    .await;
    let groups = body["groups"].as_array().unwrap();
    assert_eq!(groups[0]["id"], g["id"]);
    assert_eq!(ids(&groups[0]["view_ids"]), vec![b.to_string()]);
}

#[tokio::test]
async fn replace_layout_reorders_groups_and_moves_members() {
    let app = TestApp::spawn().await;
    let user = register(&app, "layout@example.com").await;
    let a = seed_view(&app, &user, "A").await;
    let b = seed_view(&app, &user, "B").await;
    let c = seed_view(&app, &user, "C").await;
    let d = seed_view(&app, &user, "D").await;
    let g1 = create_group(&app, &user, "cbl", "One", &[a, b]).await;
    let g2 = create_group(&app, &user, "cbl", "Two", &[c]).await;

    // Swap group order, move `a` into Two (first), ungroup `b`, and pin
    // the ungrouped order as d-before-b.
    let (status, body) = http(
        &app,
        Method::PUT,
        "/api/me/view-groups/layout",
        Some(&user),
        Some(serde_json::json!({
            "scope": "cbl",
            "groups": [
                { "id": g2["id"], "view_ids": [a, c] },
                { "id": g1["id"], "view_ids": [] },
            ],
            "ungrouped": [d, b],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/view-groups?scope=cbl",
        Some(&user),
        None,
    )
    .await;
    let groups = body["groups"].as_array().unwrap();
    assert_eq!(groups[0]["id"], g2["id"]);
    assert_eq!(groups[0]["position"], 0);
    assert_eq!(
        ids(&groups[0]["view_ids"]),
        vec![a.to_string(), c.to_string()]
    );
    assert_eq!(groups[1]["id"], g1["id"]);
    assert_eq!(groups[1]["position"], 1);
    assert_eq!(groups[1]["view_ids"].as_array().unwrap().len(), 0);
    assert_eq!(
        ids(&body["ungrouped_ids"]),
        vec![d.to_string(), b.to_string()],
        "ungrouped order is pinned"
    );

    // A later write that pulls `d` into a group drops it from the
    // ungrouped order; `ungrouped` may be omitted entirely.
    let (status, body) = http(
        &app,
        Method::PUT,
        "/api/me/view-groups/layout",
        Some(&user),
        Some(serde_json::json!({
            "scope": "cbl",
            "groups": [
                { "id": g2["id"], "view_ids": [a, c] },
                { "id": g1["id"], "view_ids": [d] },
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/view-groups?scope=cbl",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(body["ungrouped_ids"].as_array().unwrap().len(), 0);
    assert_eq!(ids(&body["groups"][1]["view_ids"]), vec![d.to_string()]);
}

#[tokio::test]
async fn replace_layout_rejects_incomplete_or_invalid_sets() {
    let app = TestApp::spawn().await;
    let user = register(&app, "layout-bad@example.com").await;
    let other = register(&app, "layout-bad-other@example.com").await;
    let a = seed_view(&app, &user, "A").await;
    let theirs = seed_view(&app, &other, "Theirs").await;
    let g1 = create_group(&app, &user, "cbl", "One", &[a]).await;
    let g2 = create_group(&app, &user, "cbl", "Two", &[]).await;
    let foreign = create_group(&app, &other, "cbl", "Foreign", &[]).await;

    let cases = [
        (
            serde_json::json!({ "scope": "cbl", "groups": [{ "id": g1["id"], "view_ids": [] }] }),
            "missing a group",
        ),
        (
            serde_json::json!({ "scope": "cbl", "groups": [
                { "id": g1["id"], "view_ids": [] },
                { "id": g2["id"], "view_ids": [] },
                { "id": foreign["id"], "view_ids": [] },
            ] }),
            "foreign group",
        ),
        (
            serde_json::json!({ "scope": "cbl", "groups": [
                { "id": g1["id"], "view_ids": [] },
                { "id": g1["id"], "view_ids": [] },
            ] }),
            "duplicate group",
        ),
        (
            serde_json::json!({ "scope": "cbl", "groups": [
                { "id": g1["id"], "view_ids": [a] },
                { "id": g2["id"], "view_ids": [a] },
            ] }),
            "view in two groups",
        ),
        (
            serde_json::json!({ "scope": "cbl", "groups": [
                { "id": g1["id"], "view_ids": [a] },
                { "id": g2["id"], "view_ids": [] },
            ], "ungrouped": [a] }),
            "view both grouped and ungrouped",
        ),
        (
            serde_json::json!({ "scope": "cbl", "groups": [
                { "id": g1["id"], "view_ids": [] },
                { "id": g2["id"], "view_ids": [] },
            ], "ungrouped": [theirs] }),
            "foreign view in ungrouped",
        ),
        (
            serde_json::json!({ "scope": "cbl", "groups": [
                { "id": g1["id"], "view_ids": [theirs] },
                { "id": g2["id"], "view_ids": [] },
            ] }),
            "foreign view",
        ),
        (
            serde_json::json!({ "scope": "filter", "groups": [
                { "id": g1["id"], "view_ids": [] },
                { "id": g2["id"], "view_ids": [] },
            ] }),
            "wrong scope for these groups",
        ),
    ];
    for (body, why) in cases {
        let (status, resp) = http(
            &app,
            Method::PUT,
            "/api/me/view-groups/layout",
            Some(&user),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{why}: {resp}");
    }

    // Nothing above was applied: `a` is still in One.
    let (_, body) = http(
        &app,
        Method::GET,
        "/api/me/view-groups?scope=cbl",
        Some(&user),
        None,
    )
    .await;
    let groups = body["groups"].as_array().unwrap();
    assert_eq!(groups[0]["id"], g1["id"]);
    assert_eq!(ids(&groups[0]["view_ids"]), vec![a.to_string()]);
}
