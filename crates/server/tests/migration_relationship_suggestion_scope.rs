//! WP-7.6 migration round-trip: `m20270506_000001_relationship_suggestion_scope`.
//!
//! The per-test DB is cloned from the fully-migrated template (up), so the
//! test runs the migration **down**, seeds an old-shape suggestion, runs
//! **up** and checks the new columns and CHECKs, then seeds an arc target and
//! a scoped row and runs down (lossy: arc rows deleted, scope dropped) and up
//! again.

mod common;

use common::TestApp;
use common::seed::{SeriesSeed, seed_library};
use migration::MigratorTrait;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, Value};
use uuid::Uuid;

const MIGRATION: &str = "m20270506_000001_relationship_suggestion_scope";

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

async fn count(db: &DatabaseConnection, sql: &str) -> i64 {
    db.query_one_raw(Statement::from_string(DbBackend::Postgres, sql))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap()
}

async fn has_column(db: &DatabaseConnection, col: &str) -> bool {
    db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT 1 AS one FROM information_schema.columns \
          WHERE table_name = 'series_relationship_suggestion' AND column_name = $1",
        [col.into()],
    ))
    .await
    .unwrap()
    .is_some()
}

const INSERT_SERIES_ROW: &str = "INSERT INTO series_relationship_suggestion \
   (id, from_series_id, to_series_id, kind, confidence, bucket, reason) \
   VALUES ($1, $2, $3, $4, 0.9, 'high', 'r')";

#[tokio::test]
async fn suggestion_scope_migration_round_trips() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let a = SeriesSeed::new(lib, "A").insert(&db).await;
    let b = SeriesSeed::new(lib, "B").insert(&db).await;
    let c = SeriesSeed::new(lib, "C").insert(&db).await;

    // ── down: the WP-7.5 shape (to_series_id NOT NULL, no scope) ──
    migration::Migrator::down(&db, Some(steps_to_undo_ours()))
        .await
        .unwrap();
    assert!(!has_column(&db, "to_arc_id").await);
    let old = Uuid::now_v7();
    exec(
        &db,
        INSERT_SERIES_ROW,
        vec![old.into(), a.into(), b.into(), "continues".into()],
    )
    .await;

    // ── up ──
    migration::Migrator::up(&db, None).await.unwrap();
    for col in [
        "to_arc_id",
        "from_range",
        "to_range",
        "coverage",
        "qualifier",
    ] {
        assert!(has_column(&db, col).await, "{col}");
    }
    assert_eq!(
        count(
            &db,
            "SELECT count(*) AS n FROM series_relationship_suggestion WHERE to_arc_id IS NULL \
               AND to_series_id IS NOT NULL"
        )
        .await,
        1,
        "the old row survives as a series target"
    );

    let arc = Uuid::now_v7();
    exec(
        &db,
        "INSERT INTO story_arc (id, slug, name, normalized_name) VALUES ($1, 'ev', 'Ev', 'ev')",
        vec![arc.into()],
    )
    .await;
    let arc_row = "INSERT INTO series_relationship_suggestion \
       (id, from_series_id, to_series_id, to_arc_id, kind, confidence, bucket, reason, qualifier, from_range) \
       VALUES ($1, $2, $3, $4, $5, 0.7, 'medium', 'r', $6, $7)";
    // A valid arc tie-in with a role and a range.
    exec(
        &db,
        arc_row,
        vec![
            Uuid::now_v7().into(),
            a.into(),
            Option::<Uuid>::None.into(),
            arc.into(),
            "tie_in_to".into(),
            Some("prelude").into(),
            Some("1-3").into(),
        ],
    )
    .await;
    // CHECKs: exactly one target; arc targets only for tie_in_to; one arc
    // row per (from, arc, kind); scope bound to its kinds.
    for (target_series, target_arc, kind, qualifier) in [
        (None, None, "tie_in_to", None),
        (Some(b), Some(arc), "tie_in_to", None),
        (None, Some(arc), "see_also", None),
        (None, Some(arc), "tie_in_to", Some("prelude")),
        (None, Some(arc), "tie_in_to", Some("relaunch")),
    ] {
        let res = try_exec(
            &db,
            arc_row,
            vec![
                Uuid::now_v7().into(),
                a.into(),
                target_series.into(),
                target_arc.into(),
                kind.into(),
                qualifier.map(str::to_owned).into(),
                Option::<String>::None.into(),
            ],
        )
        .await;
        assert!(
            res.is_err(),
            "{target_series:?} {target_arc:?} {kind} {qualifier:?}"
        );
    }
    let scoped = "INSERT INTO series_relationship_suggestion \
       (id, from_series_id, to_series_id, kind, confidence, bucket, reason, to_range, coverage) \
       VALUES ($1, $2, $3, $4, 0.8, 'high', 'r', $5, $6)";
    exec(
        &db,
        scoped,
        vec![
            Uuid::now_v7().into(),
            c.into(),
            a.into(),
            "collects".into(),
            "1-6,9".into(),
            "partial".into(),
        ],
    )
    .await;
    assert!(
        try_exec(
            &db,
            scoped,
            vec![
                Uuid::now_v7().into(),
                c.into(),
                b.into(),
                "see_also".into(),
                "1".into(),
                "full".into(),
            ],
        )
        .await
        .is_err(),
        "coverage only on collects / reprints"
    );
    assert!(
        try_exec(
            &db,
            scoped,
            vec![
                Uuid::now_v7().into(),
                c.into(),
                b.into(),
                "collects".into(),
                "x".repeat(101).into(),
                "full".into(),
            ],
        )
        .await
        .is_err(),
        "range length"
    );
    // Series targets stay unique per (from, to, kind), and the canonical
    // CHECK still holds for self-inverse kinds.
    assert!(
        try_exec(
            &db,
            INSERT_SERIES_ROW,
            vec![
                Uuid::now_v7().into(),
                a.into(),
                b.into(),
                "continues".into()
            ],
        )
        .await
        .is_err()
    );
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    assert!(
        try_exec(
            &db,
            INSERT_SERIES_ROW,
            vec![
                Uuid::now_v7().into(),
                hi.into(),
                lo.into(),
                "see_also".into()
            ],
        )
        .await
        .is_err()
    );

    // ── down again: lossy ──
    migration::Migrator::down(&db, Some(steps_to_undo_ours()))
        .await
        .unwrap();
    assert!(!has_column(&db, "to_arc_id").await);
    assert!(!has_column(&db, "to_range").await);
    assert_eq!(
        count(
            &db,
            "SELECT count(*) AS n FROM series_relationship_suggestion"
        )
        .await,
        2,
        "the arc row is gone; the series rows stay"
    );
    // The old unique constraint is back.
    assert!(
        try_exec(
            &db,
            INSERT_SERIES_ROW,
            vec![
                Uuid::now_v7().into(),
                a.into(),
                b.into(),
                "continues".into()
            ],
        )
        .await
        .is_err()
    );

    // And up once more.
    migration::Migrator::up(&db, None).await.unwrap();
    assert!(has_column(&db, "to_arc_id").await);
}
