//! Multi-page rails M4 — allow `kind = 'page'` in `user_sidebar_entries`.
//!
//! The original migration locked the discriminator to a fixed enum of
//! `('builtin', 'library', 'view')`. Adding multi-page rails introduces
//! a fourth discriminator value `'page'` whose `ref_id` is a
//! `user_page.id`. This migration drops and re-adds the CHECK
//! constraint with the new value included; otherwise an `INSERT` of a
//! page override would fail at write time.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared(
            "ALTER TABLE user_sidebar_entries \
             DROP CONSTRAINT IF EXISTS user_sidebar_entries_kind_chk",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE user_sidebar_entries \
             ADD CONSTRAINT user_sidebar_entries_kind_chk \
             CHECK (kind IN ('builtin', 'library', 'view', 'page'))",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        // Refuse to roll back if any rows already use kind='page' —
        // dropping the constraint with the new enum still in use would
        // re-add an invalid constraint the very next write breaks on.
        conn.execute_unprepared("DELETE FROM user_sidebar_entries WHERE kind = 'page'")
            .await?;
        conn.execute_unprepared(
            "ALTER TABLE user_sidebar_entries \
             DROP CONSTRAINT IF EXISTS user_sidebar_entries_kind_chk",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE user_sidebar_entries \
             ADD CONSTRAINT user_sidebar_entries_kind_chk \
             CHECK (kind IN ('builtin', 'library', 'view'))",
        )
        .await?;
        Ok(())
    }
}
