//! `m20270608_000001_series_named_specials` round-trip: the one-off repair
//! that clears `issues.special_type` where the series' own identity
//! carries the marker must key on the same identity the scanner compares
//! against — the series *folder* name — and tokenize the way
//! `series_name_carries_marker` does.
//!
//! The per-test DB is cloned from the fully-migrated template, so the
//! test seeds rows, rolls the migration down (a no-op) and up again.

mod common;

use common::TestApp;
use common::seed::{seed_issue, seed_library, seed_series};
use migration::MigratorTrait;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use std::path::Path;
use uuid::Uuid;

const MIGRATION: &str = "m20270608_000001_series_named_specials";

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

/// Seed a series (name + optional folder path) holding one issue tagged
/// `tag`; returns the issue id.
async fn tagged_issue(
    db: &DatabaseConnection,
    lib_id: Uuid,
    root: &Path,
    name: &str,
    folder: Option<&str>,
    tag: &str,
) -> String {
    let series_id = seed_series(db, lib_id, name).await;
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE series SET folder_path = $1 WHERE id = $2",
        [
            folder
                .map(|f| root.join(f).to_string_lossy().into_owned())
                .into(),
            series_id.into(),
        ],
    ))
    .await
    .unwrap();
    // The seeded id is the payload's content hash, so each row needs
    // distinct bytes.
    let file = root.join(format!("{series_id}.cbz"));
    let id = seed_issue(db, lib_id, series_id, &file, series_id.as_bytes(), 1.0).await;
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE issues SET special_type = $1 WHERE id = $2",
        [tag.into(), id.clone().into()],
    ))
    .await
    .unwrap();
    id
}

async fn special_type(db: &DatabaseConnection, id: &str) -> Option<String> {
    db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT special_type FROM issues WHERE id = $1",
        [id.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<Option<String>>("", "special_type")
    .unwrap()
}

#[tokio::test]
async fn repair_keys_on_the_series_folder_name_like_the_scanner() {
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let lib_id = seed_library(&db, root).await;

    // Folder carries the marker → the issues are the run.
    let folder_annual = tagged_issue(
        &db,
        lib_id,
        root,
        "The Amazing Spider-Man Annual",
        Some("The Amazing Spider-Man Annual (1965)"),
        "Annual",
    )
    .await;
    // Name carries the marker, folder does not: the scanner keeps this
    // annual (it sits in `Batman (2016)/Annuals/` and its `<Series>` set
    // the row name) — the repair must not strip it.
    let name_only = tagged_issue(
        &db,
        lib_id,
        root,
        "Batman Annual",
        Some("Batman (2016)"),
        "Annual",
    )
    .await;
    // No folder_path (pre-fast-path row) → fall back to the name.
    let legacy = tagged_issue(&db, lib_id, root, "Marvel Holiday Special", None, "Special").await;
    // Underscore-separated folder tokenizes like the scanner.
    let underscore = tagged_issue(
        &db,
        lib_id,
        root,
        "Deadpool",
        Some("Deadpool_One_Shot"),
        "OneShot",
    )
    .await;
    let oneshot_joined = tagged_issue(
        &db,
        lib_id,
        root,
        "X",
        Some("Batman Oneshots (2010)"),
        "OneShot",
    )
    .await;
    // Whole-word only.
    let semiannual = tagged_issue(
        &db,
        lib_id,
        root,
        "X",
        Some("Semiannual Report (1990)"),
        "Annual",
    )
    .await;
    let specialists = tagged_issue(
        &db,
        lib_id,
        root,
        "X",
        Some("The Specialists (2001)"),
        "Special",
    )
    .await;
    // The marker must match the tag; TPB is never dropped.
    let cross_tag = tagged_issue(
        &db,
        lib_id,
        root,
        "X",
        Some("Spider-Man Annual (1965)"),
        "Special",
    )
    .await;
    let tpb = tagged_issue(&db, lib_id, root, "X", Some("Batman Annual (1961)"), "TPB").await;

    migration::Migrator::down(&db, Some(steps_to_undo_ours()))
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();

    assert_eq!(special_type(&db, &folder_annual).await, None);
    assert_eq!(
        special_type(&db, &name_only).await.as_deref(),
        Some("Annual"),
        "series.name is not the identity; the folder is"
    );
    assert_eq!(special_type(&db, &legacy).await, None);
    assert_eq!(special_type(&db, &underscore).await, None);
    assert_eq!(special_type(&db, &oneshot_joined).await, None);
    assert_eq!(
        special_type(&db, &semiannual).await.as_deref(),
        Some("Annual")
    );
    assert_eq!(
        special_type(&db, &specialists).await.as_deref(),
        Some("Special")
    );
    assert_eq!(
        special_type(&db, &cross_tag).await.as_deref(),
        Some("Special")
    );
    assert_eq!(special_type(&db, &tpb).await.as_deref(), Some("TPB"));
}
