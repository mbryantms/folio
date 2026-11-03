//! Cover stack style (cover-stacks-and-onboarding-tour-1.0, PR B).
//!
//! Reading-list and collection cards show their first few covers as a
//! stack that fans out on hover / keyboard focus. The motion is a personal
//! taste, so the style follows the account:
//! `users.cover_stack_style TEXT NOT NULL DEFAULT 'pickup'`, one of the five
//! variants in `web/lib/cover-stack/variants.ts`, pinned by a CHECK.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE users ADD COLUMN IF NOT EXISTS cover_stack_style TEXT \
             NOT NULL DEFAULT 'pickup'",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE users DROP CONSTRAINT IF EXISTS users_cover_stack_style_check",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE users ADD CONSTRAINT users_cover_stack_style_check \
             CHECK (cover_stack_style IN ('straight', 'hand', 'arc', 'pile', 'pickup'))",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE users DROP CONSTRAINT IF EXISTS users_cover_stack_style_check",
        )
        .await?;
        db.execute_unprepared("ALTER TABLE users DROP COLUMN IF EXISTS cover_stack_style")
            .await?;
        Ok(())
    }
}
