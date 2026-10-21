//! Integration tests for `crates/server/src/api/issue_notes.rs` — the
//! per-user, issue-level closing note written from the reader's Wrap up
//! pane. Covers set / update / clear (null and blank), the length gate,
//! the visibility 404, and the inline `user_note` on the issue detail.

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

async fn body_json(b: Body) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(b, usize::MAX).await.unwrap()).unwrap()
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

async fn put_note(
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

async fn get_json(app: &TestApp, auth: &Authed, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::COOKIE, auth.cookies())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn stored_note(app: &TestApp, user_id: Uuid, issue_id: &str) -> Option<String> {
    use entity::user_issue_note;
    use sea_orm::EntityTrait;
    let db = Database::connect(&app.db_url).await.unwrap();
    user_issue_note::Entity::find_by_id((user_id, issue_id.to_owned()))
        .one(&db)
        .await
        .unwrap()
        .map(|m| m.body)
}

/// A library + series + one issue the registered user can see, plus the
/// issue's `(series_slug, issue_slug)` for the URL. Same shape as
/// `tests/ratings.rs`.
async fn seed_visible_issue(
    app: &TestApp,
    tmp: &std::path::Path,
    email: &str,
) -> (String, String, String, Authed) {
    use entity::{library_user_access, series};
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};

    let auth = register(app, email).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = seed_library(&db, tmp).await;
    let series_id = seed_series(&db, lib_id, "Noteworthy").await;
    let issue_id = seed_issue(&db, lib_id, series_id, &tmp.join("a.cbz"), b"note-a", 1.0).await;
    let series_row = series::Entity::find_by_id(series_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let issue_row = entity::issue::Entity::find_by_id(issue_id.clone())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let now = chrono::Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(auth.user_id),
        library_id: Set(lib_id),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    (series_row.slug, issue_row.slug, issue_id, auth)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn note_set_update_and_clear() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (series_slug, issue_slug, issue_id, auth) =
        seed_visible_issue(&app, tmp.path(), "noter@example.com").await;
    let uri = format!("/api/series/{series_slug}/issues/{issue_slug}/note");

    // Set.
    let resp = put_note(
        &app,
        &auth,
        &uri,
        serde_json::json!({"body": "  Loved the last act.  "}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["body"].as_str(), Some("Loved the last act."));
    assert!(body["updated_at"].as_str().is_some());
    assert_eq!(
        stored_note(&app, auth.user_id, &issue_id).await.as_deref(),
        Some("Loved the last act.")
    );

    // Update in place — still one row.
    let resp = put_note(
        &app,
        &auth,
        &uri,
        serde_json::json!({"body": "Second thoughts."}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        stored_note(&app, auth.user_id, &issue_id).await.as_deref(),
        Some("Second thoughts.")
    );

    // Detail view inlines it.
    let (status, detail) = get_json(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/{issue_slug}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["user_note"].as_str(), Some("Second thoughts."));

    // Blank clears, same as null.
    let resp = put_note(&app, &auth, &uri, serde_json::json!({"body": "   \n"})).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert!(body["body"].is_null());
    assert!(body["updated_at"].is_null());
    assert_eq!(stored_note(&app, auth.user_id, &issue_id).await, None);

    // Null on an already-clear note is idempotent.
    let resp = put_note(&app, &auth, &uri, serde_json::json!({"body": null})).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (_, detail) = get_json(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/{issue_slug}"),
    )
    .await;
    assert!(detail["user_note"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn note_rejects_oversized_body_with_422() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (series_slug, issue_slug, issue_id, auth) =
        seed_visible_issue(&app, tmp.path(), "longnoter@example.com").await;
    let uri = format!("/api/series/{series_slug}/issues/{issue_slug}/note");
    let huge = "x".repeat(server::api::issue_notes::MAX_NOTE_CHARS + 1);
    let resp = put_note(&app, &auth, &uri, serde_json::json!({"body": huge})).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"].as_str(), Some("validation"));
    assert_eq!(stored_note(&app, auth.user_id, &issue_id).await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn note_404s_when_issue_invisible_to_caller() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (series_slug, issue_slug, issue_id, _owner) =
        seed_visible_issue(&app, tmp.path(), "owner@example.com").await;
    let other = register(&app, "other@example.com").await;
    let uri = format!("/api/series/{series_slug}/issues/{issue_slug}/note");
    let resp = put_note(&app, &other, &uri, serde_json::json!({"body": "peek"})).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(stored_note(&app, other.user_id, &issue_id).await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notes_are_per_user() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (series_slug, issue_slug, issue_id, a) =
        seed_visible_issue(&app, tmp.path(), "a@example.com").await;
    // Second user with access to the same library.
    let b = register(&app, "b@example.com").await;
    {
        use entity::{issue, library_user_access};
        use sea_orm::{ActiveModelTrait, EntityTrait, Set};
        let db = Database::connect(&app.db_url).await.unwrap();
        let row = issue::Entity::find_by_id(issue_id.clone())
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        let now = chrono::Utc::now().fixed_offset();
        library_user_access::ActiveModel {
            user_id: Set(b.user_id),
            library_id: Set(row.library_id),
            age_rating_max: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&db)
        .await
        .unwrap();
    }
    let uri = format!("/api/series/{series_slug}/issues/{issue_slug}/note");
    put_note(&app, &a, &uri, serde_json::json!({"body": "A's take"})).await;
    put_note(&app, &b, &uri, serde_json::json!({"body": "B's take"})).await;
    assert_eq!(
        stored_note(&app, a.user_id, &issue_id).await.as_deref(),
        Some("A's take")
    );
    assert_eq!(
        stored_note(&app, b.user_id, &issue_id).await.as_deref(),
        Some("B's take")
    );
    let (_, detail) = get_json(
        &app,
        &b,
        &format!("/api/series/{series_slug}/issues/{issue_slug}"),
    )
    .await;
    assert_eq!(detail["user_note"].as_str(), Some("B's take"));
}
