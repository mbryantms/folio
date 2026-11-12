//! Indexes for the removal confirm sweep's keyset walk
//! (`library::reconcile::auto_confirm_sweep`).
//!
//! The daily sweep reads a library's unconfirmed soft-removed rows in pages
//! of 500, oldest removal first:
//! `WHERE library_id = $1 AND removed_at IS NOT NULL
//!    AND removal_confirmed_at IS NULL AND removed_at < $2
//!    AND (removed_at, id) > ($cursor)
//!  ORDER BY removed_at, id LIMIT 500`,
//! and walks to the end of them (rows it cannot confirm no longer stop it).
//! The only indexes it could use were `issues_removed_at_idx` /
//! `series_removed_at_idx (library_id, removed_at) WHERE removed_at IS NOT
//! NULL`, which do not carry `id`. A reconcile stamps every row it removes
//! in one run with the same `removed_at`, so a large removal is one long run
//! of equal keys, and every page inside it re-read the whole run from its
//! start and sorted it by `id`: quadratic in the size of the removal.
//!
//! - `issues_confirm_keyset_idx` / `series_confirm_keyset_idx` —
//!   `(library_id, removed_at, id) WHERE removed_at IS NOT NULL AND
//!   removal_confirmed_at IS NULL`. The query's predicates imply the
//!   partial one, and the column order is the equality column then the
//!   keyset, so each page is one range scan that stops after `LIMIT` rows.
//!   Partial: only rows waiting for confirmation are in it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS issues_confirm_keyset_idx \
             ON issues (library_id, removed_at, id) \
             WHERE removed_at IS NOT NULL AND removal_confirmed_at IS NULL",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS series_confirm_keyset_idx \
             ON series (library_id, removed_at, id) \
             WHERE removed_at IS NOT NULL AND removal_confirmed_at IS NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS series_confirm_keyset_idx")
            .await?;
        db.execute_unprepared("DROP INDEX IF EXISTS issues_confirm_keyset_idx")
            .await?;
        Ok(())
    }
}
