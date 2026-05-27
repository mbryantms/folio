//! metadata-providers-1.0 M7 — bulk-refresh scope resolver +
//! /libraries/{slug}/metadata/refresh endpoint.
//!
//! The cron itself isn't exercised here (tokio_cron_scheduler would
//! need a fake clock and is covered by manual integration); the
//! refresh-module helpers `eligible_series_for_scope` and the API
//! handler are the load-bearing pieces and get full coverage.

mod common;

use chrono::{Duration, Utc};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{external_id, series};
use sea_orm::{ActiveModelTrait, ConnectionTrait, EntityTrait, Set, Statement};
use server::metadata::refresh::{RefreshScope, eligible_series_for_scope};
use tempfile::tempdir;

/// Seed a single issue under `series_id` then backdate its
/// `created_at` so the recent-window test can drive both branches.
async fn seed_issue_at(
    app: &TestApp,
    lib: uuid::Uuid,
    series_id: uuid::Uuid,
    dir: &std::path::Path,
    name: &str,
    days_ago: i64,
) -> String {
    let file = dir.join(name);
    // Unique per call so the BLAKE3 content-hash (which doubles as
    // issues.id) doesn't collide across the two seedings in a single
    // test.
    let bytes = format!("fake-cbz-payload-{name}-{days_ago}").into_bytes();
    let id = IssueSeed::new(lib, series_id, &file, &bytes, 1.0)
        .insert(&app.state().db)
        .await;
    if days_ago > 0 {
        let when = (Utc::now() - Duration::days(days_ago)).fixed_offset();
        app.state()
            .db
            .execute(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "UPDATE issues SET created_at = $1 WHERE id = $2",
                [when.into(), id.clone().into()],
            ))
            .await
            .unwrap();
    }
    id
}

async fn mark_synced(app: &TestApp, series_id: uuid::Uuid, days_ago: i64) {
    let db = &app.state().db;
    let when = (Utc::now() - Duration::days(days_ago)).fixed_offset();
    let row = series::Entity::find_by_id(series_id).one(db).await.unwrap().unwrap();
    let mut am: series::ActiveModel = row.into();
    am.last_metadata_sync_at = Set(Some(when));
    am.update(db).await.unwrap();
}

async fn mark_paused(app: &TestApp, series_id: uuid::Uuid) {
    let db = &app.state().db;
    let row = series::Entity::find_by_id(series_id).one(db).await.unwrap().unwrap();
    let mut am: series::ActiveModel = row.into();
    am.metadata_sync_paused = Set(true);
    am.update(db).await.unwrap();
}

async fn give_external_id(app: &TestApp, series_id: uuid::Uuid, source: &str, id: &str) {
    let now = Utc::now().fixed_offset();
    external_id::ActiveModel {
        entity_type: Set("series".into()),
        entity_id: Set(series_id.to_string()),
        source: Set(source.into()),
        external_id: Set(id.into()),
        external_url: Set(None),
        set_by: Set("comicvine".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

#[tokio::test]
async fn unmatched_scope_includes_only_series_with_no_external_ids() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let matched = SeriesSeed::new(lib, "Matched").insert(&app.state().db).await;
    let unmatched = SeriesSeed::new(lib, "Unmatched").insert(&app.state().db).await;
    give_external_id(&app, matched, "comicvine", "1").await;

    let ids = eligible_series_for_scope(&app.state().db, lib, RefreshScope::Unmatched, 180, 14)
        .await
        .unwrap();
    assert_eq!(ids, vec![unmatched]);
}

#[tokio::test]
async fn unmatched_scope_excludes_paused() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let unmatched = SeriesSeed::new(lib, "Unmatched").insert(&app.state().db).await;
    let paused = SeriesSeed::new(lib, "Paused").insert(&app.state().db).await;
    mark_paused(&app, paused).await;
    let ids = eligible_series_for_scope(&app.state().db, lib, RefreshScope::Unmatched, 180, 14)
        .await
        .unwrap();
    assert_eq!(ids, vec![unmatched]);
}

#[tokio::test]
async fn stale_scope_includes_never_synced_and_older_than_threshold() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let never_synced = SeriesSeed::new(lib, "Never synced").insert(&app.state().db).await;
    let fresh = SeriesSeed::new(lib, "Fresh").insert(&app.state().db).await;
    let stale = SeriesSeed::new(lib, "Stale").insert(&app.state().db).await;
    // Give all three an external id so the "unmatched" branch doesn't
    // dominate — we want to verify the staleness-OR alone here.
    give_external_id(&app, never_synced, "comicvine", "1").await;
    give_external_id(&app, fresh, "comicvine", "2").await;
    give_external_id(&app, stale, "comicvine", "3").await;
    mark_synced(&app, fresh, 30).await; // within threshold (default 180)
    mark_synced(&app, stale, 200).await; // older than threshold

    let ids = eligible_series_for_scope(&app.state().db, lib, RefreshScope::Stale, 180, 14)
        .await
        .unwrap();
    // Order is created_at ASC; never_synced created first, then stale.
    // fresh excluded.
    assert!(ids.contains(&never_synced));
    assert!(ids.contains(&stale));
    assert!(!ids.contains(&fresh));
}

#[tokio::test]
async fn all_scope_returns_every_active_non_paused_series() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let a = SeriesSeed::new(lib, "A").insert(&app.state().db).await;
    let b = SeriesSeed::new(lib, "B").insert(&app.state().db).await;
    let paused = SeriesSeed::new(lib, "Paused").insert(&app.state().db).await;
    mark_paused(&app, paused).await;
    let ids = eligible_series_for_scope(&app.state().db, lib, RefreshScope::All, 180, 14)
        .await
        .unwrap();
    assert!(ids.contains(&a));
    assert!(ids.contains(&b));
    assert!(!ids.contains(&paused));
}

#[tokio::test]
async fn recent_scope_uses_latest_issue_created_at_window() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let recent = SeriesSeed::new(lib, "Recent").insert(&app.state().db).await;
    let older = SeriesSeed::new(lib, "Older").insert(&app.state().db).await;
    // Recent: issue created today. Older: issue created 30 days ago.
    let _ = seed_issue_at(&app, lib, recent, dir.path(), "recent.cbz", 0).await;
    let _ = seed_issue_at(&app, lib, older, dir.path(), "older.cbz", 30).await;

    let ids = eligible_series_for_scope(&app.state().db, lib, RefreshScope::Recent, 180, 14)
        .await
        .unwrap();
    assert!(ids.contains(&recent), "issue inside window must match");
    assert!(!ids.contains(&older), "issue older than window must not match");
}

#[tokio::test]
async fn recent_scope_falls_back_to_series_created_at_when_no_issues() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&app.state().db).await;
    let s = SeriesSeed::new(lib, "Fresh series").insert(&app.state().db).await;
    // No issues at all; the COALESCE falls back to series.created_at
    // which is `now`, so the series is included in the recent window.
    let ids = eligible_series_for_scope(&app.state().db, lib, RefreshScope::Recent, 180, 14)
        .await
        .unwrap();
    assert!(ids.contains(&s));
}
