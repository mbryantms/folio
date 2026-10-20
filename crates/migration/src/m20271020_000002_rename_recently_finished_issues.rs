//! Rename the wave-2 built-in issue view `…0020` from "Recently finished"
//! to "Recently finished issues" so it reads unambiguously beside the
//! series twin "Recently finished series" (`…0003`).
//!
//! v0.60.0 shipped the old name. The seed in `m20271020_000001` uses
//! `ON CONFLICT (id) DO NOTHING`, so changing its template string only
//! helps fresh installs; upgraded installs need this guarded UPDATE
//! (same shape as the `m20270122` renames). On a fresh install the seed
//! already carries the new name and this is a no-op.
use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

const RECENTLY_FINISHED_ISSUES_ID: &str = "00000000-0000-0000-0000-000000000020";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let conn = manager.get_connection();
        conn.execute_raw(Statement::from_sql_and_values(
            backend,
            "UPDATE saved_views SET name = 'Recently finished issues' \
             WHERE id = $1::uuid AND name = 'Recently finished'",
            [RECENTLY_FINISHED_ISSUES_ID.into()],
        ))
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let conn = manager.get_connection();
        conn.execute_raw(Statement::from_sql_and_values(
            backend,
            "UPDATE saved_views SET name = 'Recently finished' \
             WHERE id = $1::uuid AND name = 'Recently finished issues'",
            [RECENTLY_FINISHED_ISSUES_ID.into()],
        ))
        .await?;
        Ok(())
    }
}
