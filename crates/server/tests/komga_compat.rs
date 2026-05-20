//! Integration tests for progress-writeback-2.0 M2 + M3 — the Komga
//! compatibility shim that lets Panels (iOS/macOS) and Tachiyomi-class
//! clients sync reading progress by treating Folio as Komga.
//!
//! M2 — OPDS fingerprint: when `compat.opds_panels_mode = "komga"` is
//! set, the OPDS root feed embeds `<author><name>Komga</name></author>`
//! and titles itself `Komga OPDS catalog`; the `/opds/v1.2/catalog`
//! path alias maps to the same root handler.
//!
//! M3 — Komga REST shim: `PATCH /api/v1/books/{id}/read-progress`
//! writes progress, `GET /api/v1/books/{id}` returns Komga-shaped
//! JSON with the user's `readProgress` block.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
    #[allow(dead_code)] // unused by M2 tests; M3 progress-seeding tests will use it.
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

async fn body_text(b: Body) -> String {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn register_admin(app: &TestApp, email: &str) -> Authed {
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
    let json_: serde_json::Value = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json_["user"]["id"].as_str().unwrap()).unwrap();
    // First-registered user becomes admin by Folio's default seed rule,
    // so the admin endpoints below are reachable without a second step.
    Authed {
        session,
        csrf,
        user_id,
    }
}

async fn get_cookie(app: &TestApp, uri: &str, auth: &Authed) -> Response<Body> {
    app.router
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
        .unwrap()
}

async fn enable_komga_compat(app: &TestApp, admin: &Authed) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/api/admin/settings")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, admin.cookies())
                .header("X-CSRF-Token", &admin.csrf)
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "compat.opds_panels_mode": "komga"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "compat setting accepted: body={}",
        body_text(resp.into_body()).await
    );
}

// ────────────── M2 — OPDS fingerprint ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compat_off_omits_komga_author_from_root_feed() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app, "compat-off@example.com").await;

    let resp = get_cookie(&app, "/opds/v1", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(
        !body.contains("<name>Komga</name>"),
        "default-off OPDS root must not advertise as Komga: {body}"
    );
    assert!(
        body.contains("<title>Comic Reader</title>"),
        "default-off root title remains Folio's `Comic Reader`: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compat_komga_emits_author_and_title_on_root_feed() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app, "compat-on@example.com").await;
    enable_komga_compat(&app, &auth).await;

    let resp = get_cookie(&app, "/opds/v1", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains("<name>Komga</name>"),
        "Komga compat root feed embeds the Komga author element: {body}"
    );
    assert!(
        body.contains("<uri>https://github.com/gotson/komga</uri>"),
        "Komga compat root feed embeds the canonical Komga URI: {body}"
    );
    assert!(
        body.contains("<title>Komga OPDS catalog</title>"),
        "Komga compat root feed titles itself like Komga's: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compat_komga_aliases_v1_2_catalog_to_root() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app, "v1-2-path@example.com").await;
    enable_komga_compat(&app, &auth).await;

    let resp = get_cookie(&app, "/opds/v1.2/catalog", &auth).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "Komga's canonical catalog path resolves to the root handler"
    );
    let body = body_text(resp.into_body()).await;
    assert!(
        body.contains("<name>Komga</name>"),
        "the alias path emits the same Komga-fingerprinted root feed: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compat_off_v1_2_catalog_path_still_resolves_as_folio() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app, "v1-2-off@example.com").await;

    // The route is registered unconditionally — easier than rebuilding
    // the router on flag flip. When compat is off, the alias path
    // still resolves but emits the Folio-branded root feed.
    let resp = get_cookie(&app, "/opds/v1.2/catalog", &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp.into_body()).await;
    assert!(
        !body.contains("<name>Komga</name>"),
        "compat off: alias path serves Folio identity, not Komga's: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compat_komga_can_be_toggled_back_off() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app, "toggle-off@example.com").await;
    enable_komga_compat(&app, &auth).await;

    // Sanity: compat is on.
    let on_body = body_text(get_cookie(&app, "/opds/v1", &auth).await.into_body()).await;
    assert!(on_body.contains("<name>Komga</name>"));

    // Flip back to `off` via PATCH.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/api/admin/settings")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookies())
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "compat.opds_panels_mode": "off"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let off_body = body_text(get_cookie(&app, "/opds/v1", &auth).await.into_body()).await;
    assert!(
        !off_body.contains("<name>Komga</name>"),
        "after toggling off, the Komga author element is gone: {off_body}"
    );
    assert!(
        off_body.contains("<title>Comic Reader</title>"),
        "after toggling off, the Folio title is restored: {off_body}"
    );
}

// ────────────── M3 — Komga REST shim ──────────────

use common::seed::{seed_issue, seed_library, seed_series};
use sea_orm::Database;

async fn http_with_csrf(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> Response<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, auth.cookies())
        .header("X-CSRF-Token", &auth.csrf);
    if body.is_some() {
        req = req.header(header::CONTENT_TYPE, "application/json");
    }
    let body = match body {
        Some(v) => Body::from(serde_json::to_string(&v).unwrap()),
        None => Body::empty(),
    };
    app.router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_read_progress_writes_progress_record() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app, "m3-patch@example.com").await;
    enable_komga_compat(&app, &auth).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Komga PATCH").await;
    let issue_id = seed_issue(&db, lib, series, &tmp.path().join("k.cbz"), b"kp-1", 1.0).await;

    let resp = http_with_csrf(
        &app,
        &auth,
        Method::PATCH,
        &format!("/api/v1/books/{issue_id}/read-progress"),
        Some(json!({ "page": 7 })),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "Komga's contract returns 204 on successful PATCH"
    );

    // GET back and confirm the round-trip: Komga's wire page is
    // 1-indexed, so DB last_page = 6 after PATCH page=7.
    let resp = http_with_csrf(
        &app,
        &auth,
        Method::GET,
        &format!("/api/v1/books/{issue_id}"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = body_json(resp.into_body()).await;
    assert_eq!(body["readProgress"]["page"], 7, "round-trip 1-indexed");
    assert_eq!(body["readProgress"]["completed"], false);
    assert!(body["readProgress"]["lastModified"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_with_completed_true_marks_finished() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app, "m3-complete@example.com").await;
    enable_komga_compat(&app, &auth).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Komga complete").await;
    let issue_id = seed_issue(&db, lib, series, &tmp.path().join("k.cbz"), b"kc-1", 1.0).await;

    let resp = http_with_csrf(
        &app,
        &auth,
        Method::PATCH,
        &format!("/api/v1/books/{issue_id}/read-progress"),
        Some(json!({ "completed": true })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = http_with_csrf(
        &app,
        &auth,
        Method::GET,
        &format!("/api/v1/books/{issue_id}"),
        None,
    )
    .await;
    let body: serde_json::Value = body_json(resp.into_body()).await;
    assert_eq!(body["readProgress"]["completed"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_without_page_or_completed_returns_422() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app, "m3-empty@example.com").await;
    enable_komga_compat(&app, &auth).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Komga empty").await;
    let issue_id = seed_issue(&db, lib, series, &tmp.path().join("k.cbz"), b"ke-1", 1.0).await;

    let resp = http_with_csrf(
        &app,
        &auth,
        Method::PATCH,
        &format!("/api/v1/books/{issue_id}/read-progress"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "Komga rejects body without at least one of `page` / `completed`"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_unknown_book_returns_404() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app, "m3-404@example.com").await;
    enable_komga_compat(&app, &auth).await;

    let resp = http_with_csrf(
        &app,
        &auth,
        Method::PATCH,
        "/api/v1/books/nonexistent-issue-id/read-progress",
        Some(json!({ "page": 1 })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_book_returns_read_progress_null_when_unread() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app, "m3-unread@example.com").await;
    enable_komga_compat(&app, &auth).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Komga unread").await;
    let issue_id = seed_issue(&db, lib, series, &tmp.path().join("k.cbz"), b"ku-1", 1.0).await;

    let resp = http_with_csrf(
        &app,
        &auth,
        Method::GET,
        &format!("/api/v1/books/{issue_id}"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = body_json(resp.into_body()).await;
    assert!(
        body["readProgress"].is_null(),
        "readProgress is null when no progress row exists: {body}"
    );
    assert_eq!(body["id"], issue_id);
    assert!(body["seriesId"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn endpoints_return_404_when_compat_off() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app, "m3-disabled@example.com").await;
    // Note: compat is OFF (default for new TestApp::spawn).
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = seed_series(&db, lib, "Compat off").await;
    let issue_id = seed_issue(&db, lib, series, &tmp.path().join("k.cbz"), b"kdis-1", 1.0).await;

    let get_resp = http_with_csrf(
        &app,
        &auth,
        Method::GET,
        &format!("/api/v1/books/{issue_id}"),
        None,
    )
    .await;
    assert_eq!(
        get_resp.status(),
        StatusCode::NOT_FOUND,
        "GET /api/v1/books/.. returns 404 when compat is off"
    );

    let patch_resp = http_with_csrf(
        &app,
        &auth,
        Method::PATCH,
        &format!("/api/v1/books/{issue_id}/read-progress"),
        Some(json!({ "page": 1 })),
    )
    .await;
    assert_eq!(
        patch_resp.status(),
        StatusCode::NOT_FOUND,
        "PATCH /api/v1/books/.. returns 404 when compat is off"
    );
}
