//! `series_relationship_suggestion.status` gains `stale` (WP-7.3).
//!
//! A pending suggestion the engine no longer produces for its library (the
//! evidence went away: a series was renamed, character data was cleaned up,
//! a heuristic was tightened, or an admin linked the pair by hand) is marked
//! `stale` by the next run instead of sitting in the review queue forever.
//! A later run that produces it again flips it back to `pending`.
//!
//! `stale` is still not a review: it is not rejection memory, rows are still
//! never deleted, and the review list hides stale rows unless asked for them.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE series_relationship_suggestion \
               DROP CONSTRAINT IF EXISTS series_relationship_suggestion_status_chk",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE series_relationship_suggestion \
               ADD CONSTRAINT series_relationship_suggestion_status_chk \
               CHECK (status IN ('pending', 'accepted', 'rejected', 'modified', 'stale'))",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // Stale rows were pending before; put them back so the old CHECK
        // holds.
        db.execute_unprepared(
            "UPDATE series_relationship_suggestion SET status = 'pending' WHERE status = 'stale'",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE series_relationship_suggestion \
               DROP CONSTRAINT IF EXISTS series_relationship_suggestion_status_chk",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE series_relationship_suggestion \
               ADD CONSTRAINT series_relationship_suggestion_status_chk \
               CHECK (status IN ('pending', 'accepted', 'rejected', 'modified'))",
        )
        .await?;
        Ok(())
    }
}
