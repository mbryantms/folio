//! Entity ↔ Postgres parity guard (audit-remediation M8.3, shipped
//! 2026-05-24).
//!
//! Every column the migrations create should be reachable through an
//! entity in `crates/entity/`. The exceptions — `search_doc` (a
//! GENERATED ALWAYS tsvector on series + issues) and any SQL view —
//! are spelled out in the allow-lists below.
//!
//! A divergence fails this test with a column-level diff, so adding
//! a column to the schema without updating the entity is loud.

mod common;

use common::TestApp;
use sea_orm::{
    ColumnTrait, Database, DatabaseBackend, EntityName, FromQueryResult, Iterable, Statement,
};
use std::collections::HashSet;

/// Postgres columns that are intentionally absent from their entity.
/// Each entry is `(table_name, column_name, reason)` — the reason is
/// asserted on so a future drift between this list and the
/// entity comments still produces grep-able text.
const GENERATED_COLUMNS: &[(&str, &str, &str)] = &[
    (
        "series",
        "search_doc",
        "Postgres GENERATED ALWAYS tsvector — see m20260301_000001_search_docs",
    ),
    (
        "issues",
        "search_doc",
        "Postgres GENERATED ALWAYS tsvector — see m20260301_000001_search_docs",
    ),
];

/// `(entity table, column iterator → strings)` for every entity
/// backed by a real table (not a view). Adding a new entity? Add it
/// here too — the test will then guard its parity automatically.
struct EntityCheck {
    table: &'static str,
    columns: Vec<String>,
}

fn all_entities() -> Vec<EntityCheck> {
    use entity::*;
    fn collect<E: EntityName, C: Iterable + ColumnTrait>() -> EntityCheck {
        EntityCheck {
            table: E::default().table_name().to_string().leak(),
            columns: C::iter().map(|c| c.to_string()).collect(),
        }
    }
    vec![
        collect::<app_password::Entity, app_password::Column>(),
        collect::<app_setting::Entity, app_setting::Column>(),
        collect::<audit_log::Entity, audit_log::Column>(),
        collect::<auth_session::Entity, auth_session::Column>(),
        collect::<catalog_source::Entity, catalog_source::Column>(),
        collect::<cbl_entry::Entity, cbl_entry::Column>(),
        collect::<cbl_list::Entity, cbl_list::Column>(),
        collect::<cbl_refresh_log::Entity, cbl_refresh_log::Column>(),
        collect::<collection_entry::Entity, collection_entry::Column>(),
        collect::<issue::Entity, issue::Column>(),
        collect::<issue_character::Entity, issue_character::Column>(),
        collect::<issue_credit::Entity, issue_credit::Column>(),
        collect::<issue_genre::Entity, issue_genre::Column>(),
        collect::<issue_location::Entity, issue_location::Column>(),
        collect::<issue_tag::Entity, issue_tag::Column>(),
        collect::<issue_team::Entity, issue_team::Column>(),
        collect::<library::Entity, library::Column>(),
        collect::<library_health_issue::Entity, library_health_issue::Column>(),
        collect::<library_user_access::Entity, library_user_access::Column>(),
        collect::<log_widget::Entity, log_widget::Column>(),
        collect::<marker::Entity, marker::Column>(),
        collect::<person::Entity, person::Column>(),
        collect::<progress_record::Entity, progress_record::Column>(),
        collect::<rail_dismissal::Entity, rail_dismissal::Column>(),
        collect::<reading_session::Entity, reading_session::Column>(),
        collect::<saved_view::Entity, saved_view::Column>(),
        collect::<scan_run::Entity, scan_run::Column>(),
        collect::<series::Entity, series::Column>(),
        collect::<series_character::Entity, series_character::Column>(),
        collect::<series_credit::Entity, series_credit::Column>(),
        collect::<series_genre::Entity, series_genre::Column>(),
        collect::<series_location::Entity, series_location::Column>(),
        collect::<series_tag::Entity, series_tag::Column>(),
        collect::<series_team::Entity, series_team::Column>(),
        collect::<user::Entity, user::Column>(),
        collect::<user_page::Entity, user_page::Column>(),
        collect::<user_rating::Entity, user_rating::Column>(),
        collect::<user_sidebar_entry::Entity, user_sidebar_entry::Column>(),
        collect::<user_view_pin::Entity, user_view_pin::Column>(),
    ]
}

#[derive(FromQueryResult)]
struct ColRow {
    column_name: String,
}

async fn live_columns(
    db: &sea_orm::DatabaseConnection,
    table: &str,
) -> HashSet<String> {
    ColRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT column_name FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = $1",
        [table.into()],
    ))
    .all(db)
    .await
    .unwrap_or_else(|e| panic!("information_schema.columns query for {table}: {e}"))
    .into_iter()
    .map(|r| r.column_name)
    .collect()
}

#[tokio::test]
async fn every_entity_column_exists_in_postgres() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let mut failures: Vec<String> = Vec::new();

    for check in all_entities() {
        let live = live_columns(&db, check.table).await;
        assert!(
            !live.is_empty(),
            "{} entity claims to back a Postgres table but information_schema returned no columns — \
             table missing from the schema?",
            check.table
        );
        for col in &check.columns {
            if !live.contains(col) {
                failures.push(format!(
                    "{}.{}: declared in entity but absent from Postgres",
                    check.table, col,
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "entity → DB drift:\n  - {}",
        failures.join("\n  - ")
    );
}

#[tokio::test]
async fn every_postgres_column_is_in_an_entity_or_allow_listed() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let mut failures: Vec<String> = Vec::new();

    let allow_list: HashSet<(&str, &str)> = GENERATED_COLUMNS
        .iter()
        .map(|(t, c, _)| (*t, *c))
        .collect();

    for check in all_entities() {
        let live = live_columns(&db, check.table).await;
        let declared: HashSet<String> = check.columns.iter().cloned().collect();
        for db_col in &live {
            if declared.contains(db_col) {
                continue;
            }
            if allow_list.contains(&(check.table, db_col.as_str())) {
                continue;
            }
            failures.push(format!(
                "{}.{}: present in Postgres but absent from entity (and not in GENERATED_COLUMNS)",
                check.table, db_col,
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "DB → entity drift:\n  - {}\n\n\
         If the new column is a Postgres GENERATED ALWAYS column or a similar \
         intentional entity omission, add it to `GENERATED_COLUMNS` with a one-line \
         reason and document it on the entity (see series.rs / issue.rs).",
        failures.join("\n  - ")
    );
}

/// Sanity check that the allow-list itself stays anchored to real
/// Postgres columns — otherwise a typo here could mask a future
/// genuine drift.
#[tokio::test]
async fn generated_columns_allow_list_is_grounded() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    for (table, col, reason) in GENERATED_COLUMNS {
        let live = live_columns(&db, table).await;
        assert!(
            live.contains(*col),
            "GENERATED_COLUMNS lists {table}.{col} ({reason}) but the column is not in Postgres — \
             remove the entry or fix the column name."
        );
    }
}
