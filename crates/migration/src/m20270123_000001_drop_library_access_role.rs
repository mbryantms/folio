//! WP-2.7 (roadmap D5): drop `library_user_access.role`.
//!
//! The column accepted `reader` plus an editor value since Phase 1 but nothing ever
//! read it — every mutation is admin-gated or owner-scoped, so the
//! editor grant never did anything. Rather than ship a half-defined
//! editor role, the decision (2026-09-29) is to remove it; a real
//! per-library editor role can be re-added later with its own semantics.
//!
//! `age_rating_max` stays and is now enforced (see
//! `server::library::age_rating`). `down` re-adds the column with its
//! original `NOT NULL DEFAULT 'reader'` shape so a rollback restores the
//! pre-WP-2.7 schema exactly.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE library_user_access DROP COLUMN IF EXISTS role")
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE library_user_access \
                 ADD COLUMN IF NOT EXISTS role TEXT NOT NULL DEFAULT 'reader'",
            )
            .await?;
        Ok(())
    }
}
