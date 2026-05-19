//! Integration tests for `crates/server/src/api/ratings.rs`.
//!
//! The audit identified ratings as one of the few user-mutation
//! surfaces lacking a dedicated test file — coverage existed only
//! incidentally via `issues_edit.rs`. This file owns the contract:
//! set / clear / half-step / range validation for both series and
//! issue rating endpoints.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use common::seed::{seed_issue, seed_library, seed_series};
use sea_orm::Database;
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

impl Authed {
    fn cookies(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
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

async fn body_bytes(b: Body) -> Vec<u8> {
    to_bytes(b, usize::MAX).await.unwrap().to_vec()
}

async fn body_json(b: Body) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(b).await).unwrap()
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
    let json: serde_json::Value = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session,
        csrf,
        user_id,
    }
}

async fn put_rating(
    app: &TestApp,
    auth: &Authed,
    uri: &str,
    body: serde_json::Value,
) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(uri)
                .header(header::COOKIE, auth.cookies())
                .header("X-CSRF-Token", &auth.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn fetch_user_rating(app: &TestApp, user_id: Uuid, target_id: &str) -> Option<f64> {
    use entity::user_rating;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let db = Database::connect(&app.db_url).await.unwrap();
    user_rating::Entity::find()
        .filter(user_rating::Column::UserId.eq(user_id))
        .filter(user_rating::Column::TargetId.eq(target_id))
        .one(&db)
        .await
        .unwrap()
        .map(|m| m.rating)
}

async fn seed_user_visible_issue(
    app: &TestApp,
    tmp: &std::path::Path,
) -> (
    entity::library::Model,
    entity::series::Model,
    String,
    Authed,
) {
    use entity::{library, library_user_access, series};
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};

    let auth = register(app, "rater@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let lib_id = seed_library(&db, tmp).await;
    let series_id = seed_series(&db, lib_id, "Reviewable").await;
    let issue_id = seed_issue(&db, lib_id, series_id, &tmp.join("a.cbz"), b"rate-a", 1.0).await;

    // Grant read access (libraries default to admin-only without an
    // explicit grant; tests/markers.rs has the same shape).
    let lib = library::Entity::find_by_id(lib_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let series_row = series::Entity::find_by_id(series_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let now = chrono::Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(auth.user_id),
        library_id: Set(lib_id),
        role: Set("reader".into()),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();

    (lib, series_row, issue_id, auth)
}

// ────────────── series rating ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_rating_can_be_set_and_cleared() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (_, series, _, auth) = seed_user_visible_issue(&app, tmp.path()).await;

    // Set 4.5 → 200 OK, body contains rating.
    let uri = format!("/api/series/{}/rating", series.slug);
    let resp = put_rating(&app, &auth, &uri, serde_json::json!({"rating": 4.5})).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["rating"].as_f64(), Some(4.5));
    assert_eq!(
        fetch_user_rating(&app, auth.user_id, &series.id.to_string()).await,
        Some(4.5),
    );

    // Update to 3.0 — same path, no separate "edit" verb.
    let resp = put_rating(&app, &auth, &uri, serde_json::json!({"rating": 3.0})).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        fetch_user_rating(&app, auth.user_id, &series.id.to_string()).await,
        Some(3.0),
    );

    // Clear with null — DB row should be removed (or write returns
    // None). The contract: a follow-up read sees no rating.
    let resp = put_rating(&app, &auth, &uri, serde_json::json!({"rating": null})).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert!(
        body["rating"].is_null(),
        "cleared rating should serialise as null, got: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_rating_rejects_out_of_range() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (_, series, _, auth) = seed_user_visible_issue(&app, tmp.path()).await;

    let uri = format!("/api/series/{}/rating", series.slug);

    // Below floor.
    let resp = put_rating(&app, &auth, &uri, serde_json::json!({"rating": -0.5})).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "validation.rating");

    // Above ceiling.
    let resp = put_rating(&app, &auth, &uri, serde_json::json!({"rating": 5.5})).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "validation.rating");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_rating_rejects_non_half_step() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (_, series, _, auth) = seed_user_visible_issue(&app, tmp.path()).await;

    let uri = format!("/api/series/{}/rating", series.slug);

    // Quarter-step — should bounce.
    let resp = put_rating(&app, &auth, &uri, serde_json::json!({"rating": 3.25})).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "validation.rating");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_rating_404_when_slug_unknown() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "404@example.com").await;
    let resp = put_rating(
        &app,
        &auth,
        "/api/series/does-not-exist/rating",
        serde_json::json!({"rating": 3.0}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ────────────── issue rating ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_rating_round_trip() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (_, series, issue_id, auth) = seed_user_visible_issue(&app, tmp.path()).await;

    // Issue rating endpoint takes BOTH series_slug + issue_slug.
    let db = Database::connect(&app.db_url).await.unwrap();
    use sea_orm::EntityTrait;
    let issue_row = entity::issue::Entity::find_by_id(issue_id.clone())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let uri = format!(
        "/api/series/{}/issues/{}/rating",
        series.slug, issue_row.slug
    );

    let resp = put_rating(&app, &auth, &uri, serde_json::json!({"rating": 5.0})).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["rating"].as_f64(), Some(5.0));
    assert_eq!(
        fetch_user_rating(&app, auth.user_id, &issue_id).await,
        Some(5.0),
    );

    // Clear → row removed.
    let resp = put_rating(&app, &auth, &uri, serde_json::json!({"rating": null})).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(fetch_user_rating(&app, auth.user_id, &issue_id).await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_rating_404_on_cross_user_invisible_library() {
    // Issue ratings should 404 (not 403) when the caller can't see
    // the underlying library — guards the existence-leak surface.
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (_, series, issue_id, _owner) = seed_user_visible_issue(&app, tmp.path()).await;

    let other = register(&app, "other@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    use sea_orm::EntityTrait;
    let issue_row = entity::issue::Entity::find_by_id(issue_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let uri = format!(
        "/api/series/{}/issues/{}/rating",
        series.slug, issue_row.slug
    );
    let resp = put_rating(&app, &other, &uri, serde_json::json!({"rating": 3.0})).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
