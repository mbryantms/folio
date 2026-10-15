//! Reverse-FK indexes the post-audit review found missing (T3.8). Every
//! one backs a lookup the code already does — "who references this
//! issue / series?" — that today walks the referencing table:
//!
//! - `collection_entries (series_id)` / `(issue_id)`: the "Appears in"
//!   tab and the hard-purge / removal sweeps look entries up by target,
//!   but the only indexes lead with `saved_view_id`.
//! - `progress_records (issue_id)`: the two existing partial indexes
//!   (`page_hash IS NULL` / `IS NOT NULL`) each serve their own probe,
//!   but a plain `WHERE issue_id = $1` (purge, per-issue stats, remap)
//!   implies neither predicate and cannot use them.
//! - `user_ratings (target_type, target_id)`: the per-entity rating
//!   lookups (`WHERE target_type = 'issue' AND target_id = $1`) only had
//!   the `(user_id, …)` primary key and `(user_id, target_type)`.
//! - `issues (superseded_by)`: "which row superseded me?" during retag /
//!   purge.
//! - `character (first_appearance_issue_id)`: the issue page's cast
//!   lookup and the purge cascade.
//!
//! Plain `CREATE INDEX` (the suite's migrations are transactional, so no
//! `CONCURRENTLY`); each table is small per deployment and the indexes
//! build in well under a second at the audit's 50k-issue scale. On a
//! live prod, run the migration during low use.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

const INDEXES: &[(&str, &str)] = &[
    (
        "collection_entries_series_idx",
        "ON collection_entries (series_id) WHERE series_id IS NOT NULL",
    ),
    (
        "collection_entries_issue_idx",
        "ON collection_entries (issue_id) WHERE issue_id IS NOT NULL",
    ),
    (
        "progress_records_issue_idx",
        "ON progress_records (issue_id)",
    ),
    (
        "user_ratings_target_idx",
        "ON user_ratings (target_type, target_id)",
    ),
    (
        "issues_superseded_by_idx",
        "ON issues (superseded_by) WHERE superseded_by IS NOT NULL",
    ),
    (
        "character_first_appearance_issue_idx",
        "ON character (first_appearance_issue_id) WHERE first_appearance_issue_id IS NOT NULL",
    ),
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for (name, def) in INDEXES {
            db.execute_unprepared(&format!("CREATE INDEX IF NOT EXISTS {name} {def}"))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for (name, _) in INDEXES.iter().rev() {
            db.execute_unprepared(&format!("DROP INDEX IF EXISTS {name}"))
                .await?;
        }
        Ok(())
    }
}
