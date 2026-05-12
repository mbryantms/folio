//! Promote favorite from a `kind` to a flag.
//!
//! The unified-marker plan originally treated favorites as a fourth
//! kind alongside bookmark / note / highlight, but no reader UI ever
//! materialized a `kind='favorite'` row — leaving the Favorites chip
//! on /bookmarks unreachable. This migration promotes favorite to a
//! boolean flag (`is_favorite`) usable on any marker. Anyone is now
//! free to favorite an existing bookmark / note / highlight without
//! duplicating the row.
//!
//! Migration of any pre-existing `kind='favorite'` rows: rewrite as
//! `kind='bookmark', is_favorite=true`. Favorites were page-level by
//! design, so bookmark is the closest semantic carry-over.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            "ALTER TABLE markers ADD COLUMN is_favorite BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .await?;

        // Carry pre-existing favorite rows over to the new shape.
        // Defensive: works even if no rows have kind='favorite'.
        db.execute_unprepared(
            "UPDATE markers \
             SET kind = 'bookmark', is_favorite = TRUE \
             WHERE kind = 'favorite'",
        )
        .await?;

        // Tighten the kind allow-list. The old constraint admitted
        // 'favorite'; the new schema rejects it so callers get a clear
        // 422 rather than a row that nothing in the UI can create.
        db.execute_unprepared("ALTER TABLE markers DROP CONSTRAINT markers_kind_chk")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE markers ADD CONSTRAINT markers_kind_chk \
             CHECK (kind IN ('bookmark','note','highlight'))",
        )
        .await?;

        // Filter index for "show me my favorites" queries on
        // /bookmarks. Partial — most users will have very few favorites
        // relative to total markers, so the index stays tiny.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS markers_user_favorite_idx \
             ON markers(user_id, updated_at DESC) WHERE is_favorite",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Reverse the kind allow-list change first so the rewrite
        // below has a valid target value.
        db.execute_unprepared("ALTER TABLE markers DROP CONSTRAINT markers_kind_chk")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE markers ADD CONSTRAINT markers_kind_chk \
             CHECK (kind IN ('bookmark','note','favorite','highlight'))",
        )
        .await?;

        db.execute_unprepared(
            "UPDATE markers \
             SET kind = 'favorite' \
             WHERE kind = 'bookmark' AND is_favorite = TRUE",
        )
        .await?;

        db.execute_unprepared("DROP INDEX IF EXISTS markers_user_favorite_idx")
            .await?;
        db.execute_unprepared("ALTER TABLE markers DROP COLUMN is_favorite")
            .await?;

        Ok(())
    }
}
