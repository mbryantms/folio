//! "Mark metadata complete" escape hatch (metadata-at-scale B4).
//!
//! A thin / unmatched issue sits in the `needs_metadata` worklist forever
//! because completeness is computed from field presence. The accept endpoint
//! records a reversible operator acknowledgement so the completeness tier
//! reports `accepted` instead — without faking field presence (the detail
//! view still lists the real gaps).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::{TestApp, seed};
use sea_orm::EntityTrait;
use serde_json::Value;
use tower::ServiceExt;

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
                    r#"{"email":"accept-admin@example.com","password":"correctly-horse-battery"}"#,
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
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn body_json(b: Body) -> Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn get_issue(app: &TestApp, auth: &Authed, s_slug: &str, i_slug: &str) -> Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/series/{s_slug}/issues/{i_slug}"))
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp.into_body()).await
}

async fn set_accept(
    app: &TestApp,
    auth: &Authed,
    s_slug: &str,
    i_slug: &str,
    method: Method,
) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(format!(
                    "/api/series/{s_slug}/issues/{i_slug}/metadata/accept"
                ))
                .header(header::COOKIE, auth.cookie())
                .header("x-csrf-token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn accept_marks_issue_complete_then_reverts() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let st = app.state();
    let db = &st.db;

    let dir = tempfile::tempdir().unwrap();
    let lib = seed::seed_library(db, dir.path()).await;
    let series_id = seed::seed_series(db, lib, "Avengers").await;
    // A bare issue: no provider match, no summary/credits/year → NeedsMetadata.
    let issue_id = seed::seed_issue(db, lib, series_id, &dir.path().join("a.cbz"), b"x", 1.0).await;

    let series = entity::series::Entity::find_by_id(series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let issue = entity::issue::Entity::find_by_id(issue_id.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let (s_slug, i_slug) = (series.slug, issue.slug);

    // Intrinsic state: needs metadata, not accepted.
    let v = get_issue(&app, &auth, &s_slug, &i_slug).await;
    assert_eq!(v["metadata_completeness"]["tier"], "needs_metadata");
    assert!(v["metadata_review_accepted_at"].is_null());
    let gaps_before = v["metadata_completeness"]["missing_core"]
        .as_array()
        .unwrap()
        .len();
    assert!(gaps_before > 0, "a bare issue should have real gaps");

    // Accept → tier flips to `accepted`, timestamp set, gaps UNCHANGED (honest).
    let resp = set_accept(&app, &auth, &s_slug, &i_slug, Method::POST).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert!(body["metadata_review_accepted_at"].is_string());

    let v = get_issue(&app, &auth, &s_slug, &i_slug).await;
    assert_eq!(v["metadata_completeness"]["tier"], "accepted");
    assert!(v["metadata_review_accepted_at"].is_string());
    assert_eq!(
        v["metadata_completeness"]["missing_core"]
            .as_array()
            .unwrap()
            .len(),
        gaps_before,
        "accepting must NOT fake field presence — the gaps still stand"
    );

    // Un-accept → reverts to the intrinsic tier.
    let resp = set_accept(&app, &auth, &s_slug, &i_slug, Method::DELETE).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v = get_issue(&app, &auth, &s_slug, &i_slug).await;
    assert_eq!(v["metadata_completeness"]["tier"], "needs_metadata");
    assert!(v["metadata_review_accepted_at"].is_null());
}

#[tokio::test]
async fn accept_requires_library_access() {
    let app = TestApp::spawn().await;
    let _admin = register_admin(&app).await;

    // Second user — a non-admin with no library grants.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"outsider@example.com","password":"correctly-horse-battery"}"#,
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
            .unwrap()
    };
    let outsider = Authed {
        session: pick("__Host-comic_session="),
        csrf: pick("__Host-comic_csrf="),
    };

    let st = app.state();
    let db = &st.db;
    let dir = tempfile::tempdir().unwrap();
    let lib = seed::seed_library(db, dir.path()).await;
    let series_id = seed::seed_series(db, lib, "Hidden").await;
    let issue_id = seed::seed_issue(db, lib, series_id, &dir.path().join("h.cbz"), b"x", 1.0).await;
    let series = entity::series::Entity::find_by_id(series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let issue = entity::issue::Entity::find_by_id(issue_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();

    let resp = set_accept(&app, &outsider, &series.slug, &issue.slug, Method::POST).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
