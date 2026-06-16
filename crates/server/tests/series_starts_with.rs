//! `GET /series?starts_with=<bucket>` — the A–Z jump-rail filter (audit B9).
//!
//! The rail maps each letter (and `#`) to a server-side `starts_with`
//! filter on `normalized_name` — the same column the Name sort orders by —
//! so clicking "S" lands on the rows that actually sort under S. Articles
//! are NOT stripped by `normalize_name`, so "The Boys" sorts (and filters)
//! under T.

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
                    r#"{"email":"az-admin@example.com","password":"correctly-horse-battery"}"#,
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

/// Names returned by `/series?starts_with=<bucket>`, sorted for stable asserts.
async fn names_for(app: &TestApp, auth: &Authed, lib: Uuid, bucket: &str) -> Vec<String> {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/api/series?library={lib}&starts_with={bucket}&limit=100"
                ))
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "bucket={bucket}");
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let mut names: Vec<String> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn starts_with_buckets_by_normalized_name() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let st = app.state();
    let db = &st.db;
    let dir = tempfile::tempdir().unwrap();
    let lib = seed::seed_library(db, dir.path()).await;

    seed::seed_series(db, lib, "Avengers").await;
    seed::seed_series(db, lib, "Aquaman").await;
    seed::seed_series(db, lib, "Batman").await;
    // Article is NOT stripped → normalizes to "the boys" → sorts under T.
    seed::seed_series(db, lib, "The Boys").await;
    // Leading digit → the "#" bucket.
    seed::seed_series(db, lib, "2020 Force Works").await;

    // Letter bucket, case-insensitive.
    assert_eq!(
        names_for(&app, &auth, lib, "a").await,
        vec!["Aquaman", "Avengers"],
        "a → both A series"
    );
    assert_eq!(
        names_for(&app, &auth, lib, "B").await,
        vec!["Batman"],
        "uppercase B matches"
    );
    // Article kept → T, not B.
    assert_eq!(
        names_for(&app, &auth, lib, "t").await,
        vec!["The Boys"],
        "t → The Boys (article not stripped)"
    );
    assert!(
        names_for(&app, &auth, lib, "b").await == vec!["Batman"],
        "b is Batman only, not The Boys"
    );
    // "#" → the digit-leading name.
    assert_eq!(
        names_for(&app, &auth, lib, "%23").await, // URL-encoded '#'
        vec!["2020 Force Works"],
        "# → digit-leading names"
    );
    // A letter with no matches is an empty (but valid) page.
    assert!(
        names_for(&app, &auth, lib, "z").await.is_empty(),
        "z → empty"
    );
}

#[tokio::test]
async fn invalid_starts_with_is_422() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    for bad in ["ab", "1", "$"] {
        let resp = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/series?starts_with={bad}"))
                    .header(header::COOKIE, auth.cookie())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "bad starts_with={bad:?}"
        );
    }
}
