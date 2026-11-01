//! Audit 2026-11 E1 — `audit_log` export-and-trim retention.
//!
//! Validates:
//!   - the default (`audit_log.retention_days = 0`) keeps every row and
//!     writes no file
//!   - with retention set, rows older than the cutoff are exported to an
//!     NDJSON file whose line count matches, then deleted; recent rows stay
//!   - an unusable export directory leaves the table untouched and the run
//!     reports the error (nothing is deleted without a durable export)
//!   - a second run is idempotent (no rows, no new file); a rerun that
//!     would reuse an existing export name fails instead of overwriting it
//!   - `COMIC_AUDIT_LOG_RETENTION_DAYS` is seeded into `app_setting` once
//!   - the setting goes through the registry chain: `PATCH /admin/settings`
//!     applies it live, rejects a below-floor value with 400, and `0` turns
//!     it back off

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use common::TestApp;
use entity::audit_log;
use sea_orm::{ActiveModelTrait, EntityTrait, PaginatorTrait, Set};
use serde_json::{Value, json};
use server::jobs::audit_log_retention::{self, MIN_RETENTION_DAYS, export_dir};
use std::collections::BTreeSet;
use tower::ServiceExt;
use uuid::Uuid;

/// Insert one audit row stamped `age_days` ago. Tests build the rows
/// directly because `audit::record` always stamps `now()`; production code
/// never writes this table except through that helper.
async fn seed_row(app: &TestApp, actor: Uuid, age_days: i64) -> Uuid {
    let id = Uuid::now_v7();
    audit_log::ActiveModel {
        id: Set(id),
        actor_id: Set(actor),
        actor_type: Set("user".into()),
        action: Set("admin.test.seed".into()),
        target_type: Set(Some("series".into())),
        target_id: Set(Some(format!("age-{age_days}"))),
        payload: Set(json!({ "age_days": age_days })),
        ip: Set(Some("127.0.0.1".into())),
        user_agent: Set(Some("retention-test".into())),
        created_at: Set((Utc::now() - Duration::days(age_days)).fixed_offset()),
    }
    .insert(&app.state().db)
    .await
    .expect("seed audit row");
    id
}

async fn count_rows(app: &TestApp) -> u64 {
    audit_log::Entity::find()
        .count(&app.state().db)
        .await
        .expect("count audit rows")
}

async fn remaining_ids(app: &TestApp) -> BTreeSet<Uuid> {
    audit_log::Entity::find()
        .all(&app.state().db)
        .await
        .expect("list audit rows")
        .into_iter()
        .map(|r| r.id)
        .collect()
}

fn set_retention(app: &TestApp, days: u32) {
    let mut cfg = (*app.state().cfg()).clone();
    cfg.audit_log_retention_days = days;
    app.state().replace_cfg(cfg);
}

fn export_files(app: &TestApp) -> Vec<std::path::PathBuf> {
    let dir = export_dir(&app.state().cfg().data_path);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    files
}

#[tokio::test]
async fn default_setting_keeps_every_row_and_writes_nothing() {
    let app = TestApp::spawn().await;
    assert_eq!(
        app.state().cfg().audit_log_retention_days,
        0,
        "default is keep forever"
    );
    let actor = Uuid::new_v4();
    for age in [400, 200, 1] {
        seed_row(&app, actor, age).await;
    }
    let before = count_rows(&app).await;

    let stats = audit_log_retention::run(&app.state()).await.unwrap();
    assert_eq!(stats, audit_log_retention::RetentionStats::default());
    assert_eq!(count_rows(&app).await, before);
    assert!(
        !export_dir(&app.state().cfg().data_path).exists(),
        "no export directory is created while retention is off"
    );
}

#[tokio::test]
async fn old_rows_are_exported_then_deleted_and_recent_rows_remain() {
    let app = TestApp::spawn().await;
    let actor = Uuid::new_v4();
    let mut old = BTreeSet::new();
    for age in [91, 120, 200, 365, 1000] {
        old.insert(seed_row(&app, actor, age).await);
    }
    let mut recent = BTreeSet::new();
    for age in [0, 30, 89] {
        recent.insert(seed_row(&app, actor, age).await);
    }
    set_retention(&app, 90);

    let stats = audit_log_retention::run(&app.state()).await.unwrap();
    assert_eq!(stats.exported, 5);
    assert_eq!(stats.deleted, 5);
    let file = stats.file.expect("an export file was written");
    assert!(file.starts_with(export_dir(&app.state().cfg().data_path)));
    assert_eq!(
        file.extension().and_then(|e| e.to_str()),
        Some("ndjson"),
        "export is newline-delimited JSON"
    );
    assert!(
        file.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("audit-log-")),
        "export file follows the backup naming scheme: {}",
        file.display()
    );

    // The file holds exactly the deleted rows, one JSON object per line.
    let text = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 5, "line count matches the exported count");
    let exported_ids: BTreeSet<Uuid> = lines
        .iter()
        .map(|l| {
            let v: Value = serde_json::from_str(l).expect("each line is a JSON object");
            assert_eq!(v["action"], "admin.test.seed");
            assert!(v["created_at"].is_string());
            assert!(v["payload"]["age_days"].as_i64().unwrap() > 90);
            Uuid::parse_str(v["id"].as_str().unwrap()).unwrap()
        })
        .collect();
    assert_eq!(exported_ids, old, "every old row and only old rows");

    // The table keeps exactly the recent rows.
    assert_eq!(remaining_ids(&app).await, recent);
    assert_eq!(
        export_files(&app),
        vec![file.clone()],
        "no temp file is left behind"
    );

    // Second run: nothing older than the cutoff is left, so nothing is
    // exported, deleted or written.
    let again = audit_log_retention::run(&app.state()).await.unwrap();
    assert_eq!(again, audit_log_retention::RetentionStats::default());
    assert_eq!(remaining_ids(&app).await, recent);
    assert_eq!(export_files(&app), vec![file], "idempotent: still one file");
}

#[tokio::test]
async fn below_floor_retention_is_raised_to_the_minimum() {
    let app = TestApp::spawn().await;
    let actor = Uuid::new_v4();
    // 50 days old: inside the 90-day floor, so a (mis)configured 10-day
    // retention must not touch it.
    let kept = seed_row(&app, actor, 50).await;
    let trimmed = seed_row(&app, actor, i64::from(MIN_RETENTION_DAYS) + 5).await;

    let stats = audit_log_retention::run_with(
        &app.state().db,
        &app.state().cfg().data_path,
        10,
        Utc::now(),
    )
    .await
    .unwrap();
    assert_eq!(stats.exported, 1);
    assert_eq!(stats.deleted, 1);
    let left = remaining_ids(&app).await;
    assert!(left.contains(&kept));
    assert!(!left.contains(&trimmed));
}

#[tokio::test]
async fn unusable_export_dir_deletes_nothing_and_reports_the_error() {
    let app = TestApp::spawn().await;
    let actor = Uuid::new_v4();
    for age in [100, 200, 300] {
        seed_row(&app, actor, age).await;
    }
    let before = remaining_ids(&app).await;
    // Occupy the export path with a regular file so the directory cannot
    // be created — the portable way to make it unwritable (a chmod 000
    // directory is still writable for root, which CI containers may be).
    let dir = export_dir(&app.state().cfg().data_path);
    std::fs::write(&dir, b"not a directory").unwrap();
    set_retention(&app, 90);

    let err = audit_log_retention::run(&app.state())
        .await
        .expect_err("an unusable export directory fails the run");
    let msg = err.to_string();
    assert!(
        msg.contains("export directory") && msg.contains(&dir.display().to_string()),
        "error names the export directory: {msg}"
    );
    assert_eq!(
        remaining_ids(&app).await,
        before,
        "no row is deleted without a successful export"
    );
    assert_eq!(
        std::fs::read(&dir).unwrap(),
        b"not a directory",
        "the blocking file is left alone"
    );
}

#[tokio::test]
async fn same_stamp_rerun_refuses_to_replace_an_existing_export() {
    let app = TestApp::spawn().await;
    let actor = Uuid::new_v4();
    let first = seed_row(&app, actor, 200).await;
    let now = Utc::now();
    let data_path = app.state().cfg().data_path.clone();

    let stats = audit_log_retention::run_with(&app.state().db, &data_path, 90, now)
        .await
        .unwrap();
    assert_eq!(stats.deleted, 1);
    let file = stats.file.unwrap();
    let first_bytes = std::fs::read(&file).unwrap();

    // A second run with the identical `now` would name the same file.
    let second = seed_row(&app, actor, 300).await;
    let err = audit_log_retention::run_with(&app.state().db, &data_path, 90, now)
        .await
        .expect_err("a colliding export name fails the run");
    assert!(
        err.to_string().contains("already exists"),
        "names the collision: {err}"
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        first_bytes,
        "the earlier export is untouched"
    );
    let left = remaining_ids(&app).await;
    assert!(left.contains(&second), "nothing was deleted");
    assert!(!left.contains(&first));
    assert_eq!(export_files(&app), vec![file], "no temp or claim file left");
}

#[tokio::test]
async fn env_value_is_seeded_into_app_setting_once() {
    use entity::app_setting;
    use sea_orm::{ColumnTrait, QueryFilter};
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let row = || async {
        app_setting::Entity::find()
            .filter(app_setting::Column::Key.eq("audit_log.retention_days"))
            .one(db)
            .await
            .unwrap()
    };

    // Default (0) seeds nothing — the card falls back to 0 anyway.
    let cfg = (*app.state().cfg()).clone();
    server::settings::bootstrap::seed_audit_log_retention_from_env(db, &cfg)
        .await
        .unwrap();
    assert!(row().await.is_none());

    let mut cfg = cfg;
    cfg.audit_log_retention_days = 180;
    server::settings::bootstrap::seed_audit_log_retention_from_env(db, &cfg)
        .await
        .unwrap();
    assert_eq!(row().await.unwrap().value, json!(180));

    // Idempotent: an existing row (even one the operator changed) wins.
    cfg.audit_log_retention_days = 365;
    server::settings::bootstrap::seed_audit_log_retention_from_env(db, &cfg)
        .await
        .unwrap();
    assert_eq!(row().await.unwrap().value, json!(180));
}

// ───── settings chain: PATCH /admin/settings ─────

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
                    r#"{"email":"retention-admin@example.com","password":"correctly-horse-battery"}"#,
                ))
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

async fn patch_settings(app: &TestApp, auth: &Authed, body: Value) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/api/admin/settings")
                .header(header::COOKIE, auth.cookie())
                .header("x-csrf-token", &auth.csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn retention_setting_applies_live_and_enforces_the_floor() {
    let app = TestApp::spawn().await;
    let admin = register_admin(&app).await;
    assert_eq!(app.state().cfg().audit_log_retention_days, 0);

    let resp = patch_settings(&app, &admin, json!({ "audit_log.retention_days": 180 })).await;
    assert_eq!(resp.status(), StatusCode::OK, "180 days is accepted");
    assert_eq!(app.state().cfg().audit_log_retention_days, 180);
    assert!(
        !server::settings::registry::requires_restart("audit_log.retention_days"),
        "retention is live, not restart-gated"
    );

    let resp = patch_settings(&app, &admin, json!({ "audit_log.retention_days": 30 })).await;
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "below the 90-day floor is rejected by dry-run validation"
    );
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "settings.invalid_combination");
    assert_eq!(
        app.state().cfg().audit_log_retention_days,
        180,
        "a rejected PATCH changes nothing"
    );

    let resp = patch_settings(&app, &admin, json!({ "audit_log.retention_days": 0 })).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        app.state().cfg().audit_log_retention_days,
        0,
        "0 turns retention back off"
    );

    // The PATCHes themselves were audited (fresh rows) and a run with the
    // setting off touches none of them.
    let rows = count_rows(&app).await;
    assert!(rows >= 3, "settings PATCHes are audited: {rows}");
    let stats = audit_log_retention::run(&app.state()).await.unwrap();
    assert_eq!(stats.deleted, 0);
    assert_eq!(count_rows(&app).await, rows);
}
