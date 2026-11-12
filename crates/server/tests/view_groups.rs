//! Views-index groups — `/me/view-groups` integration coverage.
//!
//! Exercises create / list / rename+collapse / delete / wholesale layout
//! replace, plus the invariants that matter for the drag-and-drop board:
//! a view sits in one group at a time, the layout write must name every
//! group exactly once, groups are isolated per user, and — with nesting —
//! a parent is the caller's own group of the same scope, the tree has no
//! cycles and at most four levels (also under concurrent writes), and
//! deleting a group moves its contents up one level. Create refuses
//! foreign or unknown view ids; the layout write drops them, answering
//! both alike.

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

#[tokio::test]
async fn replace_layout_rejects_lists_over_the_caps_with_field_details() {
    let app = TestApp::spawn().await;
    let user = register(&app, "layout-cap@example.com").await;
    let max = server::api::view_groups::MAX_LAYOUT_VIEW_IDS;
    let many: Vec<Uuid> = (0..=max).map(|_| Uuid::now_v7()).collect();
    let groups: Vec<serde_json::Value> = (0..51)
        .map(|_| serde_json::json!({ "id": Uuid::now_v7(), "view_ids": [] }))
        .collect();
    for (body, field) in [
        (
            serde_json::json!({ "scope": "filter", "groups": [], "ungrouped": many }),
            "ungrouped",
        ),
        (
            serde_json::json!({
                "scope": "filter",
                "groups": [{ "id": Uuid::now_v7(), "view_ids": many }],
            }),
            "groups[0].view_ids",
        ),
        (
            serde_json::json!({ "scope": "filter", "groups": groups }),
            "groups",
        ),
    ] {
        let (status, resp) = http(
            &app,
            Method::PUT,
            "/api/me/view-groups/layout",
            Some(&user),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{resp}");
        assert_eq!(resp["error"]["code"], "validation");
        assert_eq!(resp["error"]["details"][0]["field"], field, "{resp}");
    }
}

// ───── nesting ─────

async fn list_scope(app: &TestApp, user: &Authed, scope: &str) -> serde_json::Value {
    let (status, body) = http(
        app,
        Method::GET,
        &format!("/api/me/view-groups?scope={scope}"),
        Some(user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn create_child(
    app: &TestApp,
    user: &Authed,
    scope: &str,
    name: &str,
    parent: &serde_json::Value,
    view_ids: &[Uuid],
) -> serde_json::Value {
    let (status, body) = http(
        app,
        Method::POST,
        "/api/me/view-groups",
        Some(user),
        Some(serde_json::json!({
            "scope": scope, "name": name, "parent_id": parent["id"], "view_ids": view_ids,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

async fn put_layout(
    app: &TestApp,
    user: &Authed,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    http(
        app,
        Method::PUT,
        "/api/me/view-groups/layout",
        Some(user),
        Some(body),
    )
    .await
}

/// `(parent_id, view_ids)` of every group by id, plus the groups of each
/// parent in sibling (position) order.
struct Tree {
    by_id: std::collections::HashMap<String, (Option<String>, Vec<String>, i64)>,
}

impl Tree {
    fn from(body: &serde_json::Value) -> Self {
        let by_id = body["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                (
                    g["id"].as_str().unwrap().to_owned(),
                    (
                        g["parent_id"].as_str().map(str::to_owned),
                        ids(&g["view_ids"]),
                        g["position"].as_i64().unwrap(),
                    ),
                )
            })
            .collect();
        Self { by_id }
    }
    fn parent(&self, g: &serde_json::Value) -> Option<String> {
        self.by_id[g["id"].as_str().unwrap()].0.clone()
    }
    fn views(&self, g: &serde_json::Value) -> Vec<String> {
        self.by_id[g["id"].as_str().unwrap()].1.clone()
    }
    /// Child ids of `parent` (`None` = top level) in sibling order.
    fn children(&self, parent: Option<&serde_json::Value>) -> Vec<String> {
        let want = parent.map(|p| p["id"].as_str().unwrap().to_owned());
        let mut kids: Vec<(i64, String)> = self
            .by_id
            .iter()
            .filter(|(_, (p, _, _))| *p == want)
            .map(|(id, (_, _, pos))| (*pos, id.clone()))
            .collect();
        kids.sort();
        kids.into_iter().map(|(_, id)| id).collect()
    }
    /// Depth of every group; panics on a cycle.
    fn max_depth(&self) -> usize {
        self.by_id
            .keys()
            .map(|id| {
                let mut depth = 1;
                let mut cur = id.clone();
                while let Some(p) = self.by_id[&cur].0.clone() {
                    depth += 1;
                    assert!(depth <= self.by_id.len(), "cycle through {id}");
                    cur = p;
                }
                depth
            })
            .max()
            .unwrap_or(0)
    }
}

fn sid(g: &serde_json::Value) -> String {
    g["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn create_under_a_parent_and_list_returns_parent_id() {
    let app = TestApp::spawn().await;
    let user = register(&app, "nest-create@example.com").await;
    let a = seed_view(&app, &user, "A").await;
    let top = create_group(&app, &user, "cbl", "Marvel", &[]).await;
    assert!(
        top["parent_id"].is_null(),
        "top-level groups have no parent"
    );
    let child = create_child(&app, &user, "cbl", "Events", &top, &[a]).await;
    assert_eq!(child["parent_id"], top["id"]);
    assert_eq!(child["position"], 0, "first among its siblings");
    let second = create_child(&app, &user, "cbl", "Annuals", &top, &[]).await;
    assert_eq!(second["position"], 1);
    // A new top-level group is numbered among the top-level ones only.
    let other_top = create_group(&app, &user, "cbl", "DC", &[]).await;
    assert_eq!(other_top["position"], 1);

    let tree = Tree::from(&list_scope(&app, &user, "cbl").await);
    assert_eq!(tree.parent(&child), Some(sid(&top)));
    assert_eq!(tree.parent(&top), None);
    assert_eq!(tree.views(&child), vec![a.to_string()]);
    assert_eq!(tree.children(Some(&top)), vec![sid(&child), sid(&second)]);
    assert_eq!(tree.children(None), vec![sid(&top), sid(&other_top)]);
}

#[tokio::test]
async fn create_refuses_foreign_missing_cross_scope_and_too_deep_parents() {
    let app = TestApp::spawn().await;
    let user = register(&app, "nest-refuse@example.com").await;
    let other = register(&app, "nest-refuse-other@example.com").await;
    let foreign = create_group(&app, &other, "cbl", "Theirs", &[]).await;
    let filter_group = create_group(&app, &user, "filter", "Filters", &[]).await;
    let l1 = create_group(&app, &user, "cbl", "L1", &[]).await;
    let l2 = create_child(&app, &user, "cbl", "L2", &l1, &[]).await;
    let l3 = create_child(&app, &user, "cbl", "L3", &l2, &[]).await;
    let l4 = create_child(&app, &user, "cbl", "L4", &l3, &[]).await;

    let mut not_found_answers = Vec::new();
    for (parent, why) in [
        (foreign["id"].clone(), "another user's group"),
        (serde_json::json!(Uuid::now_v7()), "unknown group"),
        (filter_group["id"].clone(), "a group of another scope"),
        (l4["id"].clone(), "a fifth level"),
    ] {
        let (status, resp) = http(
            &app,
            Method::POST,
            "/api/me/view-groups",
            Some(&user),
            Some(serde_json::json!({ "scope": "cbl", "name": "x", "parent_id": parent })),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{why}: {resp}");
        assert_eq!(resp["error"]["code"], "validation", "{why}");
        assert_eq!(resp["error"]["details"][0]["field"], "parent_id", "{why}");
        if why != "a fifth level" {
            not_found_answers.push(resp);
        }
    }
    // Another user's group is indistinguishable from a missing one.
    assert_eq!(not_found_answers[0], not_found_answers[1]);
    assert_eq!(not_found_answers[1], not_found_answers[2]);

    // Nothing was created.
    let tree = Tree::from(&list_scope(&app, &user, "cbl").await);
    assert_eq!(tree.by_id.len(), 4);
    assert_eq!(tree.max_depth(), 4);
}

#[tokio::test]
async fn layout_round_trips_a_three_level_tree() {
    let app = TestApp::spawn().await;
    let user = register(&app, "nest-layout@example.com").await;
    let a = seed_view(&app, &user, "A").await;
    let b = seed_view(&app, &user, "B").await;
    let c = seed_view(&app, &user, "C").await;
    let g1 = create_group(&app, &user, "cbl", "One", &[]).await;
    let g2 = create_group(&app, &user, "cbl", "Two", &[]).await;
    let g3 = create_group(&app, &user, "cbl", "Three", &[]).await;
    let g4 = create_group(&app, &user, "cbl", "Four", &[]).await;

    // Two ⊃ One ⊃ Three, Four next to One inside Two (listed before it).
    let (status, resp) = put_layout(
        &app,
        &user,
        serde_json::json!({ "scope": "cbl", "groups": [
            { "id": g2["id"], "view_ids": [a] },
            { "id": g4["id"], "parent_id": g2["id"], "view_ids": [] },
            { "id": g3["id"], "parent_id": g1["id"], "view_ids": [c, b] },
            { "id": g1["id"], "parent_id": g2["id"], "view_ids": [] },
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{resp}");

    let body = list_scope(&app, &user, "cbl").await;
    let tree = Tree::from(&body);
    assert_eq!(tree.children(None), vec![sid(&g2)]);
    assert_eq!(tree.children(Some(&g2)), vec![sid(&g4), sid(&g1)]);
    assert_eq!(tree.children(Some(&g1)), vec![sid(&g3)]);
    assert_eq!(tree.views(&g3), vec![c.to_string(), b.to_string()]);
    assert_eq!(tree.views(&g2), vec![a.to_string()]);
    assert_eq!(tree.max_depth(), 3);

    // Sending back what the list says reproduces it exactly.
    let groups: Vec<serde_json::Value> = body["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| serde_json::json!({ "id": g["id"], "parent_id": g["parent_id"], "view_ids": g["view_ids"] }))
        .collect();
    let (status, resp) = put_layout(
        &app,
        &user,
        serde_json::json!({ "scope": "cbl", "groups": groups }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{resp}");
    let again = Tree::from(&list_scope(&app, &user, "cbl").await);
    assert_eq!(again.by_id, tree.by_id);

    // Moving a folder to the top level is a layout write too; a layout
    // without `parent_id` (an old client) flattens nothing it doesn't
    // name: absent = top level.
    let (status, _) = put_layout(
        &app,
        &user,
        serde_json::json!({ "scope": "cbl", "groups": [
            { "id": g2["id"], "view_ids": [a] },
            { "id": g4["id"], "parent_id": g2["id"], "view_ids": [] },
            { "id": g1["id"], "view_ids": [] },
            { "id": g3["id"], "parent_id": g1["id"], "view_ids": [c, b] },
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let moved = Tree::from(&list_scope(&app, &user, "cbl").await);
    assert_eq!(moved.children(None), vec![sid(&g2), sid(&g1)]);
    assert_eq!(moved.children(Some(&g2)), vec![sid(&g4)]);
    assert_eq!(moved.children(Some(&g1)), vec![sid(&g3)]);
}

#[tokio::test]
async fn layout_refuses_cycles_depth_five_and_unlisted_parents() {
    let app = TestApp::spawn().await;
    let user = register(&app, "nest-layout-bad@example.com").await;
    let other = register(&app, "nest-layout-bad-other@example.com").await;
    let g: Vec<serde_json::Value> = {
        let mut v = Vec::new();
        for n in ["A", "B", "C", "D", "E"] {
            v.push(create_group(&app, &user, "cbl", n, &[]).await);
        }
        v
    };
    let foreign = create_group(&app, &other, "cbl", "Theirs", &[]).await;
    let filter_group = create_group(&app, &user, "filter", "F", &[]).await;
    let flat = |overrides: &[(usize, serde_json::Value)]| {
        let groups: Vec<serde_json::Value> = g
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let parent = overrides
                    .iter()
                    .find(|(j, _)| *j == i)
                    .map(|(_, p)| p.clone())
                    .unwrap_or(serde_json::Value::Null);
                serde_json::json!({ "id": x["id"], "parent_id": parent, "view_ids": [] })
            })
            .collect();
        serde_json::json!({ "scope": "cbl", "groups": groups })
    };
    let cases = [
        (
            flat(&[(0, g[1]["id"].clone()), (1, g[0]["id"].clone())]),
            "two-group cycle",
            "groups[0].parent_id",
        ),
        (
            flat(&[
                (0, g[2]["id"].clone()),
                (1, g[0]["id"].clone()),
                (2, g[1]["id"].clone()),
            ]),
            "three-group cycle",
            "groups[0].parent_id",
        ),
        (
            flat(&[(3, g[3]["id"].clone())]),
            "own parent",
            "groups[3].parent_id",
        ),
        (
            flat(&[
                (1, g[0]["id"].clone()),
                (2, g[1]["id"].clone()),
                (3, g[2]["id"].clone()),
                (4, g[3]["id"].clone()),
            ]),
            "depth 5",
            "groups[4].parent_id",
        ),
        (
            flat(&[(2, serde_json::json!(Uuid::now_v7()))]),
            "parent missing from the payload",
            "groups[2].parent_id",
        ),
        (
            flat(&[(2, foreign["id"].clone())]),
            "another user's group as parent",
            "groups[2].parent_id",
        ),
        (
            flat(&[(2, filter_group["id"].clone())]),
            "another scope's group as parent",
            "groups[2].parent_id",
        ),
    ];
    for (body, why, field) in cases {
        let (status, resp) = put_layout(&app, &user, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{why}: {resp}");
        assert_eq!(resp["error"]["code"], "validation", "{why}");
        assert_eq!(resp["error"]["details"][0]["field"], field, "{why}: {resp}");
    }
    // Nothing above was applied: still five top-level groups.
    let tree = Tree::from(&list_scope(&app, &user, "cbl").await);
    assert_eq!(tree.children(None).len(), 5);
    assert_eq!(tree.max_depth(), 1);
}

#[tokio::test]
async fn delete_moves_lists_and_child_folders_up_one_level_in_place() {
    let app = TestApp::spawn().await;
    let user = register(&app, "nest-delete@example.com").await;
    let x = seed_view(&app, &user, "X").await;
    let a = seed_view(&app, &user, "A").await;
    let b = seed_view(&app, &user, "B").await;
    let c = seed_view(&app, &user, "C").await;
    let u = seed_view(&app, &user, "U").await;
    let t = seed_view(&app, &user, "T").await;

    // Root ⊃ [S1, Doomed ⊃ [C1 ⊃ [Deep], C2] (a, b), S2] (x)
    let root = create_group(&app, &user, "cbl", "Root", &[x]).await;
    let s1 = create_child(&app, &user, "cbl", "S1", &root, &[]).await;
    let doomed = create_child(&app, &user, "cbl", "Doomed", &root, &[a, b]).await;
    let s2 = create_child(&app, &user, "cbl", "S2", &root, &[]).await;
    let c1 = create_child(&app, &user, "cbl", "C1", &doomed, &[]).await;
    let c2 = create_child(&app, &user, "cbl", "C2", &doomed, &[]).await;
    let deep = create_child(&app, &user, "cbl", "Deep", &c1, &[c]).await;
    let top = create_group(&app, &user, "cbl", "TopLevel", &[t]).await;
    let (status, _) = put_layout(
        &app,
        &user,
        serde_json::json!({ "scope": "cbl", "groups": [
            { "id": root["id"], "view_ids": [x] },
            { "id": s1["id"], "parent_id": root["id"], "view_ids": [] },
            { "id": doomed["id"], "parent_id": root["id"], "view_ids": [a, b] },
            { "id": s2["id"], "parent_id": root["id"], "view_ids": [] },
            { "id": c1["id"], "parent_id": doomed["id"], "view_ids": [] },
            { "id": c2["id"], "parent_id": doomed["id"], "view_ids": [] },
            { "id": deep["id"], "parent_id": c1["id"], "view_ids": [c] },
            { "id": top["id"], "view_ids": [t] },
        ], "ungrouped": [u] }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/api/me/view-groups/{}", sid(&doomed)),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let body = list_scope(&app, &user, "cbl").await;
    let tree = Tree::from(&body);
    assert!(!tree.by_id.contains_key(&sid(&doomed)));
    // Child folders take its place, in their order; lists follow the
    // parent's own.
    assert_eq!(
        tree.children(Some(&root)),
        vec![sid(&s1), sid(&c1), sid(&c2), sid(&s2)]
    );
    assert_eq!(
        tree.views(&root),
        vec![x.to_string(), a.to_string(), b.to_string()]
    );
    // The subtree below a child comes along untouched.
    assert_eq!(tree.children(Some(&c1)), vec![sid(&deep)]);
    assert_eq!(tree.views(&deep), vec![c.to_string()]);

    // A top-level folder: its lists join the end of the ungrouped order.
    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/api/me/view-groups/{}", sid(&top)),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let body = list_scope(&app, &user, "cbl").await;
    assert_eq!(
        ids(&body["ungrouped_ids"]),
        vec![u.to_string(), t.to_string()]
    );
    let tree = Tree::from(&body);
    assert_eq!(tree.children(None), vec![sid(&root)]);

    // Deleting the root lifts its children to the top level.
    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/api/me/view-groups/{}", sid(&root)),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let body = list_scope(&app, &user, "cbl").await;
    let tree = Tree::from(&body);
    assert_eq!(
        tree.children(None),
        vec![sid(&s1), sid(&c1), sid(&c2), sid(&s2)]
    );
    assert_eq!(
        ids(&body["ungrouped_ids"]),
        vec![
            u.to_string(),
            t.to_string(),
            x.to_string(),
            a.to_string(),
            b.to_string()
        ]
    );
}

#[tokio::test]
async fn layout_drops_unknown_and_foreign_view_ids_alike() {
    let app = TestApp::spawn().await;
    let user = register(&app, "layout-drop@example.com").await;
    let other = register(&app, "layout-drop-other@example.com").await;
    let a = seed_view(&app, &user, "A").await;
    let b = seed_view(&app, &user, "B").await;
    let theirs = seed_view(&app, &other, "Theirs").await;
    let unknown = Uuid::now_v7();
    let g = create_group(&app, &user, "cbl", "G", &[]).await;

    let send = |ghost: Uuid| {
        serde_json::json!({ "scope": "cbl", "groups": [
            { "id": g["id"], "view_ids": [a, ghost] },
        ], "ungrouped": [ghost, b] })
    };
    // Same id in two places is still a duplicate, whatever it is.
    let (status, _) = put_layout(&app, &user, send(unknown)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let one = |ghost: Uuid| {
        serde_json::json!({ "scope": "cbl", "groups": [
            { "id": g["id"], "view_ids": [ghost, a] },
        ], "ungrouped": [b] })
    };
    let (s_unknown, r_unknown) = put_layout(&app, &user, one(unknown)).await;
    let after_unknown = list_scope(&app, &user, "cbl").await;
    let (s_theirs, r_theirs) = put_layout(&app, &user, one(theirs)).await;
    let after_theirs = list_scope(&app, &user, "cbl").await;
    assert_eq!(s_unknown, StatusCode::NO_CONTENT, "{r_unknown}");
    assert_eq!((s_unknown, &r_unknown), (s_theirs, &r_theirs));
    for body in [&after_unknown, &after_theirs] {
        assert_eq!(ids(&body["groups"][0]["view_ids"]), vec![a.to_string()]);
        assert_eq!(ids(&body["ungrouped_ids"]), vec![b.to_string()]);
    }
    // And the other user's arrangement is untouched by any of it.
    let theirs_body = list_scope(&app, &other, "cbl").await;
    assert_eq!(theirs_body["groups"].as_array().unwrap().len(), 0);
    assert_eq!(theirs_body["ungrouped_ids"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn concurrent_layouts_cannot_build_a_cycle() {
    let app = TestApp::spawn().await;
    let user = register(&app, "nest-race@example.com").await;
    let x = create_group(&app, &user, "cbl", "X", &[]).await;
    let y = create_group(&app, &user, "cbl", "Y", &[]).await;
    for _ in 0..15 {
        // Each payload is a valid tree on its own; listed in opposite
        // orders so their row writes interleave.
        let x_in_y = serde_json::json!({ "scope": "cbl", "groups": [
            { "id": x["id"], "parent_id": y["id"], "view_ids": [] },
            { "id": y["id"], "view_ids": [] },
        ] });
        let y_in_x = serde_json::json!({ "scope": "cbl", "groups": [
            { "id": y["id"], "parent_id": x["id"], "view_ids": [] },
            { "id": x["id"], "view_ids": [] },
        ] });
        let (r1, r2) = tokio::join!(
            put_layout(&app, &user, x_in_y),
            put_layout(&app, &user, y_in_x)
        );
        assert_eq!(r1.0, StatusCode::NO_CONTENT, "{}", r1.1);
        assert_eq!(r2.0, StatusCode::NO_CONTENT, "{}", r2.1);
        let tree = Tree::from(&list_scope(&app, &user, "cbl").await);
        assert_eq!(tree.max_depth(), 2, "one inside the other, no cycle");
    }
}

#[tokio::test]
async fn concurrent_create_and_layout_cannot_exceed_four_levels() {
    let app = TestApp::spawn().await;
    let user = register(&app, "nest-race-depth@example.com").await;
    for round in 0..8 {
        let scope = if round % 2 == 0 { "cbl" } else { "filter" };
        let base = format!("r{round}");
        let l1 = create_group(&app, &user, scope, &format!("{base}-1"), &[]).await;
        let l2 = create_child(&app, &user, scope, &format!("{base}-2"), &l1, &[]).await;
        let l3 = create_child(&app, &user, scope, &format!("{base}-3"), &l2, &[]).await;
        let host = create_group(&app, &user, scope, &format!("{base}-h"), &[]).await;
        // Every group of the scope, as the list has them, with l1 moved
        // under `host`: valid now (l3 becomes level 4).
        let current = list_scope(&app, &user, scope).await;
        let groups: Vec<serde_json::Value> = current["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                let parent = if g["id"] == l1["id"] {
                    host["id"].clone()
                } else {
                    g["parent_id"].clone()
                };
                serde_json::json!({ "id": g["id"], "parent_id": parent, "view_ids": g["view_ids"] })
            })
            .collect();
        let create = http(
            &app,
            Method::POST,
            "/api/me/view-groups",
            Some(&user),
            Some(serde_json::json!({ "scope": scope, "name": "new", "parent_id": l3["id"] })),
        );
        let layout = put_layout(
            &app,
            &user,
            serde_json::json!({ "scope": scope, "groups": groups }),
        );
        let (c, l) = tokio::join!(create, layout);
        // Exactly one may win: whichever ran second sees the other's
        // tree and refuses (create: too deep; layout: the new group is
        // missing from its payload).
        let wins = [c.0 == StatusCode::CREATED, l.0 == StatusCode::NO_CONTENT];
        assert_eq!(wins.iter().filter(|w| **w).count(), 1, "{} / {}", c.1, l.1);
        let tree = Tree::from(&list_scope(&app, &user, scope).await);
        assert!(tree.max_depth() <= 4, "round {round}");
    }
}

#[tokio::test]
async fn deleting_the_user_removes_a_nested_tree() {
    let app = TestApp::spawn().await;
    let user = register(&app, "nest-purge@example.com").await;
    let keep = register(&app, "nest-purge-keep@example.com").await;
    let l1 = create_group(&app, &user, "cbl", "L1", &[]).await;
    let l2 = create_child(&app, &user, "cbl", "L2", &l1, &[]).await;
    let _l3 = create_child(&app, &user, "cbl", "L3", &l2, &[]).await;
    let kept = create_group(&app, &keep, "cbl", "Kept", &[]).await;

    let db = Database::connect(&app.db_url).await.unwrap();
    use sea_orm::{ConnectionTrait, Statement};
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        "DELETE FROM users WHERE id = $1",
        [user.user_id.into()],
    ))
    .await
    .expect("the foreign key never blocks a user delete");
    let left: i64 = db
        .query_one_raw(Statement::from_sql_and_values(
            db.get_database_backend(),
            "SELECT count(*) AS n FROM saved_view_groups WHERE user_id = $1",
            [user.user_id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap();
    assert_eq!(left, 0);
    let body = list_scope(&app, &keep, "cbl").await;
    assert_eq!(body["groups"][0]["id"], kept["id"]);
}
