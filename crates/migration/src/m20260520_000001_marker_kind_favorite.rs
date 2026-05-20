//! Reintroduce `kind='favorite'` as a first-class marker kind.
//!
//! Background: m20261217_000001_marker_favorite_flag promoted favorite
//! from a kind to a boolean flag (`is_favorite`) so any marker could
//! be starred without duplicating the row. The reader chrome's star
//! button, however, ended up creating `kind='bookmark', is_favorite=true`
//! rows for previously-unbookmarked pages — meaning a starred page
//! also showed up in the bookmarks list. Users want favorites to be
//! independent of bookmarks (a favorite is no longer also a bookmark).
//!
//! This migration:
//!   1. Relaxes the `markers_kind_chk` CHECK to admit 'favorite' again.
//!   2. Backfills existing `is_favorite=true` rows into standalone
//!      `kind='favorite'` markers — preserving the original
//!      bookmark / note / highlight intact AND surfacing a clean
//!      favorite row that the new page-level star button can target.
//!
//! The `is_favorite` column is intentionally NOT dropped here. Older
//! marker-editor surfaces (where a user can star an individual
//! highlight) still set it; the favorites list query is widened to
//! treat `kind='favorite' OR is_favorite=true` as "favorited" so no
//! existing functionality regresses. A later cleanup migration may
//! remove the column once every surface is migrated to the kind.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared("ALTER TABLE markers DROP CONSTRAINT markers_kind_chk")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE markers ADD CONSTRAINT markers_kind_chk \
             CHECK (kind IN ('bookmark','note','favorite','highlight'))",
        )
        .await?;

        // Backfill: every existing `is_favorite=true` marker gets a
        // companion `kind='favorite'` row at the same page. The
        // original row is left untouched so its bookmark / note /
        // highlight identity survives. New favorites going forward
        // are standalone — they don't carry a parent kind anymore.
        //
        // `gen_random_uuid()` mints UUIDv4 from `pgcrypto`, enabled in
        // the extensions migration. Backfilled rows don't need the
        // v7 monotonicity property that user-issued markers have —
        // they're frozen historical companions, not navigation
        // anchors.
        //
        // `markers_user_favorite_idx` partial index from the original
        // flag migration stays — it's keyed on `is_favorite`, doesn't
        // interfere with the kind-based filter going forward.
        db.execute_unprepared(
            "INSERT INTO markers ( \
                id, user_id, series_id, issue_id, page_index, \
                kind, is_favorite, tags, region, selection, \
                body, color, created_at, updated_at \
             ) \
             SELECT \
                gen_random_uuid(), user_id, series_id, issue_id, page_index, \
                'favorite', FALSE, ARRAY[]::text[], NULL, NULL, \
                NULL, color, created_at, updated_at \
             FROM markers \
             WHERE is_favorite = TRUE",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Remove the backfilled standalone favorite rows. We can't
        // perfectly identify them post-hoc (gen_random_uuid leaves no
        // breadcrumb), so DROP all kind='favorite' rows on rollback.
        // Anyone who created favorites between this migration and
        // rollback loses them; acceptable for a defensive `down`.
        db.execute_unprepared("DELETE FROM markers WHERE kind = 'favorite'")
            .await?;

        db.execute_unprepared("ALTER TABLE markers DROP CONSTRAINT markers_kind_chk")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE markers ADD CONSTRAINT markers_kind_chk \
             CHECK (kind IN ('bookmark','note','highlight'))",
        )
        .await?;

        Ok(())
    }
}
