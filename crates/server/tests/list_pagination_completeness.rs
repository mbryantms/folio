//! Regression guard for the uniform `CursorPage<T>` list envelope
//! (audit-remediation M4 + M4-residual, shipped 2026-05-23).
//!
//! Before M4 the three endpoints below returned ad-hoc shapes:
//!   - `GET /me/pages` → `Vec<PageView>` (bare array)
//!   - `GET /me/log/widgets` → `{ widgets: Vec<LogWidgetView> }`
//!   - `GET /admin/stats/users` → `{ users: Vec<AdminUserStatsRow> }`
//!
//! M4-residual added the `/filter-options/*` family (9 endpoints) which
//! used to return `{ values: Vec<String> }` capped at 200 rows. Now
//! they return `CursorPage<String>` with proper cursor walking via
//! `?cursor=` / `?limit=`.
//!
//! Each endpoint returns `{ items, next_cursor, total? }`. Bounded
//! endpoints (pages cap, widget kind set, deployment user count)
//! return `next_cursor: null`; filter-options walks the catalog with
//! `next_cursor` set when more rows exist. Tests assert the body
//! shape so a regression that drops back to a bare array or renames
//! `items` fails the build.
//!
//! See `decision #4` in the audit-remediation plan: "Paginated<T>
//! envelope always — bounded endpoints return `next_cursor: None`."

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use serde_json::Value;
use tower::ServiceExt;

async fn body_json(b: Body) -> Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

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

async fn register_authed(app: &TestApp, email: &str, password: &str) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"{password}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let cookies: Vec<String> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect();
    let extract = |needle: &str| -> String {
        cookies
            .iter()
            .find_map(|c| c.split(';').next()?.strip_prefix(needle).map(str::to_owned))
            .unwrap_or_default()
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn get(app: &TestApp, auth: &Authed, path: &str) -> Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "GET {path}");
    body_json(resp.into_body()).await
}

fn assert_cursor_envelope(body: &Value, path: &str) {
    assert!(
        body.is_object(),
        "{path}: body should be an envelope object, got {body}"
    );
    let items = body.get("items").unwrap_or_else(|| {
        panic!("{path}: missing `items` field — should be `CursorPage<T>` envelope, got {body}")
    });
    assert!(
        items.is_array(),
        "{path}: `items` should be an array, got {items}"
    );
    let next_cursor = body.get("next_cursor").unwrap_or_else(|| {
        panic!("{path}: missing `next_cursor` field — uniform envelope requires it (null is fine)")
    });
    assert!(
        next_cursor.is_null() || next_cursor.is_string(),
        "{path}: `next_cursor` should be string or null, got {next_cursor}"
    );
}

#[tokio::test]
async fn me_pages_returns_cursor_envelope() {
    let app = TestApp::spawn().await;
    let user = register_authed(&app, "u@example.com", "correctly-horse-battery").await;
    let body = get(&app, &user, "/api/me/pages").await;
    assert_cursor_envelope(&body, "/me/pages");
    // System "Home" page is auto-created on first list — assert at least
    // one item so a regression that returns an empty body trips the test.
    let items = body["items"].as_array().unwrap();
    assert!(
        !items.is_empty(),
        "/me/pages: expected at least the system Home page, got empty array"
    );
    // Page rows carry both id + slug + is_system; spot-check the system row.
    let has_home = items
        .iter()
        .any(|p| p["is_system"] == true && p["slug"] == "home");
    assert!(has_home, "/me/pages: missing system Home row in {body}");
}

#[tokio::test]
async fn me_log_widgets_returns_cursor_envelope() {
    let app = TestApp::spawn().await;
    let user = register_authed(&app, "u@example.com", "correctly-horse-battery").await;
    let body = get(&app, &user, "/api/me/log/widgets").await;
    assert_cursor_envelope(&body, "/me/log/widgets");
    // Defaults are seeded lazily on first read — assert non-empty so a
    // regression that skips the seeding (or drops the envelope shape)
    // trips the test.
    let items = body["items"].as_array().unwrap();
    assert!(
        !items.is_empty(),
        "/me/log/widgets: expected seeded defaults, got empty"
    );
}

#[tokio::test]
async fn admin_stats_users_returns_cursor_envelope() {
    let app = TestApp::spawn().await;
    // The first registered user becomes admin (Folio bootstrap).
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let body = get(&app, &admin, "/api/admin/stats/users").await;
    assert_cursor_envelope(&body, "/admin/stats/users");
    let items = body["items"].as_array().unwrap();
    assert!(
        !items.is_empty(),
        "/admin/stats/users: expected the admin row, got empty"
    );
}

/// M4-residual: every `/filter-options/*` endpoint returns
/// `CursorPage<String>`. Catalog may be empty in a fresh test DB so we
/// only assert envelope shape — `items` is `[]` and `next_cursor` is
/// `null`, but the keys must be present and typed correctly so a
/// regression to the old `{values: [...]}` shape fails the build.
#[tokio::test]
async fn filter_options_return_cursor_envelope() {
    let app = TestApp::spawn().await;
    let user = register_authed(&app, "u@example.com", "correctly-horse-battery").await;
    for path in [
        "/api/filter-options/genres",
        "/api/filter-options/tags",
        "/api/filter-options/publishers",
        "/api/filter-options/languages",
        "/api/filter-options/age_ratings",
        "/api/filter-options/characters",
        "/api/filter-options/teams",
        "/api/filter-options/locations",
        "/api/filter-options/credits/writer",
    ] {
        let body = get(&app, &user, path).await;
        assert_cursor_envelope(&body, path);
        // Items are strings (no opaque IDs in these catalogs).
        for v in body["items"].as_array().unwrap() {
            assert!(v.is_string(), "{path}: items should be strings, got {v}");
        }
    }
}
