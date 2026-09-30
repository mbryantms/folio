//! WP-3.7 migration round-trip: `m20270216_000001_retire_user_edited`.
//!
//! The per-test DB is cloned from the fully-migrated template (up), so
//! this test runs the migration **down** (column re-added, repopulated
//! from column-key user pins) and **up** again (list backfilled into
//! `field_provenance`, column dropped), asserting the data at each step.

mod common;

use common::TestApp;
use common::seed::{IssueSeed, SeriesSeed, seed_library};
use migration::MigratorTrait;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use std::collections::BTreeMap;

const MIGRATION: &str = "m20270216_000001_retire_user_edited";

/// How many migrations to roll back so ours is the last one undone.
fn steps_to_undo_ours() -> u32 {
    let names: Vec<String> = migration::Migrator::migrations()
        .iter()
        .map(|m| m.name().to_owned())
        .collect();
    let pos = names
        .iter()
        .position(|n| n == MIGRATION)
        .expect("migration registered");
    u32::try_from(names.len() - pos).unwrap()
}

async fn has_column(db: &DatabaseConnection) -> bool {
    db.query_one_raw(Statement::from_string(
        DbBackend::Postgres,
        "SELECT 1 AS one FROM information_schema.columns \
         WHERE table_name = 'issues' AND column_name = 'user_edited'",
    ))
    .await
    .unwrap()
    .is_some()
}

async fn user_edited(db: &DatabaseConnection, id: &str) -> serde_json::Value {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT user_edited FROM issues WHERE id = $1",
            [id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get::<serde_json::Value>("", "user_edited").unwrap()
}

async fn prov(db: &DatabaseConnection, id: &str) -> BTreeMap<String, String> {
    db.query_all_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT field, set_by FROM field_provenance \
         WHERE entity_type = 'issue' AND entity_id = $1",
        [id.into()],
    ))
    .await
    .unwrap()
    .into_iter()
    .map(|r| {
        (
            r.try_get::<String>("", "field").unwrap(),
            r.try_get::<String>("", "set_by").unwrap(),
        )
    })
    .collect()
}

async fn exec(db: &DatabaseConnection, sql: &str, id: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        [id.into()],
    ))
    .await
    .unwrap();
}

#[tokio::test]
async fn retire_user_edited_round_trips_down_and_up() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = SeriesSeed::new(lib, "Round Trip").insert(&db).await;
    let pa = tmp.path().join("a.cbz");
    let pb = tmp.path().join("b.cbz");
    let a = IssueSeed::new(lib, series, &pa, b"issue-a", 1.0)
        .insert(&db)
        .await;
    let b = IssueSeed::new(lib, series, &pb, b"issue-b", 2.0)
        .insert(&db)
        .await;
    assert!(!has_column(&db).await, "column is gone after up");

    // Issue A: a column pin + a column pin with its rolled-up key.
    for field in ["web_url", "writer", "credits"] {
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO field_provenance (entity_type, entity_id, field, set_by, set_at) \
             VALUES ('issue', $1, $2, 'user', now())",
            [a.clone().into(), field.into()],
        ))
        .await
        .unwrap();
    }
    // Issue B: a provider-set title (no user pin).
    exec(
        &db,
        "INSERT INTO field_provenance (entity_type, entity_id, field, set_by, set_at) \
         VALUES ('issue', $1, 'title', 'comicvine', now())",
        &b,
    )
    .await;

    // ── down: column back, repopulated from column-key user pins ──
    migration::Migrator::down(&db, Some(steps_to_undo_ours()))
        .await
        .unwrap();
    assert!(has_column(&db).await, "down re-adds the column");
    assert_eq!(
        user_edited(&db, &a).await,
        serde_json::json!(["web_url", "writer"]),
        "down lists column keys only (not the rolled-up `credits`)"
    );
    assert_eq!(user_edited(&db, &b).await, serde_json::json!([]));

    // Model data recorded only in the list: wipe A's pins, give B list
    // entries (one over a file-tier row, one over the provider row, and
    // a non-string junk entry).
    exec(
        &db,
        "DELETE FROM field_provenance WHERE entity_type = 'issue' AND entity_id = $1",
        &a,
    )
    .await;
    exec(
        &db,
        "UPDATE issues SET user_edited = '[\"genre\", \"title\", \"sort_number\", 7]'::jsonb \
         WHERE id = $1",
        &b,
    )
    .await;
    exec(
        &db,
        "INSERT INTO field_provenance (entity_type, entity_id, field, set_by, set_at) \
         VALUES ('issue', $1, 'genre', 'comicinfo', now())",
        &b,
    )
    .await;

    // ── up: list backfilled into field_provenance, column dropped ──
    migration::Migrator::up(&db, None).await.unwrap();
    assert!(!has_column(&db).await, "up drops the column");

    let pa = prov(&db, &a).await;
    for field in ["web_url", "writer", "credits"] {
        assert_eq!(pa.get(field).map(String::as_str), Some("user"), "{pa:?}");
    }

    let pb = prov(&db, &b).await;
    assert_eq!(
        pb.get("genre").map(String::as_str),
        Some("user"),
        "a file-tier row is upgraded to the user pin: {pb:?}"
    );
    assert_eq!(pb.get("genres").map(String::as_str), Some("user"), "{pb:?}");
    assert_eq!(
        pb.get("sort_number").map(String::as_str),
        Some("user"),
        "{pb:?}"
    );
    assert_eq!(
        pb.get("title").map(String::as_str),
        Some("comicvine"),
        "a provider row is never downgraded to a stale list entry: {pb:?}"
    );
    assert!(!pb.contains_key("7"), "non-string entries are skipped");
}
