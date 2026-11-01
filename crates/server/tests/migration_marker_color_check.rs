//! Audit AR-8: `m20271101_000002_marker_color_check` bounds
//! `markers.color` in the schema with the API's colour rule.
//!
//! The per-test DB is cloned from the fully-migrated template, so the
//! constraint is live: a bad colour is rejected, a palette name / hex /
//! NULL is accepted. The round trip runs the migration **down** (CHECK
//! gone, bad values insertable), seeds out-of-rule rows, and **up** again,
//! asserting the normalisation (trim, palette lower-casing, the rest to
//! NULL) and that the CHECK is back.

mod common;

use common::TestApp;
use common::seed::{IssueSeed, SeriesSeed, seed_library};
use migration::MigratorTrait;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement};
use uuid::Uuid;

const MIGRATION: &str = "m20271101_000002_marker_color_check";

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

struct Fixture {
    user: Uuid,
    series: Uuid,
    issue: String,
    _tmp: tempfile::TempDir,
}

async fn fixture(db: &DatabaseConnection) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(db, tmp.path()).await;
    let series = SeriesSeed::new(lib, "Colour Check").insert(db).await;
    let path = tmp.path().join("a.cbz");
    let issue = IssueSeed::new(lib, series, &path, b"colour-a", 1.0)
        .insert(db)
        .await;
    let user = Uuid::now_v7();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO users (id, external_id, display_name) VALUES ($1, $2, 'colour')",
        [user.into(), format!("local:{user}").into()],
    ))
    .await
    .unwrap();
    Fixture {
        user,
        series,
        issue,
        _tmp: tmp,
    }
}

/// Insert a bookmark with `color`; returns its id or the DB error.
async fn insert_marker(
    db: &DatabaseConnection,
    fx: &Fixture,
    color: Option<&str>,
) -> Result<Uuid, DbErr> {
    let id = Uuid::now_v7();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO markers (id, user_id, series_id, issue_id, page_index, kind, color) \
         VALUES ($1, $2, $3, $4, 0, 'bookmark', $5)",
        [
            id.into(),
            fx.user.into(),
            fx.series.into(),
            fx.issue.clone().into(),
            color.map(str::to_owned).into(),
        ],
    ))
    .await?;
    Ok(id)
}

async fn color_of(db: &DatabaseConnection, id: Uuid) -> Option<String> {
    db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT color FROM markers WHERE id = $1",
        [id.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<Option<String>>("", "color")
    .unwrap()
}

#[tokio::test]
async fn check_rejects_bad_colour_and_accepts_good() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let fx = fixture(&db).await;

    for ok in [
        Some("yellow"),
        Some("violet"),
        Some("#a1B2c3"),
        Some("#A1B2C3FF"),
        None,
    ] {
        insert_marker(&db, &fx, ok)
            .await
            .unwrap_or_else(|e| panic!("{ok:?} must be accepted: {e}"));
    }
    for bad in [
        "purple",
        "Yellow",
        " red",
        "",
        "#abc",
        "#1234567",
        "red;",
        &"x".repeat(33),
    ] {
        let err = insert_marker(&db, &fx, Some(bad))
            .await
            .expect_err(&format!("{bad:?} must be rejected"));
        assert!(
            err.to_string().contains("markers_color_chk"),
            "{bad:?}: {err}"
        );
    }
}

#[tokio::test]
async fn up_normalises_existing_rows_and_down_drops_the_check() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let fx = fixture(&db).await;

    migration::Migrator::down(&db, Some(steps_to_undo_ours()))
        .await
        .unwrap();
    // Pre-constraint data: anything goes.
    let good = insert_marker(&db, &fx, Some("#336699")).await.unwrap();
    let none = insert_marker(&db, &fx, None).await.unwrap();
    let padded = insert_marker(&db, &fx, Some("  red  ")).await.unwrap();
    let blank = insert_marker(&db, &fx, Some("   ")).await.unwrap();
    let cased = insert_marker(&db, &fx, Some("Yellow")).await.unwrap();
    let junk = insert_marker(&db, &fx, Some("chartreuse")).await.unwrap();
    let long = insert_marker(&db, &fx, Some(&"#".repeat(500)))
        .await
        .unwrap();

    migration::Migrator::up(&db, None).await.unwrap();

    assert_eq!(color_of(&db, good).await.as_deref(), Some("#336699"));
    assert_eq!(color_of(&db, none).await, None);
    assert_eq!(color_of(&db, padded).await.as_deref(), Some("red"));
    assert_eq!(color_of(&db, blank).await, None, "blank is no colour");
    assert_eq!(color_of(&db, cased).await.as_deref(), Some("yellow"));
    assert_eq!(color_of(&db, junk).await, None, "unknown name dropped");
    assert_eq!(color_of(&db, long).await, None, "over-long value dropped");
    assert!(
        insert_marker(&db, &fx, Some("chartreuse")).await.is_err(),
        "the CHECK is back after up"
    );
}
