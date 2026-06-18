//! `GET /series?sort=random` — the "Surprise me" discovery pick (audit 3.7).
//!
//! Random order has no stable cursor, so the endpoint returns up to `limit`
//! rows in `ORDER BY random()` order and never hands back a `next_cursor`.
//! Surprise-me calls it with `limit=1`.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::{TestApp, seed};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
}

impl Authed {
    fn cookie(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
}

async fn register_admin(app: &TestApp) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"rand-admin@example.com","password":"correctly-horse-battery"}"#,
                ))
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
    let pick = |p: &str| {
        cookies
            .iter()
            .find(|c| c.starts_with(p))
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap()
                    .trim_start_matches(p)
                    .to_owned()
            })
            .expect(p)
    };
    Authed {
        session: pick("__Host-comic_session="),
        csrf: pick("__Host-comic_csrf="),
    }
}

async fn random_list(app: &TestApp, auth: &Authed, lib: Uuid, limit: u64) -> Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/api/series?library={lib}&sort=random&limit={limit}"
                ))
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn random_sort_returns_one_with_no_cursor() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let db = &app.state().db;
    let dir = tempfile::tempdir().unwrap();
    let lib = seed::seed_library(db, dir.path()).await;
    for name in ["Alpha", "Bravo", "Charlie", "Delta", "Echo"] {
        seed::seed_series(db, lib, name).await;
    }

    // limit=1 (surprise-me): exactly one row, never a pagination cursor.
    let one = random_list(&app, &auth, lib, 1).await;
    assert_eq!(one["items"].as_array().unwrap().len(), 1, "one random row");
    assert!(
        one["next_cursor"].is_null(),
        "random order is never paginated"
    );

    // A wider limit returns every accessible row (still no cursor), so a
    // small library can't get stuck behind a phantom page boundary.
    let all = random_list(&app, &auth, lib, 100).await;
    assert_eq!(all["items"].as_array().unwrap().len(), 5, "all 5 rows");
    assert!(all["next_cursor"].is_null());
}
