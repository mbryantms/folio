//! WP-8.1 migration: `m20270601_000001_canonical_credit_roles`.
//!
//! The per-test DB is cloned from the fully-migrated template, so this
//! test seeds the bad rows the pre-WP-8.1 provider apply left behind
//! (PascalCase roles, the person UUID stashed in `person`, a "ghost"
//! person minted from that UUID by the series rollup), rolls the
//! migration back (a documented no-op) and re-applies it, then asserts
//! the junctions are canonical and the CSV read-cache is rebuilt — only
//! for the issues that had bad rows.

mod common;

use common::TestApp;
use common::seed::{IssueSeed, SeriesSeed, seed_library};
use migration::MigratorTrait;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, Value};
use uuid::Uuid;

const MIGRATION: &str = "m20270601_000001_canonical_credit_roles";

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
    .unwrap();
}

async fn person(db: &DatabaseConnection, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    exec(
        db,
        "INSERT INTO person (id, slug, name, normalized_name) VALUES ($1, $2, $3, lower($3))",
        vec![id.into(), format!("p-{id}").into(), name.into()],
    )
    .await;
    id
}

async fn credit(db: &DatabaseConnection, issue: &str, role: &str, person: &str, pid: Option<Uuid>) {
    exec(
        db,
        "INSERT INTO issue_credits (issue_id, role, person, person_id, ordinal) \
         VALUES ($1, $2, $3, $4, 0)",
        vec![issue.into(), role.into(), person.into(), pid.into()],
    )
    .await;
}

async fn credits(
    db: &DatabaseConnection,
    table: &str,
    owner: &str,
    key: &str,
) -> Vec<(String, String, Option<Uuid>)> {
    let mut rows: Vec<(String, String, Option<Uuid>)> = db
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT role, person, person_id FROM {table} WHERE {key}::text = $1"),
            [owner.into()],
        ))
        .await
        .unwrap()
        .into_iter()
        .map(|r| {
            (
                r.try_get::<String>("", "role").unwrap(),
                r.try_get::<String>("", "person").unwrap(),
                r.try_get::<Option<Uuid>>("", "person_id").unwrap(),
            )
        })
        .collect();
    rows.sort();
    rows
}

async fn csv(db: &DatabaseConnection, issue: &str, col: &str) -> Option<String> {
    db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {col} AS v FROM issues WHERE id = $1"),
        [issue.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<Option<String>>("", "v")
    .unwrap()
}

async fn person_exists(db: &DatabaseConnection, id: Uuid) -> bool {
    db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT 1 AS one FROM person WHERE id = $1",
        [id.into()],
    ))
    .await
    .unwrap()
    .is_some()
}

#[tokio::test]
async fn canonicalizes_roles_and_names_dedupes_and_rebuilds_the_csv_cache() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series = SeriesSeed::new(lib, "Saga").insert(&db).await;
    let mut ids = Vec::new();
    for (n, name) in [(1.0, "a.cbz"), (2.0, "b.cbz"), (3.0, "c.cbz")] {
        let path = tmp.path().join(name);
        ids.push(
            IssueSeed::new(lib, series, &path, name.as_bytes(), n)
                .insert(&db)
                .await,
        );
    }
    let [a, b, c] = <[String; 3]>::try_from(ids).unwrap();

    let bkv = person(&db, "Brian K. Vaughan").await;
    let fs = person(&db, "Fiona Staples").await;
    let jd = person(&db, "Jane Doe").await;
    let real = person(&db, "Pat Penciller").await;
    // The rollup's ghost: a person *named* with `real`'s UUID.
    let ghost = person(&db, &real.to_string()).await;

    // Issue A — what a pre-WP-8.1 non-writeback apply wrote: PascalCase
    // roles, the person UUID in `person`, plus spellings that collide
    // once normalized.
    credit(&db, &a, "Writer", &bkv.to_string(), Some(bkv)).await;
    credit(&db, &a, "writer", "Brian K. Vaughan", Some(bkv)).await;
    credit(&db, &a, "CoverArtist", &fs.to_string(), Some(fs)).await;
    credit(&db, &a, "Cover Artist", "Fiona Staples", Some(fs)).await;
    credit(&db, &a, "Editor In Chief", &bkv.to_string(), Some(bkv)).await;
    credit(&db, &a, "Ink Assists", &jd.to_string(), Some(jd)).await;
    // Issue B — the rollup re-pointed `person_id` at the ghost.
    credit(&db, &b, "Penciller", &real.to_string(), Some(ghost)).await;
    // Issue C — already canonical, scanner-shaped (no person_id) with the
    // file's own CSV spelling: must be left alone.
    credit(&db, &c, "writer", "Someone", None).await;
    exec(
        &db,
        "UPDATE issues SET writer = 'Someone; Else' WHERE id = $1",
        vec![c.clone().into()],
    )
    .await;
    // Series credits copied from the bad issue rows by the rollup.
    for (role, p, pid) in [
        ("Writer", bkv.to_string(), bkv),
        ("writer", "Brian K. Vaughan".to_owned(), bkv),
        ("Penciller", real.to_string(), ghost),
    ] {
        exec(
            &db,
            "INSERT INTO series_credits (series_id, role, person, person_id) \
             VALUES ($1, $2, $3, $4)",
            vec![series.into(), role.into(), p.into(), pid.into()],
        )
        .await;
    }
    assert_eq!(csv(&db, &a, "writer").await, None);

    // down (no-op) → up re-runs the normalization over the seeded rows.
    migration::Migrator::down(&db, Some(steps_to_undo_ours()))
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();

    assert_eq!(
        credits(&db, "issue_credits", &a, "issue_id").await,
        vec![
            ("cover_artist".into(), "Fiona Staples".into(), Some(fs)),
            ("editor".into(), "Brian K. Vaughan".into(), Some(bkv)),
            ("ink_assists".into(), "Jane Doe".into(), Some(jd)),
            ("writer".into(), "Brian K. Vaughan".into(), Some(bkv)),
        ]
    );
    assert_eq!(
        credits(&db, "issue_credits", &b, "issue_id").await,
        vec![("penciller".into(), "Pat Penciller".into(), Some(real))]
    );
    assert_eq!(
        credits(&db, "issue_credits", &c, "issue_id").await,
        vec![("writer".into(), "Someone".into(), None)]
    );
    assert_eq!(
        credits(&db, "series_credits", &series.to_string(), "series_id").await,
        vec![
            ("penciller".into(), "Pat Penciller".into(), Some(real)),
            ("writer".into(), "Brian K. Vaughan".into(), Some(bkv)),
        ]
    );

    // CSV read-cache rebuilt for the affected issues…
    assert_eq!(
        csv(&db, &a, "writer").await.as_deref(),
        Some("Brian K. Vaughan")
    );
    assert_eq!(
        csv(&db, &a, "cover_artist").await.as_deref(),
        Some("Fiona Staples")
    );
    assert_eq!(
        csv(&db, &a, "editor").await.as_deref(),
        Some("Brian K. Vaughan")
    );
    assert_eq!(csv(&db, &a, "penciller").await, None);
    assert_eq!(
        csv(&db, &b, "penciller").await.as_deref(),
        Some("Pat Penciller")
    );
    // …and only for them: a canonical issue keeps its file spelling.
    assert_eq!(
        csv(&db, &c, "writer").await.as_deref(),
        Some("Someone; Else")
    );

    // The ghost person is gone; the real people stay.
    assert!(!person_exists(&db, ghost).await, "ghost person deleted");
    for p in [bkv, fs, jd, real] {
        assert!(person_exists(&db, p).await);
    }

    // Idempotent: a second pass changes nothing.
    migration::Migrator::down(&db, Some(steps_to_undo_ours()))
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();
    assert_eq!(credits(&db, "issue_credits", &a, "issue_id").await.len(), 4);
    assert_eq!(
        csv(&db, &c, "writer").await.as_deref(),
        Some("Someone; Else")
    );
}
