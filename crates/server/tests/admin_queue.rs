//! Integration tests for `/admin/queue/*` — covers the audit-log
//! discipline gap closed in M1 of the incompleteness-cleanup plan
//! (audit finding B-1).

mod common;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use sea_orm::EntityTrait;
use serde_json::{Value, json};
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
    assert_eq!(resp.status(), StatusCode::CREATED, "registration failed");
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

async fn send_authed(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> axum::http::Response<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, auth.cookie())
        .header("x-csrf-token", &auth.csrf);
    let body_inner = if let Some(json) = body {
        b = b.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&json).unwrap())
    } else {
        Body::empty()
    };
    app.router
        .clone()
        .oneshot(b.body(body_inner).unwrap())
        .await
        .unwrap()
}

// ───── Audit log (B-1) ─────

#[tokio::test]
async fn clear_queue_writes_audit_row() {
    // The B-1 audit-finding fix: every mutating admin handler emits via
    // `crate::audit::record` per CLAUDE.md. `clear_queue` was the
    // singular gap; this asserts the row lands with the expected shape.
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = send_authed(
        &app,
        &admin,
        Method::POST,
        "/admin/queue/clear",
        Some(json!({ "target": "all" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    use entity::audit_log;
    let rows = audit_log::Entity::find()
        .all(&app.state().db)
        .await
        .expect("audit_log query");
    let row = rows
        .iter()
        .find(|r| r.action == "admin.queue.clear")
        .expect("no admin.queue.clear audit row written");

    // Forensic invariants on the audit row.
    assert_eq!(row.target_type.as_deref(), Some("queue"));
    assert!(
        row.target_id.is_none(),
        "queue is not a single addressable entity; target_id must be None"
    );
    assert_eq!(row.actor_type, "user");

    // Payload shape: target tag + before/after depth snapshots so the
    // row is self-documenting without cross-referencing.
    let payload = &row.payload;
    assert_eq!(payload["target"], "all");
    assert!(
        payload.get("deleted_keys").is_some(),
        "payload missing deleted_keys"
    );
    assert!(
        payload["before"].is_object(),
        "payload.before should be the queue-depth snapshot"
    );
    assert!(payload["after"].is_object(), "payload.after missing");
    assert!(
        payload["before"].get("total").is_some(),
        "before.total absent — depth shape changed unexpectedly"
    );
}

#[tokio::test]
async fn clear_queue_audit_uses_target_tag_from_request() {
    // Confirm the audit row's `payload.target` mirrors the request — so
    // an operator scanning audit history can tell at a glance whether
    // someone cleared everything or just the thumbnail queue.
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    for target in ["scans", "thumbnails"] {
        let resp = send_authed(
            &app,
            &admin,
            Method::POST,
            "/admin/queue/clear",
            Some(json!({ "target": target })),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK, "{target} clear failed");
    }

    use entity::audit_log;
    let rows = audit_log::Entity::find()
        .all(&app.state().db)
        .await
        .expect("audit_log query");
    let actions: Vec<_> = rows
        .iter()
        .filter(|r| r.action == "admin.queue.clear")
        .map(|r| r.payload["target"].as_str().unwrap_or("").to_owned())
        .collect();
    assert!(
        actions.iter().any(|t| t == "scans"),
        "no audit row tagged target=scans (got {actions:?})"
    );
    assert!(
        actions.iter().any(|t| t == "thumbnails"),
        "no audit row tagged target=thumbnails (got {actions:?})"
    );
}

#[tokio::test]
async fn clear_queue_requires_admin() {
    // Non-admin caller must hit the RequireAdmin gate (403) AND must
    // not write an audit row — the action never happened.
    let app = TestApp::spawn().await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;

    let resp = send_authed(
        &app,
        &user,
        Method::POST,
        "/admin/queue/clear",
        Some(json!({ "target": "all" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    use entity::audit_log;
    let rows = audit_log::Entity::find()
        .all(&app.state().db)
        .await
        .expect("audit_log query");
    assert!(
        !rows.iter().any(|r| r.action == "admin.queue.clear"),
        "non-admin got blocked but an audit row was written anyway"
    );
}
