//! `/me/series/{series_id}/reader-prefs` (post-audit T4.4): per-user,
//! per-series reader overrides that follow the account. PUT has PATCH
//! semantics per slice (absent = keep, null = clear), clearing every
//! slice drops the row, DELETE resets, users never see each other's rows.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{LibrarySeed, SeriesSeed};
use entity::user_series_reader_prefs as prefs;
use sea_orm::EntityTrait;
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
            .and_then(|c| c.split(';').next())
            .map(|kv| kv.split_once('=').map(|(_, v)| v.to_owned()).unwrap())
            .unwrap()
    };
    let body = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(body["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id,
    }
}

async fn http(
    app: &TestApp,
    method: Method,
    path: &str,
    user: Option<&Authed>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder().method(method.clone()).uri(path);
    if let Some(u) = user {
        req = req.header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                u.session, u.csrf
            ),
        );
        if method != Method::GET {
            req = req.header("X-CSRF-Token", &u.csrf);
        }
    }
    let body_bytes = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(req.body(body_bytes).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn seed_series(app: &TestApp) -> Uuid {
    let tmp = tempfile::tempdir().unwrap();
    let db = &app.state().db;
    let lib_id = LibrarySeed::new(tmp.path()).insert(db).await;
    SeriesSeed::new(lib_id, "Prefs Series").insert(db).await
}

#[tokio::test]
async fn put_merges_per_slice_and_get_reads_back() {
    let app = TestApp::spawn().await;
    let me = register(&app, "srp-1@example.com").await;
    let series_id = seed_series(&app).await;
    let url = format!("/api/me/series/{series_id}/reader-prefs");

    let (status, body) = http(&app, Method::GET, &url, Some(&me), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({}), "no row → empty object");

    let (status, body) = http(
        &app,
        Method::PUT,
        &url,
        Some(&me),
        Some(serde_json::json!({ "reading_direction": "rtl", "fit_mode": "height" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(
        body,
        serde_json::json!({ "reading_direction": "rtl", "fit_mode": "height" })
    );

    // Absent slices are kept, null clears, a value sets.
    let (status, body) = http(
        &app,
        Method::PUT,
        &url,
        Some(&me),
        Some(serde_json::json!({ "fit_mode": null, "view_mode": "webtoon", "cover_solo": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(
        body,
        serde_json::json!({ "reading_direction": "rtl", "view_mode": "webtoon", "cover_solo": false })
    );
    let (_, body) = http(&app, Method::GET, &url, Some(&me), None).await;
    assert_eq!(body["reading_direction"], "rtl");
    assert_eq!(body["view_mode"], "webtoon");
    assert_eq!(body["cover_solo"], false);
    assert!(body.get("fit_mode").is_none());

    // Clearing the last slices drops the row.
    let (status, body) = http(
        &app,
        Method::PUT,
        &url,
        Some(&me),
        Some(
            serde_json::json!({ "reading_direction": null, "view_mode": null, "cover_solo": null }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(body, serde_json::json!({}));
    let row = prefs::Entity::find_by_id((me.user_id, series_id))
        .one(&app.state().db)
        .await
        .unwrap();
    assert!(row.is_none(), "an all-NULL row is deleted");
}

#[tokio::test]
async fn delete_resets_and_users_are_isolated() {
    let app = TestApp::spawn().await;
    let me = register(&app, "srp-2@example.com").await;
    let other = register(&app, "srp-2b@example.com").await;
    let series_id = seed_series(&app).await;
    let url = format!("/api/me/series/{series_id}/reader-prefs");

    let (status, _) = http(
        &app,
        Method::PUT,
        &url,
        Some(&me),
        Some(serde_json::json!({ "view_mode": "double", "cover_solo": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, theirs) = http(&app, Method::GET, &url, Some(&other), None).await;
    assert_eq!(theirs, serde_json::json!({}), "another user sees nothing");

    let (status, _) = http(&app, Method::DELETE, &url, Some(&me), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, mine) = http(&app, Method::GET, &url, Some(&me), None).await;
    assert_eq!(mine, serde_json::json!({}));
    // Deleting again is still a clean 204.
    let (status, _) = http(&app, Method::DELETE, &url, Some(&me), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn rejects_unknown_tokens_missing_series_and_anonymous_callers() {
    let app = TestApp::spawn().await;
    let me = register(&app, "srp-3@example.com").await;
    let series_id = seed_series(&app).await;
    let url = format!("/api/me/series/{series_id}/reader-prefs");

    let (status, body) = http(
        &app,
        Method::PUT,
        &url,
        Some(&me),
        Some(serde_json::json!({ "fit_mode": "enormous" })),
    )
    .await;
    assert!(status.is_client_error(), "{status} {body:#?}");
    assert!(body["error"]["code"].is_string(), "{body:#?}");

    let (status, _) = http(
        &app,
        Method::PUT,
        &format!("/api/me/series/{}/reader-prefs", Uuid::now_v7()),
        Some(&me),
        Some(serde_json::json!({ "fit_mode": "width" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = http(&app, Method::GET, &url, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
