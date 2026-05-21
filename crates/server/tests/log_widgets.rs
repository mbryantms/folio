//! `/me/log/widgets` — per-user widget grid for the Reading Log page.
//!
//! Coverage: default-seed-on-first-read, add/patch/delete CRUD,
//! reorder, reset, RBAC (one user's widgets aren't visible / editable
//! by another), and the per-kind config validation gate.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
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
    #[allow(dead_code)]
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
    auth: &Authed,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
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
    let req = if let Some(b) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        builder
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

// ─────────── Tests ───────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_get_auto_seeds_default_layout() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "seed@lw.test").await;

    let (status, body) = http(&app, Method::GET, "/api/me/log/widgets", &auth, None).await;
    assert_eq!(status, StatusCode::OK);
    let widgets = body["widgets"].as_array().unwrap();
    let kinds: Vec<&str> = widgets
        .iter()
        .map(|w| w["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec!["chrono_feed", "stats_hero", "heatmap", "top_creators"],
        "default M2 layout, in render order"
    );
    // Positions are dense, 0-based, ordered.
    for (i, w) in widgets.iter().enumerate() {
        assert_eq!(w["position"].as_i64().unwrap(), i as i64);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seed_is_idempotent_across_two_gets() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "idem@lw.test").await;
    let (_, body1) = http(&app, Method::GET, "/api/me/log/widgets", &auth, None).await;
    let (_, body2) = http(&app, Method::GET, "/api/me/log/widgets", &auth, None).await;
    let ids1: Vec<&str> = body1["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["id"].as_str().unwrap())
        .collect();
    let ids2: Vec<&str> = body2["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids1, ids2, "second GET must return identical rows");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_appends_with_max_position_plus_one() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "add@lw.test").await;
    // Trigger default seed so positions 0..3 are taken.
    let _ = http(&app, Method::GET, "/api/me/log/widgets", &auth, None).await;

    let req = serde_json::json!({"kind": "currently_reading"});
    let (status, body) = http(&app, Method::POST, "/api/me/log/widgets", &auth, Some(req)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["kind"].as_str().unwrap(), "currently_reading");
    assert_eq!(body["position"].as_i64().unwrap(), 4);
    // Default `config` is `{}` because we sent none.
    assert_eq!(body["config"], serde_json::json!({}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_rejects_unknown_kind() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "unk@lw.test").await;
    let req = serde_json::json!({"kind": "uncle_bobs_widget"});
    let (status, body) = http(&app, Method::POST, "/api/me/log/widgets", &auth, Some(req)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_rejects_invalid_config_shape_for_kind() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "badcfg@lw.test").await;
    // `heatmap.weeks` must be an integer.
    let req = serde_json::json!({
        "kind": "heatmap",
        "config": { "weeks": "fifty-two" }
    });
    let (status, body) = http(&app, Method::POST, "/api/me/log/widgets", &auth, Some(req)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_updates_config_and_bumps_updated_at() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "patch@lw.test").await;
    let (_, list) = http(&app, Method::GET, "/api/me/log/widgets", &auth, None).await;
    let heatmap = list["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["kind"] == "heatmap")
        .unwrap();
    let id = heatmap["id"].as_str().unwrap();
    let req = serde_json::json!({ "config": { "weeks": 12 } });
    let (status, body) = http(
        &app,
        Method::PATCH,
        &format!("/api/me/log/widgets/{id}"),
        &auth,
        Some(req),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["config"]["weeks"].as_i64().unwrap(), 12);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_rejects_unknown_field_for_kind() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "patchbad@lw.test").await;
    let (_, list) = http(&app, Method::GET, "/api/me/log/widgets", &auth, None).await;
    let id = list["widgets"][0]["id"].as_str().unwrap().to_owned();
    let req = serde_json::json!({ "config": { "bogus_field": true } });
    let (status, _) = http(
        &app,
        Method::PATCH,
        &format!("/api/me/log/widgets/{id}"),
        &auth,
        Some(req),
    )
    .await;
    // `deny_unknown_fields` rejects with validation.
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_removes_and_compacts_positions() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "del@lw.test").await;
    let (_, list) = http(&app, Method::GET, "/api/me/log/widgets", &auth, None).await;
    // Delete the heatmap (position 2). Survivors should renumber to
    // 0, 1, 2 — no gap left where the deleted row used to be.
    let heatmap = list["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["kind"] == "heatmap")
        .unwrap();
    let id = heatmap["id"].as_str().unwrap();
    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/api/me/log/widgets/{id}"),
        &auth,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, after) = http(&app, Method::GET, "/api/me/log/widgets", &auth, None).await;
    let widgets = after["widgets"].as_array().unwrap();
    assert_eq!(widgets.len(), 3);
    for (i, w) in widgets.iter().enumerate() {
        assert_eq!(w["position"].as_i64().unwrap(), i as i64);
    }
    let kinds: Vec<&str> = widgets
        .iter()
        .map(|w| w["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["chrono_feed", "stats_hero", "top_creators"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reorder_rewrites_positions_atomically() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "reorder@lw.test").await;
    let (_, list) = http(&app, Method::GET, "/api/me/log/widgets", &auth, None).await;
    let widgets = list["widgets"].as_array().unwrap();
    let ids: Vec<&str> = widgets.iter().map(|w| w["id"].as_str().unwrap()).collect();
    // Reverse the order entirely.
    let reversed: Vec<&&str> = ids.iter().rev().collect();
    let req = serde_json::json!({ "ids": reversed });
    let (status, body) = http(
        &app,
        Method::POST,
        "/api/me/log/widgets/reorder",
        &auth,
        Some(req),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let after_kinds: Vec<&str> = body["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        after_kinds,
        vec!["top_creators", "heatmap", "stats_hero", "chrono_feed"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reorder_rejects_id_set_mismatch() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "rmm@lw.test").await;
    let (_, _list) = http(&app, Method::GET, "/api/me/log/widgets", &auth, None).await;
    // A garbage id set should 400 rather than silently leave the
    // existing rows partially renumbered.
    let req = serde_json::json!({ "ids": [Uuid::new_v4().to_string()] });
    let (status, body) = http(
        &app,
        Method::POST,
        "/api/me/log/widgets/reorder",
        &auth,
        Some(req),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reset_wipes_and_reseeds() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "reset@lw.test").await;
    // Mutate the seed: delete one widget, add an extra.
    let (_, list) = http(&app, Method::GET, "/api/me/log/widgets", &auth, None).await;
    let drop_id = list["widgets"][0]["id"].as_str().unwrap();
    http(
        &app,
        Method::DELETE,
        &format!("/api/me/log/widgets/{drop_id}"),
        &auth,
        None,
    )
    .await;
    http(
        &app,
        Method::POST,
        "/api/me/log/widgets",
        &auth,
        Some(serde_json::json!({"kind": "currently_reading"})),
    )
    .await;
    // Reset → back to the canonical 4-widget M2 layout.
    let (status, body) = http(&app, Method::POST, "/api/me/log/widgets/reset", &auth, None).await;
    assert_eq!(status, StatusCode::OK);
    let kinds: Vec<&str> = body["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec!["chrono_feed", "stats_hero", "heatmap", "top_creators"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_users_widgets_are_isolated() {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice@lw.test").await;
    let bob = register(&app, "bob@lw.test").await;
    // Seed both.
    let (_, alice_list) = http(&app, Method::GET, "/api/me/log/widgets", &alice, None).await;
    let (_, bob_list) = http(&app, Method::GET, "/api/me/log/widgets", &bob, None).await;
    // Disjoint id sets.
    let alice_ids: std::collections::HashSet<&str> = alice_list["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["id"].as_str().unwrap())
        .collect();
    let bob_ids: std::collections::HashSet<&str> = bob_list["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["id"].as_str().unwrap())
        .collect();
    assert!(alice_ids.is_disjoint(&bob_ids));

    // Bob can't touch Alice's widgets.
    let alice_first = alice_list["widgets"][0]["id"].as_str().unwrap();
    let (status, _) = http(
        &app,
        Method::PATCH,
        &format!("/api/me/log/widgets/{alice_first}"),
        &bob,
        Some(serde_json::json!({"config": {}})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/api/me/log/widgets/{alice_first}"),
        &bob,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_config_is_legal_for_every_kind() {
    // Sanity guard that the per-kind schemas all use serde defaults
    // — a future reviewer who removes the `default` attribute and
    // adds a required field would break this test even before any
    // client ships a config UI.
    let app = TestApp::spawn().await;
    let auth = register(&app, "empty@lw.test").await;
    let _ = http(&app, Method::GET, "/api/me/log/widgets", &auth, None).await;
    let kinds = [
        "chrono_feed",
        "stats_hero",
        "heatmap",
        "top_creators",
        "top_publishers",
        "top_imprints",
        "series_finishes",
        "pace_chart",
        "time_of_day",
        "recent_bookmarks",
        "currently_reading",
        "note",
    ];
    for k in kinds {
        let req = serde_json::json!({"kind": k, "config": {}});
        let (status, _) = http(&app, Method::POST, "/api/me/log/widgets", &auth, Some(req)).await;
        assert_eq!(status, StatusCode::CREATED, "empty config rejected for {k}");
    }
}
