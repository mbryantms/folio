//! Audit 2026-11 N-M9 / DI-17 — a scan that creates series can queue a
//! provider search for each one (per-library `auto_search_new_series`,
//! default off).
//!
//! The test harness runs no apalis workers, so the queued
//! `SearchSeriesJob`s never execute: the assertions read the
//! `metadata_run` rows `enqueue_series_search` writes (the same rows the
//! manual "Match series" button writes), keyed by `trigger_kind`.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{audit_log, external_id, library, metadata_run, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use server::library::scanner::{self, auto_search};
use server::metadata::orchestrator::trigger_kind;
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use tower::ServiceExt;
use uuid::Uuid;

fn write_cbz(path: &Path, marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    // Real PNG signature (the readers content-sniff pages), then a marker
    // so every fixture hashes differently.
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.finish().unwrap();
}

/// Two series folders, one issue each.
fn two_series_root() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    for (i, name) in ["Alpha Run (2020)", "Beta Run (2021)"].iter().enumerate() {
        let dir = tmp.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        write_cbz(&dir.join(format!("{name} 001.cbz")), i as u32 + 1);
    }
    tmp
}

async fn scanner_runs(app: &TestApp) -> Vec<metadata_run::Model> {
    metadata_run::Entity::find()
        .filter(metadata_run::Column::TriggerKind.eq(trigger_kind::SCANNER))
        .all(&app.state().db)
        .await
        .unwrap()
}

async fn series_ids(app: &TestApp, lib: Uuid) -> HashSet<String> {
    series::Entity::find()
        .filter(series::Column::LibraryId.eq(lib))
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.id.to_string())
        .collect()
}

async fn library_row(app: &TestApp, lib: Uuid) -> library::Model {
    library::Entity::find_by_id(lib)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn scan_creating_two_series_queues_two_scanner_searches_and_rescan_queues_none() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let root = two_series_root();
    let lib = LibrarySeed::new(root.path())
        .with_auto_search_new_series()
        .insert(&app.state().db)
        .await;

    let stats = scanner::scan_library(&app.state(), lib).await.unwrap();
    assert_eq!(stats.series_created, 2);
    assert_eq!(stats.created_series_ids.len(), 2);

    let runs = scanner_runs(&app).await;
    assert_eq!(runs.len(), 2, "one scanner search per created series");
    let queued_for: HashSet<String> = runs
        .iter()
        .map(|r| r.scope_entity_id.clone().unwrap())
        .collect();
    assert_eq!(queued_for, series_ids(&app, lib).await);
    for r in &runs {
        assert_eq!(r.scope, "series");
        assert_eq!(r.library_id, Some(lib));
        assert_eq!(r.triggered_by, None);
        assert_eq!(r.status, "queued");
    }

    // A rescan that creates nothing queues nothing — not even for series
    // whose earlier search has since finished.
    for r in runs {
        let mut am: metadata_run::ActiveModel = r.into();
        am.status = sea_orm::Set("completed".into());
        sea_orm::ActiveModelTrait::update(am, &app.state().db)
            .await
            .unwrap();
    }
    let stats = scanner::scan_library_with(&app.state(), lib, true)
        .await
        .unwrap();
    assert_eq!(stats.series_created, 0);
    assert!(stats.created_series_ids.is_empty());
    assert_eq!(scanner_runs(&app).await.len(), 2);
}

#[tokio::test]
async fn scan_with_flag_off_queues_nothing() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let root = two_series_root();
    let lib = LibrarySeed::new(root.path()).insert(&app.state().db).await;

    let stats = scanner::scan_library(&app.state(), lib).await.unwrap();
    assert_eq!(stats.series_created, 2);
    let all_runs = metadata_run::Entity::find()
        .all(&app.state().db)
        .await
        .unwrap();
    assert!(all_runs.is_empty(), "default-off library queues no search");
}

/// A series with one active issue, so it is eligible for a scan search.
async fn series_with_issue(app: &TestApp, lib: Uuid, dir: &Path, name: &str) -> Uuid {
    let id = SeriesSeed::new(lib, name).insert(&app.state().db).await;
    let file = dir.join(format!("{name}.cbz"));
    let payload = format!("payload-{name}").into_bytes();
    IssueSeed::new(lib, id, &file, &payload, 1.0)
        .insert(&app.state().db)
        .await;
    id
}

async fn auto_search_lib(app: &TestApp, dir: &Path) -> Uuid {
    LibrarySeed::new(dir)
        .with_auto_search_new_series()
        .insert(&app.state().db)
        .await
}

#[tokio::test]
async fn series_with_a_search_already_queued_is_not_queued_again() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempfile::tempdir().unwrap();
    let lib = auto_search_lib(&app, dir.path()).await;
    let busy = series_with_issue(&app, lib, dir.path(), "Busy").await;
    let fresh = series_with_issue(&app, lib, dir.path(), "Fresh").await;

    // A manual "Match series" click got there first.
    let manual = server::jobs::metadata_search::enqueue_series_search(
        &app.state(),
        busy,
        None,
        trigger_kind::MANUAL,
        None,
    )
    .await
    .unwrap();
    assert!(!manual.coalesced);

    let out =
        auto_search::enqueue_for_created_series(&app.state(), lib, &[busy, fresh, busy]).await;
    assert_eq!(out.enqueued, 1);
    assert_eq!(out.skipped, 1, "the already-queued series is skipped");
    assert_eq!(out.deferred, 0);

    let runs = scanner_runs(&app).await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].scope_entity_id, Some(fresh.to_string()));
    let busy_runs = metadata_run::Entity::find()
        .filter(metadata_run::Column::ScopeEntityId.eq(busy.to_string()))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(busy_runs.len(), 1, "only the manual run exists for Busy");
    assert_eq!(busy_runs[0].id, manual.run_id);
}

#[tokio::test]
async fn matched_or_empty_series_are_skipped() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempfile::tempdir().unwrap();
    let lib = auto_search_lib(&app, dir.path()).await;
    // Identified at scan time (folder tag / sidecar id).
    let tagged = series_with_issue(&app, lib, dir.path(), "Tagged").await;
    let now = chrono::Utc::now().fixed_offset();
    sea_orm::ActiveModelTrait::insert(
        external_id::ActiveModel {
            entity_type: sea_orm::Set("series".into()),
            entity_id: sea_orm::Set(tagged.to_string()),
            source: sea_orm::Set("comicvine".into()),
            external_id: sea_orm::Set("4050-1".into()),
            external_url: sea_orm::Set(None),
            set_by: sea_orm::Set("scanner_folder_tag".into()),
            first_set_at: sea_orm::Set(now),
            last_synced_at: sea_orm::Set(now),
        },
        &app.state().db,
    )
    .await
    .unwrap();
    // A folder whose archives all failed to ingest.
    let empty = SeriesSeed::new(lib, "Empty").insert(&app.state().db).await;
    let plain = series_with_issue(&app, lib, dir.path(), "Plain").await;

    let out =
        auto_search::enqueue_for_created_series(&app.state(), lib, &[tagged, empty, plain]).await;
    assert_eq!(out.enqueued, 1);
    assert_eq!(out.skipped, 2);
    let runs = scanner_runs(&app).await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].scope_entity_id, Some(plain.to_string()));
}

#[tokio::test]
async fn rolling_budget_holds_across_scans_and_defers_the_rest() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempfile::tempdir().unwrap();
    let lib = auto_search_lib(&app, dir.path()).await;
    let mut ids = Vec::new();
    for name in ["One", "Two", "Three", "Four", "Five"] {
        ids.push(series_with_issue(&app, lib, dir.path(), name).await);
    }

    // First scan: budget 2 of 4 created.
    let out = auto_search::enqueue_for_created_series_capped(&app.state(), lib, &ids[..4], 2).await;
    assert_eq!(out.enqueued, 2);
    assert_eq!(out.deferred, 2);
    // The first two in creation order were queued.
    let queued: HashSet<String> = scanner_runs(&app)
        .await
        .into_iter()
        .map(|r| r.scope_entity_id.unwrap())
        .collect();
    let expected: HashSet<String> = ids[..2].iter().map(Uuid::to_string).collect();
    assert_eq!(queued, expected);

    // A second (e.g. file-watcher) scan inside the window gets no budget.
    let out = auto_search::enqueue_for_created_series_capped(&app.state(), lib, &ids[4..], 2).await;
    assert_eq!(out.enqueued, 0);
    assert_eq!(out.deferred, 1);
    assert_eq!(scanner_runs(&app).await.len(), 2);

    // The production budget is the documented 50 per 24 h.
    assert_eq!(auto_search::SCAN_AUTO_SEARCH_CAP, 50);
    assert_eq!(auto_search::SCAN_AUTO_SEARCH_WINDOW_HOURS, 24);
}

#[tokio::test]
async fn flag_is_read_live_and_no_providers_queues_nothing() {
    let app = TestApp::spawn_with_comicvine("k", true).await;
    let dir = tempfile::tempdir().unwrap();
    let lib = auto_search_lib(&app, dir.path()).await;
    let s = series_with_issue(&app, lib, dir.path(), "Solo").await;
    // Turned off while the scan was running: the scan-start row said on.
    let row = library_row(&app, lib).await;
    assert!(row.auto_search_new_series);
    let mut am: library::ActiveModel = row.into();
    am.auto_search_new_series = sea_orm::Set(false);
    sea_orm::ActiveModelTrait::update(am, &app.state().db)
        .await
        .unwrap();
    let out = auto_search::enqueue_for_created_series(&app.state(), lib, &[s]).await;
    assert_eq!(out, auto_search::AutoSearchOutcome::default());
    assert!(scanner_runs(&app).await.is_empty());

    // No providers configured.
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let lib = auto_search_lib(&app, dir.path()).await;
    let s = series_with_issue(&app, lib, dir.path(), "Solo").await;
    let out = auto_search::enqueue_for_created_series(&app.state(), lib, &[s]).await;
    assert_eq!(out, auto_search::AutoSearchOutcome::default());
    assert!(scanner_runs(&app).await.is_empty());
}

// ───────── settings PATCH ─────────

struct Authed {
    session: String,
    csrf: String,
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
                    r#"{"email":"auto-search@example.com","password":"correctly-horse-battery"}"#,
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

async fn patch_library(
    app: &TestApp,
    auth: &Authed,
    slug: &str,
    patch: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/api/libraries/{slug}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(patch.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let s = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (s, serde_json::from_slice(&bytes).unwrap())
}

async fn flag_audit_rows(app: &TestApp) -> Vec<audit_log::Model> {
    audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("admin.library.auto_search_new_series.set"))
        .all(&app.state().db)
        .await
        .unwrap()
}

#[tokio::test]
async fn settings_patch_toggles_the_flag_and_audits_only_real_flips() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let dir = tempfile::tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let slug = library_row(&app, lib).await.slug;

    let (status, body) = patch_library(
        &app,
        &auth,
        &slug,
        serde_json::json!({ "report_missing_comicinfo": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["auto_search_new_series"], false, "default off");

    let (status, body) = patch_library(
        &app,
        &auth,
        &slug,
        serde_json::json!({ "auto_search_new_series": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["auto_search_new_series"], true);
    assert!(library_row(&app, lib).await.auto_search_new_series);
    let rows = flag_audit_rows(&app).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].payload["enabled"], true);
    assert_eq!(rows[0].target_id.as_deref(), Some(lib.to_string().as_str()));

    // The settings form re-sends every field on save: an unchanged value
    // writes no second row.
    let (status, _) = patch_library(
        &app,
        &auth,
        &slug,
        serde_json::json!({ "auto_search_new_series": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(flag_audit_rows(&app).await.len(), 1);
}
