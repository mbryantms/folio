//! Generational suffixes in ComicInfo CSV fields: `"Mike Deodato, Jr."`
//! is one person, not `"Mike Deodato"` + `"Jr."`. Covers the suffix-aware
//! `split_csv` through the junction writer and the catch-up backfill that
//! repairs issues scanned before it, then prunes the orphaned `Jr.` rows.

mod common;

use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{issue, issue_character, issue_credit, person};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::library::scanner::metadata_rollup::{
    prune_orphan_suffix_entities, replace_issue_metadata_from_row, rollup_series_metadata,
    run_name_suffix_backfill_page,
};
use tempfile::tempdir;

async fn credits(app: &TestApp, issue_id: &str, role: &str) -> Vec<String> {
    let mut v: Vec<String> = issue_credit::Entity::find()
        .filter(issue_credit::Column::IssueId.eq(issue_id))
        .filter(issue_credit::Column::Role.eq(role))
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.person)
        .collect();
    v.sort();
    v
}

async fn characters(app: &TestApp, issue_id: &str) -> Vec<String> {
    let mut v: Vec<String> = issue_character::Entity::find()
        .filter(issue_character::Column::IssueId.eq(issue_id))
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.character)
        .collect();
    v.sort();
    v
}

async fn person_names(app: &TestApp) -> Vec<String> {
    let mut v: Vec<String> = person::Entity::find()
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    v.sort();
    v
}

/// Seed an issue whose CSV columns carry the comma-suffix shape.
async fn seed_suffix_issue(app: &TestApp, dir: &std::path::Path) -> (uuid::Uuid, String) {
    let lib_id = LibrarySeed::new(dir).insert(&app.state().db).await;
    let series_id = SeriesSeed::new(lib_id, "Venom")
        .insert(&app.state().db)
        .await;
    let path = dir.join("venom-1.cbz");
    let id = IssueSeed::new(lib_id, series_id, &path, b"venom-1", 1.0)
        .insert(&app.state().db)
        .await;
    let mut am: issue::ActiveModel = issue::Entity::find_by_id(id.clone())
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.inker = Set(Some(
        "Andrew Hennessy, Mike Deodato, Jr., J. P. Mayer".into(),
    ));
    am.cover_artist = Set(Some("Frank Martin Jr., Mike Deodato, Jr.".into()));
    am.characters = Set(Some(
        "Eddie Brock, J. Jonah Jameson, Sr, Albert Moon, Jr.".into(),
    ));
    am.update(&app.state().db).await.unwrap();
    (series_id, id)
}

#[tokio::test]
async fn ingest_keeps_a_comma_suffixed_name_as_one_entry() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let (series_id, id) = seed_suffix_issue(&app, dir.path()).await;

    replace_issue_metadata_from_row(&app.state().db, &id)
        .await
        .unwrap();
    assert_eq!(
        credits(&app, &id, "inker").await,
        vec!["Andrew Hennessy", "J. P. Mayer", "Mike Deodato Jr."]
    );
    // Already-joined + comma-joined spellings of the same person dedupe.
    assert_eq!(
        credits(&app, &id, "cover_artist").await,
        vec!["Frank Martin Jr.", "Mike Deodato Jr."]
    );
    assert_eq!(
        characters(&app, &id).await,
        vec!["Albert Moon Jr.", "Eddie Brock", "J. Jonah Jameson Sr."]
    );

    rollup_series_metadata(&app.state().db, series_id)
        .await
        .unwrap();
    let names = person_names(&app).await;
    assert!(names.contains(&"Mike Deodato Jr.".to_owned()), "{names:?}");
    assert!(
        !names.iter().any(|n| n == "Jr." || n == "Mike Deodato"),
        "{names:?}"
    );
}

/// Issues scanned before the suffix rule have `"Mike Deodato"` + `"Jr."`
/// rows; the backfill re-derives them from the CSV columns and the prune
/// drops the orphaned `Jr.` person.
#[tokio::test]
async fn backfill_repairs_split_rows_and_prunes_orphan_suffixes() {
    let app = TestApp::spawn().await;
    let dir = tempdir().unwrap();
    let (series_id, id) = seed_suffix_issue(&app, dir.path()).await;
    let db = &app.state().db;

    // The pre-fix state: one credit row per comma piece.
    for (ordinal, name) in ["Andrew Hennessy", "Mike Deodato", "Jr.", "J. P. Mayer"]
        .into_iter()
        .enumerate()
    {
        issue_credit::ActiveModel {
            issue_id: Set(id.clone()),
            role: Set("inker".into()),
            person: Set(name.into()),
            person_id: Set(None),
            ordinal: Set(ordinal as i32),
        }
        .insert(db)
        .await
        .unwrap();
    }
    rollup_series_metadata(db, series_id).await.unwrap();
    assert!(person_names(&app).await.contains(&"Jr.".to_owned()));

    let (outcome, next) = run_name_suffix_backfill_page(db, None, 50).await.unwrap();
    assert_eq!(outcome.rebuilt, 1);
    assert_eq!(outcome.skipped, 0);
    assert!(next.is_none(), "one page");
    assert_eq!(
        credits(&app, &id, "inker").await,
        vec!["Andrew Hennessy", "J. P. Mayer", "Mike Deodato Jr."]
    );
    let pruned = prune_orphan_suffix_entities(db).await.unwrap();
    assert!(pruned >= 1, "the Jr. person row is gone: {pruned}");
    let names = person_names(&app).await;
    assert!(!names.contains(&"Jr.".to_owned()), "{names:?}");
    assert!(names.contains(&"Mike Deodato Jr.".to_owned()), "{names:?}");

    // Nothing left to do: a second pass finds no split rows to rebuild
    // (the CSV still matches, but the junctions already agree — the
    // writer's short-circuit makes the rebuild a no-op).
    let (again, _) = run_name_suffix_backfill_page(db, None, 50).await.unwrap();
    assert_eq!(
        again.rebuilt, 1,
        "the row still matches the CSV shape; rebuild is idempotent"
    );
}
