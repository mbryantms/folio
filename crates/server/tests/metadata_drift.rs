//! M6 of `metadata-sidecar-writeback-1.0`: admin-only user-edit drift
//! surfacing.
//!
//! Asserts:
//!   - Planting a `field_provenance.set_by='user'` row in a writeback
//!     library makes `GET /libraries/{slug}/health-issues` synthesize a
//!     `MetadataDriftFromXml` row (Q3 from the plan).
//!   - Bumping `issue.last_rewrite_at` past the pin's `set_at` clears
//!     the synth row — same effect a real apply-via-sidecar pass would
//!     have (Q4 R: a end-to-end).
//!   - Libraries without writeback enabled never get the synth row.
//!   - `POST /libraries/{slug}/metadata-drift/flush` enqueues one
//!     rewrite job per drifted issue and emits an audit row.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{audit_log, field_provenance, issue};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use std::io::{Cursor, Write};
use tempfile::tempdir;
use tower::ServiceExt;

fn build_cbz_bytes(label: &str) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("page-001.png", opts).unwrap();
        zw.write_all(label.as_bytes()).unwrap();
        zw.finish().unwrap();
    }
    buf.into_inner()
}

struct Authed {
    session: String,
    csrf: String,
}

async fn register_admin(app: &TestApp) -> Authed {
    // First registered user becomes admin — matches the prod seed path.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"drift-admin@example.com","password":"correctly-horse-battery"}"#,
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

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn list_health(app: &TestApp, auth: &Authed, lib_slug: &str) -> serde_json::Value {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/libraries/{lib_slug}/health-issues"))
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={}", auth.session),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // The per-library list is paginated now (`{items, next_cursor, counts}`);
    // these tests assert against the row array.
    let body = body_json(resp.into_body()).await;
    body.get("items")
        .cloned()
        .expect("items array in health-issues response")
}

async fn plant_user_pin(app: &TestApp, issue_id: &str, field: &str) {
    field_provenance::ActiveModel {
        entity_type: Set("issue".into()),
        entity_id: Set(issue_id.into()),
        field: Set(field.into()),
        set_by: Set("user".into()),
        source_external_id: Set(None),
        set_at: Set(Utc::now().fixed_offset()),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

#[tokio::test]
async fn drift_row_synthesized_when_pin_postdates_last_rewrite() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();

    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let cbz = dir.path().join("saga-1.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
        .insert(&app.state().db)
        .await;

    // Baseline: no pin → no synth row.
    let baseline = list_health(&app, &auth, &lib_id.to_string()).await;
    let baseline_arr = baseline.as_array().unwrap();
    assert!(
        baseline_arr
            .iter()
            .all(|r| r["kind"] != "MetadataDriftFromXml"),
        "no pins → no synth row: {baseline:?}",
    );

    // Plant a user pin on `title` — issue.last_rewrite_at is NULL from
    // seed, so the pin is definitively newer than the latest sidecar
    // write (which never happened).
    plant_user_pin(&app, &issue_id, "title").await;

    let with_drift = list_health(&app, &auth, &lib_id.to_string()).await;
    let arr = with_drift.as_array().unwrap();
    let synth = arr
        .iter()
        .find(|r| r["kind"] == "MetadataDriftFromXml")
        .expect("synth row should appear once a user pin exists");
    assert_eq!(synth["severity"], "info");
    assert_eq!(synth["id"], "synth:metadata_drift_from_xml");
    assert_eq!(synth["payload"]["drifted_issue_count"].as_u64().unwrap(), 1,);
    assert_eq!(
        synth["payload"]["drifted_series_count"].as_u64().unwrap(),
        1,
    );
    assert_eq!(
        synth["payload"]["affected_series_ids"]
            .as_array()
            .unwrap()
            .len(),
        1,
    );
    assert_eq!(synth["resolved_at"], serde_json::Value::Null);
    assert_eq!(synth["dismissed_at"], serde_json::Value::Null);
}

#[tokio::test]
async fn drift_row_clears_when_rewrite_postdates_pin() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();

    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let cbz = dir.path().join("saga-1.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
        .insert(&app.state().db)
        .await;

    // Pin → drift → assert synth row.
    plant_user_pin(&app, &issue_id, "title").await;
    let arr_before = list_health(&app, &auth, &lib_id.to_string()).await;
    assert!(
        arr_before
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == "MetadataDriftFromXml"),
        "synth row expected after pin",
    );

    // Simulate a successful sidecar rewrite that included the pin: bump
    // `issue.last_rewrite_at` to NOW + 5s so it cleanly post-dates the
    // pin row inserted above.
    let row = issue::Entity::find_by_id(&issue_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let mut am: issue::ActiveModel = row.into();
    am.last_rewrite_at = Set(Some(
        Utc::now().fixed_offset() + chrono::Duration::seconds(5),
    ));
    am.last_rewrite_kind = Set(Some("sidecar".into()));
    am.update(&app.state().db).await.unwrap();

    let arr_after = list_health(&app, &auth, &lib_id.to_string()).await;
    assert!(
        arr_after
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["kind"] != "MetadataDriftFromXml"),
        "drift clears once last_rewrite_at > pin.set_at: {arr_after:?}",
    );
}

#[tokio::test]
async fn drift_row_never_synthesized_when_writeback_disabled() {
    // With writeback off the DB is canonical — "drift from XML" doesn't
    // apply, and surfacing the row would confuse the operator into
    // running a flush that does nothing.
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();

    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let cbz = dir.path().join("saga-1.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
        .insert(&app.state().db)
        .await;
    plant_user_pin(&app, &issue_id, "title").await;

    let arr = list_health(&app, &auth, &lib_id.to_string()).await;
    assert!(
        arr.as_array()
            .unwrap()
            .iter()
            .all(|r| r["kind"] != "MetadataDriftFromXml"),
        "writeback-off libraries never surface drift",
    );
}

#[tokio::test]
async fn flush_endpoint_enqueues_rewrite_jobs_and_audits() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();

    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga")
        .insert(&app.state().db)
        .await;
    let cbz = dir.path().join("saga-1.cbz");
    let issue_id = IssueSeed::new(lib_id, series_id, &cbz, &build_cbz_bytes("saga-1"), 1.0)
        .insert(&app.state().db)
        .await;
    plant_user_pin(&app, &issue_id, "title").await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/libraries/{lib_id}/metadata-drift/flush"))
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["enqueued_rewrites"].as_u64().unwrap(), 1);
    assert_eq!(body["skipped"].as_u64().unwrap(), 0);

    // Audit row should land with the flush action + a payload reflecting
    // the drift summary.
    let audits = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("admin.library.metadata_drift.flush"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(audits.len(), 1, "exactly one flush audit row expected");
    let payload = &audits[0].payload;
    assert_eq!(payload["enqueued_rewrites"].as_u64().unwrap(), 1);
    assert_eq!(payload["drifted_issue_count"].as_u64().unwrap(), 1);
}

#[tokio::test]
async fn flush_endpoint_409s_when_writeback_disabled() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempdir().unwrap();

    // No `with_sidecar_writeback()` — both library flags stay false.
    let lib_id = LibrarySeed::new(dir.path()).insert(&app.state().db).await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/libraries/{lib_id}/metadata-drift/flush"))
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "writeback_disabled");
}
