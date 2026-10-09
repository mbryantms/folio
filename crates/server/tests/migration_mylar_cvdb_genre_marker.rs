//! `m20270610_000001_mylar_cvdb_genre_marker`: Mylar3's `CVDB<id>` genre
//! marker is removed from the genre junctions and the derived
//! `issues.genre` column, and harvested as the issue's ComicVine id when
//! the issue has none. Real genres and existing ComicVine ids are kept.
//!
//! The migration already ran on the test template, so the test seeds
//! rows, rolls the migration down (a no-op) and up again.

mod common;

use common::TestApp;
use common::seed::{seed_issue, seed_library, seed_series};
use migration::MigratorTrait;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;

const MIGRATION: &str = "m20270610_000001_mylar_cvdb_genre_marker";

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

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}

async fn one_string(
    db: &DatabaseConnection,
    sql: &str,
    values: Vec<sea_orm::Value>,
    col: &str,
) -> Option<String> {
    db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap()
    .and_then(|r| r.try_get::<Option<String>>("", col).unwrap())
}

/// Seed an issue whose genre junction + CSV column hold `genres` (in
/// order); returns the issue id.
async fn issue_with_genres(
    db: &DatabaseConnection,
    lib_id: Uuid,
    series_id: Uuid,
    root: &std::path::Path,
    n: u8,
    genres: &[&str],
) -> String {
    let file = root.join(format!("{series_id}-{n}.cbz"));
    let id = seed_issue(db, lib_id, series_id, &file, &[n], f64::from(n)).await;
    for (ordinal, g) in genres.iter().enumerate() {
        exec(
            db,
            "INSERT INTO issue_genres (issue_id, genre, ordinal) VALUES ($1, $2, $3)",
            vec![id.clone().into(), (*g).into(), (ordinal as i32).into()],
        )
        .await;
    }
    exec(
        db,
        "UPDATE issues SET genre = $1 WHERE id = $2",
        vec![genres.join(", ").into(), id.clone().into()],
    )
    .await;
    id
}

async fn genre_rows(db: &DatabaseConnection, issue_id: &str) -> Vec<String> {
    db.query_all_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT genre FROM issue_genres WHERE issue_id = $1 ORDER BY ordinal",
        [issue_id.into()],
    ))
    .await
    .unwrap()
    .into_iter()
    .map(|r| r.try_get::<String>("", "genre").unwrap())
    .collect()
}

async fn comicvine_id(db: &DatabaseConnection, issue_id: &str) -> Option<(String, String)> {
    db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT external_id, set_by FROM external_ids \
          WHERE entity_type = 'issue' AND entity_id = $1 AND source = 'comicvine'",
        [issue_id.into()],
    ))
    .await
    .unwrap()
    .map(|r| {
        (
            r.try_get::<String>("", "external_id").unwrap(),
            r.try_get::<String>("", "set_by").unwrap(),
        )
    })
}

#[tokio::test]
async fn repair_strips_the_marker_and_keeps_the_id() {
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let lib_id = seed_library(&db, root).await;
    let series_id = seed_series(&db, lib_id, "The Flash").await;

    // Marker next to a real genre, no ComicVine id yet → harvested.
    let tagged =
        issue_with_genres(&db, lib_id, series_id, root, 1, &["Superhero", "CVDB34263"]).await;
    // Marker only, and an existing provider-set id that must survive.
    let only_marker = issue_with_genres(&db, lib_id, series_id, root, 2, &["cvdb99"]).await;
    exec(
        &db,
        "INSERT INTO external_ids (entity_type, entity_id, source, external_id, set_by, first_set_at, last_synced_at) \
         VALUES ('issue', $1, 'comicvine', '777', 'comicvine', now(), now())",
        vec![only_marker.clone().into()],
    )
    .await;
    // Clean issue: untouched.
    let clean = issue_with_genres(&db, lib_id, series_id, root, 3, &["Action", "Adventure"]).await;
    // Series-level chip.
    exec(
        &db,
        "INSERT INTO series_genres (series_id, genre) VALUES ($1, 'Superhero'), ($1, 'CVDB34263')",
        vec![series_id.into()],
    )
    .await;

    migration::Migrator::down(&db, Some(steps_to_undo_ours()))
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();

    assert_eq!(genre_rows(&db, &tagged).await, vec!["Superhero"]);
    assert_eq!(
        one_string(
            &db,
            "SELECT genre FROM issues WHERE id = $1",
            vec![tagged.clone().into()],
            "genre"
        )
        .await
        .as_deref(),
        Some("Superhero"),
        "CSV cache rebuilt from the junction"
    );
    assert_eq!(
        comicvine_id(&db, &tagged).await,
        Some(("34263".to_owned(), "comicinfo".to_owned())),
        "the marker becomes the issue's ComicVine id at file tier"
    );

    assert!(genre_rows(&db, &only_marker).await.is_empty());
    assert_eq!(
        one_string(
            &db,
            "SELECT genre FROM issues WHERE id = $1",
            vec![only_marker.clone().into()],
            "genre"
        )
        .await,
        None,
        "a marker-only genre column becomes NULL"
    );
    assert_eq!(
        comicvine_id(&db, &only_marker).await,
        Some(("777".to_owned(), "comicvine".to_owned())),
        "an existing ComicVine id is never overwritten"
    );

    assert_eq!(genre_rows(&db, &clean).await, vec!["Action", "Adventure"]);
    assert_eq!(
        one_string(
            &db,
            "SELECT genre FROM issues WHERE id = $1",
            vec![clean.clone().into()],
            "genre"
        )
        .await
        .as_deref(),
        Some("Action, Adventure"),
        "untouched rows keep their column as-is"
    );

    let series_genres: Vec<String> = db
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT genre FROM series_genres WHERE series_id = $1 ORDER BY genre",
            [series_id.into()],
        ))
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.try_get::<String>("", "genre").unwrap())
        .collect();
    assert_eq!(series_genres, vec!["Superhero"]);
}
