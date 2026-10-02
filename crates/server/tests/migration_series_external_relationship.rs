//! WP-7.8 migration round-trip: `m20270508_000001_series_external_relationship`.
//!
//! The per-test DB is cloned from the fully-migrated template, so the test
//! runs the migration **down** (table, index and reprint columns gone),
//! seeds a pre-WP-7.8 reprint row, runs **up** (the row survives; the
//! CHECKs and the unique key hold), then down and up again.

mod common;

use common::TestApp;
use common::seed::{IssueSeed, SeriesSeed, seed_library};
use migration::MigratorTrait;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, Value};
use uuid::Uuid;

const MIGRATION: &str = "m20270508_000001_series_external_relationship";

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

async fn try_exec(db: &DatabaseConnection, sql: &str, values: Vec<Value>) -> Result<(), String> {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<Value>) {
    try_exec(db, sql, values)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn exists(db: &DatabaseConnection, sql: &str) -> bool {
    db.query_one_raw(Statement::from_string(DbBackend::Postgres, sql))
        .await
        .unwrap()
        .is_some()
}

async fn has_table(db: &DatabaseConnection) -> bool {
    exists(
        db,
        "SELECT 1 FROM information_schema.tables \
          WHERE table_name = 'series_external_relationship'",
    )
    .await
}

async fn has_reprint_columns(db: &DatabaseConnection) -> bool {
    exists(
        db,
        "SELECT 1 FROM information_schema.columns \
          WHERE table_name = 'issue_reprints' AND column_name = 'reprinted_external_id'",
    )
    .await
}

const INSERT: &str = "INSERT INTO series_external_relationship \
   (id, from_series_id, kind, qualifier, source, provider_series_id, set_by, confidence) \
   VALUES ($1, $2, $3, $4, $5, $6, $7, $8)";

#[allow(clippy::too_many_arguments)]
fn row(
    from: Uuid,
    kind: &str,
    qualifier: Option<&str>,
    source: &str,
    pid: &str,
    set_by: &str,
    confidence: Option<f32>,
) -> Vec<Value> {
    vec![
        Uuid::now_v7().into(),
        from.into(),
        kind.into(),
        qualifier.map(str::to_owned).into(),
        source.into(),
        pid.into(),
        set_by.into(),
        confidence.into(),
    ]
}

#[tokio::test]
async fn external_relationship_migration_round_trips() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let a = SeriesSeed::new(lib, "A").insert(&db).await;
    let i = IssueSeed::new(lib, a, &tmp.path().join("a1.cbz"), b"a1", 1.0)
        .insert(&db)
        .await;

    // ── down ──
    migration::Migrator::down(&db, Some(steps_to_undo_ours()))
        .await
        .unwrap();
    assert!(!has_table(&db).await);
    assert!(!has_reprint_columns(&db).await);
    // A pre-WP-7.8 label-only reprint row.
    exec(
        &db,
        "INSERT INTO issue_reprints (id, issue_id, reprinted_label) VALUES ($1, $2, 'X #1')",
        vec![Uuid::now_v7().into(), i.clone().into()],
    )
    .await;

    // ── up ──
    migration::Migrator::up(&db, None).await.unwrap();
    assert!(has_table(&db).await);
    assert!(has_reprint_columns(&db).await);
    assert!(
        exists(
            &db,
            "SELECT 1 FROM issue_reprints WHERE reprinted_label = 'X #1' \
               AND reprinted_external_id IS NULL"
        )
        .await,
        "old reprint rows survive with the new columns NULL"
    );

    exec(
        &db,
        INSERT,
        row(
            a,
            "continued_by",
            Some("relaunch"),
            "metron",
            "903",
            "user",
            None,
        ),
    )
    .await;
    exec(
        &db,
        INSERT,
        row(
            a,
            "see_also",
            None,
            "comicvine",
            "12",
            "provider",
            Some(0.6),
        ),
    )
    .await;
    // Unique (from, kind, source, provider id); the CHECKs.
    for (kind, qualifier, source, pid, set_by, confidence) in [
        ("continued_by", None, "metron", "903", "provider", None), // duplicate key
        ("not_a_kind", None, "metron", "1", "user", None),
        ("see_also", Some("relaunch"), "metron", "2", "user", None), // qualifier on the wrong kind
        ("see_also", None, "marvel", "3", "user", None),
        ("see_also", None, "metron", "4", "someone", None),
        ("see_also", None, "metron", "", "user", None),
        ("see_also", None, "metron", "5", "provider", Some(1.5)),
    ] {
        assert!(
            try_exec(
                &db,
                INSERT,
                row(a, kind, qualifier, source, pid, set_by, confidence)
            )
            .await
            .is_err(),
            "{kind} {qualifier:?} {source} {pid} {set_by} {confidence:?}"
        );
    }
    // FK cascade on series delete is exercised by the down/up below only
    // through the drop; check it directly.
    let b = SeriesSeed::new(lib, "B").insert(&db).await;
    exec(
        &db,
        INSERT,
        row(b, "see_also", None, "gcd", "7", "user", None),
    )
    .await;
    exec(&db, "DELETE FROM series WHERE id = $1", vec![b.into()]).await;
    assert!(
        !exists(
            &db,
            &format!("SELECT 1 FROM series_external_relationship WHERE from_series_id = '{b}'")
        )
        .await
    );

    // ── down again (lossy only for the new table / columns), then up ──
    migration::Migrator::down(&db, Some(steps_to_undo_ours()))
        .await
        .unwrap();
    assert!(!has_table(&db).await);
    assert!(
        exists(
            &db,
            "SELECT 1 FROM issue_reprints WHERE reprinted_label = 'X #1'"
        )
        .await
    );
    migration::Migrator::up(&db, None).await.unwrap();
    assert!(has_table(&db).await);
}
