//! Mylar snapshot ingest (upcoming-releases 1.0, part B):
//! `POST /integrations/mylar/snapshot` and the admin token endpoints.
//!
//! Covers:
//! - the integration off answers 403 even to the right token;
//! - no token / a wrong token gets 401 before the body is read;
//! - a snapshot is stored, and the next one replaces it whole;
//! - an unchanged snapshot is a heartbeat (clocks move, rows do not);
//! - an unsupported schema version, an out-of-range value, malformed
//!   JSON and an oversized body are refused with the house envelope;
//! - the token endpoints are admin-only and audited; replacing or
//!   revoking the token cuts the old one off; revoke can purge;
//! - the status endpoint reports the snapshot's freshness by Folio's
//!   own receive time.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use common::TestApp;
use entity::{integration_token, mylar_pull, mylar_snapshot, mylar_wanted, mylar_watch};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};
use serde_json::{Value, json};
use tower::ServiceExt;

const INGEST: &str = "/integrations/mylar/snapshot";
const TOKEN: &str = "/api/admin/integrations/mylar/token";
const STATUS: &str = "/api/admin/upcoming/status";

// ────────────────────── harness ──────────────────────

fn set_enabled(app: &TestApp, upcoming: bool, mylar: bool) {
    let mut cfg = (*app.state().cfg()).clone();
    cfg.upcoming_enabled = upcoming;
    cfg.integrations_mylar_enabled = mylar;
    app.state().replace_cfg(cfg);
}

struct Authed {
    session: String,
    csrf: String,
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
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
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

/// A request as the signed-in browser would send it.
async fn as_user(app: &TestApp, method: Method, uri: &str, user: &Authed) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method.clone()).uri(uri).header(
        header::COOKIE,
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            user.session, user.csrf
        ),
    );
    if method != Method::GET {
        req = req.header("X-CSRF-Token", &user.csrf);
    }
    let resp = app
        .router
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// A push as the script sends it: no cookie, no CSRF header.
async fn push_raw(
    app: &TestApp,
    token: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, Value, Option<String>) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(INGEST)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::USER_AGENT, "folio-mylar-push/1");
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = app
        .router
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let challenge = resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    (status, body_json(resp.into_body()).await, challenge)
}

async fn push(app: &TestApp, token: &str, snapshot: &Value) -> (StatusCode, Value) {
    let (status, body, _) = push_raw(app, Some(token), serde_json::to_vec(snapshot).unwrap()).await;
    (status, body)
}

async fn new_token(app: &TestApp, admin: &Authed) -> String {
    let (status, body) = as_user(app, Method::POST, TOKEN, admin).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["token"]
        .as_str()
        .expect("token in the response")
        .to_owned()
}

/// The first registered user is the admin; the integration is on.
async fn ready() -> (TestApp, Authed, String) {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    set_enabled(&app, true, true);
    let token = new_token(&app, &admin).await;
    (app, admin, token)
}

fn snapshot() -> Value {
    json!({
        "meta": {"schema_version": 1, "generated_at": "2026-11-15T03:36:49+00:00",
                 "mylar_db_version": 1, "annuals_included": false},
        "pull_list": [
            {"ship_date": "2026-11-11", "publisher": "DC Comics", "series": "Action Comics",
             "issue": "1104", "status": "Downloaded", "cv_series_id": 91078,
             "cv_issue_id": 1198752, "series_year": 2016},
            {"ship_date": "2026-11-11", "publisher": "Mad Cave Studios",
             "series": "Unmatched Indie", "issue": "3", "status": "Skipped",
             "cv_series_id": null, "cv_issue_id": null, "series_year": 2026}
        ],
        "watchlist": [[4050, "Paused", false], [91078, "Active", true], [130740, "Active", true]],
        "wanted": [
            {"cv_series_id": 91078, "cv_issue_id": 1198999, "series": "Action Comics",
             "issue": "1105", "store_date": "2026-11-18", "status": "Wanted",
             "added_at": "2026-11-12T05:00:00+00:00"}
        ],
        "history": [
            {"cv_series_id": 91078, "cv_issue_id": 1198752, "series": "Action Comics",
             "issue": "1104", "status": "Post-Processed", "at": "2026-11-13T03:53:31+00:00"}
        ],
        "queue": {"counts": {"Queued": 1, "Failed": 546},
                  "items": [{"title": "X-Factor #2", "year": "2024", "size": "67 MB",
                             "status": "Queued", "updated_at": "2026-11-06T14:31:00+00:00"}]},
        "jobs": [{"name": "Weekly Pullist", "status": "Waiting",
                  "prev_run": "2026-11-15T02:02:19+00:00", "next_run": "2026-11-15T06:02:19+00:00"}]
    })
}

async fn count<E: EntityTrait>(app: &TestApp, entity: E) -> u64
where
    E::Model: Send + Sync + sea_orm::FromQueryResult,
{
    let _ = entity;
    E::find().count(&app.state().db).await.unwrap()
}

async fn header_row(app: &TestApp) -> Option<mylar_snapshot::Model> {
    mylar_snapshot::Entity::find_by_id(mylar_snapshot::SINGLETON_ID)
        .one(&app.state().db)
        .await
        .unwrap()
}

// ────────────────────── who may push ──────────────────────

#[tokio::test]
async fn the_integration_off_refuses_even_the_right_token() {
    let (app, _admin, token) = ready().await;

    for (upcoming, mylar) in [(false, false), (true, false), (false, true)] {
        set_enabled(&app, upcoming, mylar);
        let (status, body) = push(&app, &token, &snapshot()).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "upcoming={upcoming} mylar={mylar}: {body}"
        );
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("turned off"),
            "{body}"
        );
    }
    assert!(header_row(&app).await.is_none(), "nothing was stored");
}

#[tokio::test]
async fn a_missing_or_wrong_token_is_refused_before_the_body_is_read() {
    let (app, _admin, token) = ready().await;
    // Not JSON at all: a caller that got as far as the body parser would
    // see 400, not 401.
    let junk = b"this is not json".to_vec();

    // No Authorization header at all: a cookie-less POST without a Bearer
    // is stopped by the CSRF middleware, earlier still.
    let (status, body, _) = push_raw(&app, None, junk.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "auth.csrf", "{body}");

    // A Bearer that is not the token reaches the ingest's own check.
    let (status, body, challenge) = push_raw(&app, Some("fmi_nope"), junk.clone()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(challenge.as_deref(), Some("Bearer"));
    assert_eq!(
        body["error"]["code"], "auth.invalid",
        "house envelope: {body}"
    );

    let mut wrong = token.clone();
    wrong.pop();
    wrong.push(if token.ends_with('a') { 'b' } else { 'a' });
    for bad in [wrong.as_str(), "fmi_", "not-a-folio-token", "app_abcdefgh"] {
        let (status, body, _) = push_raw(&app, Some(bad), junk.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{bad}: {body}");
    }

    // The right token reaches the parser, which then objects to the body.
    let (status, _, _) = push_raw(&app, Some(&token), junk).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(header_row(&app).await.is_none());
}

// ────────────────────── storing ──────────────────────

#[tokio::test]
async fn a_snapshot_is_stored_and_the_next_one_replaces_it() {
    let (app, _admin, token) = ready().await;
    let db = &app.state().db;

    let (status, body) = push(&app, &token, &snapshot()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let pull = mylar_pull::Entity::find()
        .order_by_asc(mylar_pull::Column::SeriesName)
        .all(db)
        .await
        .unwrap();
    assert_eq!(pull.len(), 2);
    assert_eq!(pull[0].series_name, "Action Comics");
    assert_eq!(pull[0].cv_series_id, Some(91078));
    assert_eq!(pull[0].cv_issue_id, Some(1198752));
    assert_eq!(pull[0].status, "Downloaded");
    assert_eq!(
        pull[1].cv_series_id, None,
        "a line the pull service left unmatched"
    );
    assert_eq!(count(&app, mylar_watch::Entity).await, 3);
    let paused = mylar_watch::Entity::find_by_id(4050_i64)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(paused.status, "Paused");
    assert!(!paused.continuing);
    let wanted = mylar_wanted::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(wanted.issue_number, "1105");
    assert_eq!(wanted.status, "Wanted");

    let head = header_row(&app).await.expect("header row");
    assert_eq!(head.schema_version, 1);
    assert_eq!(
        (head.pull_count, head.watch_count, head.wanted_count),
        (2, 3, 1)
    );
    assert_eq!(head.queue["counts"]["Failed"], 546);
    assert_eq!(head.jobs[0]["name"], "Weekly Pullist");
    assert_eq!(head.history[0]["status"], "Post-Processed");

    let used = integration_token::Entity::find_by_id(integration_token::MYLAR.to_owned())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(used.last_used_at.is_some(), "the push stamped the token");

    // A different, smaller snapshot replaces every block.
    let mut next = snapshot();
    next["pull_list"] = json!([next["pull_list"][0].clone()]);
    next["pull_list"][0]["status"] = json!("Wanted");
    next["watchlist"] = json!([[91078, "Active", true]]);
    next["wanted"] = json!([]);
    let (status, body) = push(&app, &token, &next).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(count(&app, mylar_pull::Entity).await, 1);
    assert_eq!(count(&app, mylar_watch::Entity).await, 1);
    assert_eq!(count(&app, mylar_wanted::Entity).await, 0);
    let head = header_row(&app).await.unwrap();
    assert_eq!(
        (head.pull_count, head.watch_count, head.wanted_count),
        (1, 1, 0)
    );
}

#[tokio::test]
async fn an_unchanged_snapshot_only_moves_the_clocks() {
    let (app, _admin, token) = ready().await;
    let db = &app.state().db;
    assert_eq!(
        push(&app, &token, &snapshot()).await.0,
        StatusCode::NO_CONTENT
    );

    let ids_before: Vec<i64> = mylar_pull::Entity::find()
        .order_by_asc(mylar_pull::Column::Id)
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    // Pretend the first push was two hours ago.
    let two_hours_ago = (Utc::now() - Duration::hours(2)).fixed_offset();
    let mut aged = header_row(&app).await.unwrap().into_active_model();
    aged.received_at = Set(two_hours_ago);
    aged.update(db).await.unwrap();

    let mut heartbeat = snapshot();
    heartbeat["meta"]["generated_at"] = json!("2026-11-15T04:36:49+00:00");
    assert_eq!(
        push(&app, &token, &heartbeat).await.0,
        StatusCode::NO_CONTENT
    );

    let head = header_row(&app).await.unwrap();
    assert!(head.received_at > two_hours_ago, "received_at advanced");
    assert_eq!(
        head.generated_at.to_rfc3339(),
        "2026-11-15T04:36:49+00:00",
        "generated_at follows the heartbeat"
    );
    let ids_after: Vec<i64> = mylar_pull::Entity::find()
        .order_by_asc(mylar_pull::Column::Id)
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids_before, ids_after, "rows were not rewritten");
}

// ────────────────────── refusing bad bodies ──────────────────────

#[tokio::test]
async fn bad_snapshots_are_refused_and_store_nothing() {
    let (app, _admin, token) = ready().await;

    let mut newer = snapshot();
    newer["meta"]["schema_version"] = json!(2);
    let (status, body) = push(&app, &token, &newer).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.contains("version 2 is not supported"), "{message}");
    assert!(message.contains("accepts version 1"), "{message}");

    let mut negative = snapshot();
    negative["pull_list"][0]["cv_series_id"] = json!(-5);
    let (status, body) = push(&app, &token, &negative).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    let mut too_many = snapshot();
    too_many["jobs"] = Value::Array(
        (0..51)
            .map(|i| json!({"name": format!("job {i}")}))
            .collect(),
    );
    let (status, body) = push(&app, &token, &too_many).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    let mut bad_date = snapshot();
    bad_date["pull_list"][0]["ship_date"] = json!("next Wednesday");
    let (status, _) = push(&app, &token, &bad_date).await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "an unparseable date is refused: {status}"
    );

    assert!(header_row(&app).await.is_none());
}

#[tokio::test]
async fn an_oversized_body_gets_the_413_envelope() {
    let (app, _admin, token) = ready().await;
    // Over the 2 MiB group default.
    let mut huge = snapshot();
    huge["pull_list"][0]["publisher"] = json!("x".repeat(3 * 1024 * 1024));
    let (status, body) = push(&app, &token, &huge).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["error"]["code"], "too_large", "{body}");
}

// ────────────────────── the token ──────────────────────

#[tokio::test]
async fn token_endpoints_are_admin_only() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    for m in [Method::POST, Method::DELETE] {
        let (status, body) = as_user(&app, m, TOKEN, &reader).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    }
    assert_eq!(count(&app, integration_token::Entity).await, 0);
}

#[tokio::test]
async fn replacing_or_revoking_the_token_cuts_the_old_one_off() {
    let (app, admin, first) = ready().await;
    assert!(first.starts_with("fmi_"));
    assert_eq!(
        push(&app, &first, &snapshot()).await.0,
        StatusCode::NO_CONTENT
    );

    // Only a digest is stored, never the token.
    let row = integration_token::Entity::find_by_id(integration_token::MYLAR.to_owned())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.digest.len(), 32);
    assert!(!row.digest.windows(4).any(|w| w == b"fmi_"));

    let second = new_token(&app, &admin).await;
    assert_ne!(first, second);
    assert_eq!(
        push(&app, &first, &snapshot()).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        push(&app, &second, &snapshot()).await.0,
        StatusCode::NO_CONTENT
    );

    // Revoke without purge: the token is gone, the snapshot stays.
    let (status, body) = as_user(&app, Method::DELETE, TOKEN, &admin).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        push(&app, &second, &snapshot()).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(header_row(&app).await.is_some());
    assert_eq!(count(&app, mylar_watch::Entity).await, 3);

    // Revoke with purge deletes the stored snapshot too.
    let (status, _) = as_user(&app, Method::DELETE, &format!("{TOKEN}?purge=true"), &admin).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(header_row(&app).await.is_none());
    assert_eq!(count(&app, mylar_watch::Entity).await, 0);
    assert_eq!(count(&app, mylar_pull::Entity).await, 0);
    assert_eq!(count(&app, mylar_wanted::Entity).await, 0);

    let audited = |action: &'static str| {
        let db = app.state().db.clone();
        async move {
            entity::audit_log::Entity::find()
                .filter(entity::audit_log::Column::Action.eq(action))
                .count(&db)
                .await
                .unwrap()
        }
    };
    assert_eq!(audited("admin.integration.mylar.token.create").await, 2);
    assert_eq!(audited("admin.integration.mylar.token.revoke").await, 2);
}

// ────────────────────── status ──────────────────────

#[tokio::test]
async fn status_reports_the_token_and_the_snapshots_freshness() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    set_enabled(&app, true, true);

    let (status, body) = as_user(&app, Method::GET, STATUS, &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mylar"]["enabled"], true);
    assert_eq!(body["mylar"]["stale_after_minutes"], 180);
    assert!(body["mylar"]["token"].is_null());
    assert!(body["mylar"]["snapshot"].is_null());

    let token = new_token(&app, &admin).await;
    assert_eq!(
        push(&app, &token, &snapshot()).await.0,
        StatusCode::NO_CONTENT
    );

    let (_, body) = as_user(&app, Method::GET, STATUS, &admin).await;
    let mylar = &body["mylar"];
    assert!(mylar["token"]["created_at"].is_string());
    assert!(mylar["token"]["last_used_at"].is_string());
    assert!(
        !body.to_string().contains(&token),
        "the token never comes back"
    );
    let snap = &mylar["snapshot"];
    assert_eq!(snap["freshness"], "fresh");
    assert_eq!(snap["pull_count"], 2);
    assert_eq!(snap["watch_count"], 3);
    assert_eq!(snap["wanted_count"], 1);
    assert_eq!(snap["queue_counts"], json!({"Failed": 546, "Queued": 1}));

    // Freshness follows when Folio received it, not what the sender says.
    for (age, expected) in [
        (Duration::hours(4), "stale"),
        (Duration::days(8), "expired"),
    ] {
        let mut aged = header_row(&app).await.unwrap().into_active_model();
        aged.received_at = Set((Utc::now() - age).fixed_offset());
        aged.update(&app.state().db).await.unwrap();
        let (_, body) = as_user(&app, Method::GET, STATUS, &admin).await;
        assert_eq!(body["mylar"]["snapshot"]["freshness"], expected);
    }
}
