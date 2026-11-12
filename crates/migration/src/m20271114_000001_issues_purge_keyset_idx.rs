//! Index for the hard purge's keyset walk (`jobs::hard_purge`).
//!
//! The daily purge reads a library's confirmed-removed issues in pages of
//! 500, oldest confirmation first:
//! `WHERE library_id = $1 AND removed_at IS NOT NULL
//!    AND removal_confirmed_at IS NOT NULL AND removal_confirmed_at < $2
//!    AND (removal_confirmed_at, id) > ($cursor)
//!  ORDER BY removal_confirmed_at, id LIMIT 500`.
//! The only index it could use before was `issues_removed_at_idx
//! (library_id, removed_at) WHERE removed_at IS NOT NULL`, so every page
//! re-read all of the library's removed rows and sorted them: quadratic in
//! the number of candidates the walk has to pass (rows it keeps stay
//! confirmed at the front of the order).
//!
//! - `issues_purge_keyset_idx` — `(library_id, removal_confirmed_at, id)
//!   WHERE removal_confirmed_at IS NOT NULL`. The query's
//!   `removal_confirmed_at IS NOT NULL` implies the predicate, and the
//!   column order is the equality column then the keyset, so each page is
//!   one range scan that stops after `LIMIT` rows. Partial: only confirmed
//!   rows (a few per library at a time, normally none) are in it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS issues_purge_keyset_idx \
                 ON issues (library_id, removal_confirmed_at, id) \
                 WHERE removal_confirmed_at IS NOT NULL",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS issues_purge_keyset_idx")
            .await?;
        Ok(())
    }
}
