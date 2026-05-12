//! Markers + Collections follow-up: per-marker freeform tags.
//!
//! Lets users group markers across kinds and series — e.g. tag
//! highlights with "panel-art", notes with "thread-idea", bookmarks
//! with "reread". Tags are per-user (the marker row already carries
//! `user_id`), freeform strings, populated from an autocomplete
//! sourced from the user's existing tag set.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // `TEXT[] NOT NULL DEFAULT '{}'` — empty array, not NULL, so
        // filter queries can use `tags @> ARRAY[...]` / `tags && ...`
        // without null-checks.
        db.execute_unprepared("ALTER TABLE markers ADD COLUMN tags TEXT[] NOT NULL DEFAULT '{}'")
            .await?;

        // GIN index supports both AND (`@>`) and OR (`&&`) tag-filter
        // semantics and the distinct-tag rollup on `GET /me/markers/tags`.
        // GIN doesn't have a default operator class for UUID, so we
        // keep this as a tags-only GIN and rely on the existing
        // (user_id, kind, updated_at) btree to scope the user. The
        // planner intersects the two when the user has many markers.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS markers_tags_gin_idx \
             ON markers USING GIN (tags)",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS markers_tags_gin_idx")
            .await?;
        db.execute_unprepared("ALTER TABLE markers DROP COLUMN tags")
            .await?;
        Ok(())
    }
}
