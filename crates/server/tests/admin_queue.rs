//! Integration tests for `/admin/queue/*` — covers the audit-log
//! discipline gap closed in M1 of the incompleteness-cleanup plan
//! (audit finding B-1).

mod common;

use apalis::prelude::Storage;
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use redis::AsyncCommands;
use sea_orm::EntityTrait;
use serde_json::{Value, json};
use server::jobs::scan;
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

async fn body_json(resp: axum::http::Response<Body>) -> Value {
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
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
        "/api/admin/queue/clear",
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
            "/api/admin/queue/clear",
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
        "/api/admin/queue/clear",
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

// ───── Dead-job inspection / retry / purge (D8b) ─────

/// `(dead_set, data_hash, result_hash, conn)` for the `scan` queue, resolved
/// from the app's own apalis storage config so the keys match production.
fn scan_dead_keys(app: &TestApp) -> (String, String, String, redis::aio::ConnectionManager) {
    let st = app.state();
    let storage = st.jobs.scan_storage.clone();
    let data = storage.get_config().job_data_hash();
    let result = format!("{data}::result");
    let dead = storage.get_config().dead_jobs_set();
    (dead, data, result, st.jobs.redis.clone())
}

#[tokio::test]
async fn dead_jobs_lists_seeded_failures_newest_first() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let (dead, data, result, mut conn) = scan_dead_keys(&app);

    // Seed two dead jobs: older (score 1000) + newer (score 2000). The list
    // pulls `args` out of the stored `Request` JSON, so seed that shape.
    for (id, score, lib) in [
        ("task-old", 1000i64, "lib-a"),
        ("task-new", 2000i64, "lib-b"),
    ] {
        let _: i64 = conn.zadd(&dead, id, score).await.unwrap();
        let blob = json!({ "args": { "library_id": lib, "force": false }, "parts": {} });
        let _: i64 = conn.hset(&data, id, blob.to_string()).await.unwrap();
        let _: i64 = conn.hset(&result, id, format!("boom {id}")).await.unwrap();
    }

    let resp = send_authed(
        &app,
        &admin,
        Method::GET,
        "/api/admin/queue/dead-jobs?queue=scan",
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;

    assert_eq!(body["queue"], "scan");
    assert_eq!(body["total"], 2);
    let jobs = body["jobs"].as_array().expect("jobs array");
    assert_eq!(jobs.len(), 2);
    // Newest-killed first.
    assert_eq!(jobs[0]["task_id"], "task-new");
    assert_eq!(jobs[0]["failed_at"], 2000);
    assert_eq!(jobs[0]["error"], "boom task-new");
    assert_eq!(
        jobs[0]["payload"]["library_id"], "lib-b",
        "payload should be the job's args sub-object"
    );
    assert_eq!(jobs[1]["task_id"], "task-old");
}

#[tokio::test]
async fn dead_jobs_unknown_queue_returns_422() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    let resp = send_authed(
        &app,
        &admin,
        Method::GET,
        "/api/admin/queue/dead-jobs?queue=bogus",
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(resp).await;
    assert_eq!(body["error"]["code"], "unknown_queue");
}

#[tokio::test]
async fn retry_dead_job_reenqueues_and_audits() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let st = app.state();
    let mut storage = st.jobs.scan_storage.clone();

    // Enqueue a real job so a *valid* serialized Request lands in the data
    // hash (what the typed retry deserializes), then simulate apalis's kill:
    // pull it off the pending list and drop it into the dead set.
    let parts = storage
        .push(scan::Job {
            library_id: Uuid::now_v7(),
            scan_run_id: Uuid::now_v7(),
            force: false,
        })
        .await
        .expect("push");
    let task_id = parts.task_id.to_string();

    let active = storage.get_config().active_jobs_list();
    let (dead, _data, result, mut conn) = scan_dead_keys(&app);
    let _: i64 = conn.lrem(&active, 0, &task_id).await.unwrap();
    let _: i64 = conn.zadd(&dead, &task_id, 1000i64).await.unwrap();
    let _: i64 = conn.hset(&result, &task_id, "boom").await.unwrap();

    // Precondition: nothing pending, exactly one dead.
    assert_eq!(
        storage.len().await.unwrap(),
        0,
        "pending drained for the sim"
    );
    let dead_n: i64 = conn.zcard(&dead).await.unwrap();
    assert_eq!(dead_n, 1);

    let resp = send_authed(
        &app,
        &admin,
        Method::POST,
        "/api/admin/queue/dead-jobs/retry",
        Some(json!({ "queue": "scan", "task_id": task_id })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["retried"], true);

    // Old dead entry gone; a fresh copy re-enqueued onto pending.
    let dead_after: i64 = conn.zcard(&dead).await.unwrap();
    assert_eq!(dead_after, 0, "old dead entry removed on retry");
    assert_eq!(
        storage.len().await.unwrap(),
        1,
        "a fresh job was re-enqueued onto the pending list"
    );

    use entity::audit_log;
    let rows = audit_log::Entity::find().all(&st.db).await.unwrap();
    assert!(
        rows.iter().any(|r| r.action == "admin.queue.job.retry"
            && r.target_id.as_deref() == Some(task_id.as_str())),
        "expected an admin.queue.job.retry audit row for {task_id}"
    );
}

#[tokio::test]
async fn retry_unknown_dead_job_returns_404() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;

    // Well-formed ULID that isn't in the dead set.
    let resp = send_authed(
        &app,
        &admin,
        Method::POST,
        "/api/admin/queue/dead-jobs/retry",
        Some(json!({ "queue": "scan", "task_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn purge_dead_jobs_clears_set_and_audits() {
    let app = TestApp::spawn().await;
    let admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let (dead, data, result, mut conn) = scan_dead_keys(&app);

    for id in ["d1", "d2", "d3"] {
        let _: i64 = conn.zadd(&dead, id, 1000i64).await.unwrap();
        let _: i64 = conn.hset(&data, id, "{}").await.unwrap();
        let _: i64 = conn.hset(&result, id, "boom").await.unwrap();
    }

    let resp = send_authed(
        &app,
        &admin,
        Method::POST,
        "/api/admin/queue/dead-jobs/purge",
        Some(json!({ "queue": "scan" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["purged"], 3);

    let remaining: i64 = conn.zcard(&dead).await.unwrap();
    assert_eq!(remaining, 0, "dead set emptied");

    use entity::audit_log;
    let rows = audit_log::Entity::find()
        .all(&app.state().db)
        .await
        .unwrap();
    assert!(
        rows.iter()
            .any(|r| r.action == "admin.queue.dead.purge" && r.payload["purged"] == 3),
        "expected an admin.queue.dead.purge audit row"
    );
}

#[tokio::test]
async fn dead_job_endpoints_require_admin() {
    let app = TestApp::spawn().await;
    let _admin = register_authed(&app, "admin@example.com", "correctly-horse-battery").await;
    let user = register_authed(&app, "user@example.com", "correctly-horse-battery").await;

    let list = send_authed(
        &app,
        &user,
        Method::GET,
        "/api/admin/queue/dead-jobs?queue=scan",
        None,
    )
    .await;
    assert_eq!(list.status(), StatusCode::FORBIDDEN);

    let retry = send_authed(
        &app,
        &user,
        Method::POST,
        "/api/admin/queue/dead-jobs/retry",
        Some(json!({ "queue": "scan", "task_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV" })),
    )
    .await;
    assert_eq!(retry.status(), StatusCode::FORBIDDEN);

    let purge = send_authed(
        &app,
        &user,
        Method::POST,
        "/api/admin/queue/dead-jobs/purge",
        Some(json!({ "queue": "scan" })),
    )
    .await;
    assert_eq!(purge.status(), StatusCode::FORBIDDEN);
}
