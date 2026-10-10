//! `GET /series?provider_match=matched|unmatched` — the facet the admin
//! metadata dashboard's "Unmatched" tile links to. A series is matched
//! when it has a series-level ComicVine / Metron / GCD id in
//! `external_ids`, the same rule the dashboard counts with.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::{TestApp, seed};
use sea_orm::{ActiveModelTrait, Set};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

async fn register(app: &TestApp) -> String {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"email": "admin@example.com", "password": "correctly-horse-battery"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|c| c.split(';').next())
        .filter(|c| c.starts_with("__Host-comic_session=") || c.starts_with("__Host-comic_csrf="))
        .collect::<Vec<_>>()
        .join("; ")
}

async fn list(app: &TestApp, cookie: &str, lib: Uuid, facet: &str) -> (StatusCode, Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/api/series?library={lib}&provider_match={facet}&limit=100"
                ))
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn names(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_owned())
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn provider_match_splits_series_by_series_level_provider_id() {
    let app = TestApp::spawn().await;
    let cookie = register(&app).await;
    let db = &app.state().db;
    let dir = tempfile::tempdir().unwrap();
    let lib = seed::seed_library(db, dir.path()).await;

    let matched = seed::seed_series(db, lib, "Linked Run").await;
    let _unmatched = seed::seed_series(db, lib, "Lonely Run").await;
    // An issue-level id or a non-provider source doesn't count.
    let decoy = seed::seed_series(db, lib, "Decoy Run").await;
    for (entity_type, entity_id, source) in [
        ("series", matched.to_string(), "comicvine"),
        ("issue", "some-issue-hash".to_owned(), "comicvine"),
        ("series", decoy.to_string(), "gtin"),
    ] {
        let now = chrono::Utc::now().fixed_offset();
        entity::external_id::ActiveModel {
            entity_type: Set(entity_type.into()),
            entity_id: Set(entity_id),
            source: Set(source.into()),
            external_id: Set("1".into()),
            external_url: Set(None),
            set_by: Set("user".into()),
            first_set_at: Set(now),
            last_synced_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();
    }

    let (st, body) = list(&app, &cookie, lib, "matched").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(names(&body), vec!["Linked Run"]);

    let (st, body) = list(&app, &cookie, lib, "unmatched").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(names(&body), vec!["Decoy Run", "Lonely Run"]);

    let (st, body) = list(&app, &cookie, lib, "sometimes").await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
}
