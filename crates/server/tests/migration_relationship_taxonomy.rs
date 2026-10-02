//! WP-7.5 migration round-trip: `m20270505_000001_relationship_taxonomy`.
//!
//! The per-test DB is cloned from the fully-migrated template (up), so the
//! test runs the migration **down**, seeds rows in the pre-WP-7.5 shape
//! (old CHECKs: `prequel_of` as the inverse of `sequel_of`, continuation
//! suggestions as `sequel_of`), runs **up** and asserts the data mapping,
//! then runs down + up once more with new-only data to check the lossy
//! down path.

mod common;

use common::TestApp;
use common::seed::{SeriesSeed, seed_library};
use migration::MigratorTrait;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, Value};
use std::collections::BTreeSet;
use uuid::Uuid;

const MIGRATION: &str = "m20270505_000001_relationship_taxonomy";

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

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn edge(db: &DatabaseConnection, from: Uuid, to: Uuid, kind: &str, source: &str) {
    exec(
        db,
        "INSERT INTO series_relationship (id, from_series_id, to_series_id, kind, source) \
         VALUES ($1, $2, $3, $4, $5)",
        vec![
            Uuid::now_v7().into(),
            from.into(),
            to.into(),
            kind.into(),
            source.into(),
        ],
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn suggestion(
    db: &DatabaseConnection,
    from: Uuid,
    to: Uuid,
    kind: &str,
    status: &str,
    accepted_kind: Option<&str>,
    source: &str,
) -> Uuid {
    let id = Uuid::now_v7();
    exec(
        db,
        "INSERT INTO series_relationship_suggestion \
           (id, from_series_id, to_series_id, kind, confidence, bucket, reason, evidence, status, accepted_kind) \
         VALUES ($1, $2, $3, $4, 0.9, 'high', 'r', $5::jsonb, $6, $7)",
        vec![
            id.into(),
            from.into(),
            to.into(),
            kind.into(),
            serde_json::json!({"sources": [{"source": source, "confidence": 0.9}]})
                .to_string()
                .into(),
            status.into(),
            accepted_kind.map(str::to_owned).into(),
        ],
    )
    .await;
    id
}

async fn edges(db: &DatabaseConnection) -> BTreeSet<(Uuid, Uuid, String)> {
    db.query_all_raw(Statement::from_string(
        DbBackend::Postgres,
        "SELECT from_series_id, to_series_id, kind FROM series_relationship \
          WHERE to_series_id IS NOT NULL",
    ))
    .await
    .unwrap()
    .into_iter()
    .map(|r| {
        (
            r.try_get::<Uuid>("", "from_series_id").unwrap(),
            r.try_get::<Uuid>("", "to_series_id").unwrap(),
            r.try_get::<String>("", "kind").unwrap(),
        )
    })
    .collect()
}

async fn suggestion_kind(db: &DatabaseConnection, id: Uuid) -> (String, Option<String>) {
    let r = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT kind, accepted_kind FROM series_relationship_suggestion WHERE id = $1",
            [id.into()],
        ))
        .await
        .unwrap()
        .expect("suggestion row");
    (
        r.try_get::<String>("", "kind").unwrap(),
        r.try_get::<Option<String>>("", "accepted_kind").unwrap(),
    )
}

fn e(from: Uuid, to: Uuid, kind: &str) -> (Uuid, Uuid, String) {
    (from, to, kind.to_owned())
}

fn sorted(a: Uuid, b: Uuid) -> (Uuid, Uuid) {
    if a < b { (a, b) } else { (b, a) }
}

#[tokio::test]
async fn taxonomy_migration_maps_old_rows_and_round_trips() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let mut s = Vec::new();
    for name in [
        "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N",
    ] {
        s.push(SeriesSeed::new(lib, name).insert(&db).await);
    }
    let [a, b, c, d, e_, f, g, h, i, j, k, l, m, n] = s[..] else {
        unreachable!()
    };

    // ── down: the pre-WP-7.5 schema ──
    migration::Migrator::down(&db, Some(steps_to_undo_ours()))
        .await
        .unwrap();

    // Manual sequel pair (stays sequel_of; prequel_of half → has_sequel).
    edge(&db, a, b, "sequel_of", "manual").await;
    edge(&db, b, a, "prequel_of", "manual").await;
    // Accepted name-continuation suggestion (→ continues / continued_by).
    edge(&db, c, d, "sequel_of", "suggested").await;
    edge(&db, d, c, "prequel_of", "suggested").await;
    let s_cd = suggestion(
        &db,
        c,
        d,
        "sequel_of",
        "accepted",
        None,
        "name_continuation",
    )
    .await;
    // Accepted shared-provider-volume suggestion (→ continues).
    edge(&db, e_, f, "sequel_of", "suggested").await;
    edge(&db, f, e_, "prequel_of", "suggested").await;
    let s_ef = suggestion(&db, e_, f, "sequel_of", "accepted", None, "provider_volume").await;
    // A see_also suggestion accepted *as* sequel_of (modified): the admin
    // chose sequel_of explicitly, so the edge stays sequel_of.
    let (g1, h1) = sorted(g, h);
    edge(&db, g1, h1, "sequel_of", "suggested").await;
    edge(&db, h1, g1, "prequel_of", "suggested").await;
    let s_gh = suggestion(
        &db,
        g1,
        h1,
        "see_also",
        "modified",
        Some("sequel_of"),
        "name_continuation",
    )
    .await;
    // Pending / stale / rejected continuation suggestions.
    let s_ij = suggestion(&db, i, j, "sequel_of", "pending", None, "name_continuation").await;
    let s_ji = suggestion(&db, j, i, "sequel_of", "stale", None, "provider_volume").await;
    let s_kl = suggestion(
        &db,
        k,
        l,
        "sequel_of",
        "rejected",
        None,
        "name_continuation",
    )
    .await;
    // Accepted with an override to the old prequel_of (→ has_sequel).
    let (m1, n1) = sorted(m, n);
    let s_mn = suggestion(
        &db,
        m1,
        n1,
        "crossover_with",
        "modified",
        Some("prequel_of"),
        "story_arc",
    )
    .await;
    edge(&db, m1, n1, "prequel_of", "suggested").await;
    edge(&db, n1, m1, "sequel_of", "suggested").await;

    // ── up ──
    migration::Migrator::up(&db, None).await.unwrap();

    let want: BTreeSet<_> = [
        e(a, b, "sequel_of"),
        e(b, a, "has_sequel"),
        e(c, d, "continues"),
        e(d, c, "continued_by"),
        e(e_, f, "continues"),
        e(f, e_, "continued_by"),
        e(g1, h1, "sequel_of"),
        e(h1, g1, "has_sequel"),
        e(m1, n1, "has_sequel"),
        e(n1, m1, "sequel_of"),
    ]
    .into_iter()
    .collect();
    assert_eq!(edges(&db).await, want);

    assert_eq!(suggestion_kind(&db, s_cd).await.0, "continues");
    assert_eq!(suggestion_kind(&db, s_ef).await.0, "continues");
    assert_eq!(
        suggestion_kind(&db, s_gh).await,
        ("see_also".to_owned(), Some("sequel_of".to_owned()))
    );
    assert_eq!(suggestion_kind(&db, s_ij).await.0, "continues");
    assert_eq!(suggestion_kind(&db, s_ji).await.0, "continues");
    assert_eq!(
        suggestion_kind(&db, s_kl).await.0,
        "sequel_of",
        "rejected rows keep their kind (dedupe treats sequel_of ≡ continues)"
    );
    assert_eq!(
        suggestion_kind(&db, s_mn).await.1.as_deref(),
        Some("has_sequel")
    );

    // The migrated rows behave under the new model: the chain from D runs
    // through the continuation, and the old contradiction rule still holds.
    let chain = server::relationships::chain(&db, d).await.unwrap();
    assert_eq!(
        chain
            .iter()
            .map(|n| (n.position, n.series_id))
            .collect::<Vec<_>>(),
        vec![(0, d), (1, c)]
    );

    // ── new-only data, then down: the lossy path ──
    let arc = Uuid::now_v7();
    exec(
        &db,
        "INSERT INTO story_arc (id, slug, name, normalized_name) VALUES ($1, 'ev', 'Ev', 'ev')",
        vec![arc.into()],
    )
    .await;
    exec(
        &db,
        "INSERT INTO series_relationship (id, from_series_id, to_arc_id, kind, qualifier) \
         VALUES ($1, $2, $3, 'tie_in_to', 'prelude')",
        vec![Uuid::now_v7().into(), a.into(), arc.into()],
    )
    .await;
    edge(&db, i, j, "annual_of", "manual").await;
    edge(&db, j, i, "has_annual", "manual").await;
    // Collides with c continues d once `continues` maps back to sequel_of:
    // down keeps one row per (from, to, old kind).
    edge(&db, c, d, "sequel_of", "manual").await;
    edge(&db, d, c, "has_sequel", "manual").await;
    exec(
        &db,
        "UPDATE series_relationship SET note = 'x', from_range = '1-6' WHERE kind = 'annual_of'",
        vec![],
    )
    .await;
    let s_new = suggestion(&db, k, m, "annual_of", "pending", None, "name_continuation").await;

    migration::Migrator::down(&db, Some(steps_to_undo_ours()))
        .await
        .unwrap();
    let old: BTreeSet<_> = edges(&db).await;
    let want_old: BTreeSet<_> = [
        e(a, b, "sequel_of"),
        e(b, a, "prequel_of"),
        e(c, d, "sequel_of"),
        e(d, c, "prequel_of"),
        e(e_, f, "sequel_of"),
        e(f, e_, "prequel_of"),
        e(g1, h1, "sequel_of"),
        e(h1, g1, "prequel_of"),
        e(m1, n1, "prequel_of"),
        e(n1, m1, "sequel_of"),
        // annual_of / has_annual have no old kind: both become see_also.
        e(i, j, "see_also"),
        e(j, i, "see_also"),
    ]
    .into_iter()
    .collect();
    assert_eq!(old, want_old);
    let arc_col = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT 1 AS one FROM information_schema.columns \
             WHERE table_name = 'series_relationship' AND column_name = 'to_arc_id'",
        ))
        .await
        .unwrap();
    assert!(arc_col.is_none(), "down drops the arc column");
    assert_eq!(suggestion_kind(&db, s_cd).await.0, "sequel_of");
    assert_eq!(
        suggestion_kind(&db, s_mn).await.1.as_deref(),
        Some("prequel_of")
    );
    let gone = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 AS one FROM series_relationship_suggestion WHERE id = $1",
            [s_new.into()],
        ))
        .await
        .unwrap();
    assert!(gone.is_none(), "suggestions of new-only kinds are dropped");

    // And back up again cleanly.
    migration::Migrator::up(&db, None).await.unwrap();
    assert!(edges(&db).await.contains(&e(b, a, "has_sequel")));
}
