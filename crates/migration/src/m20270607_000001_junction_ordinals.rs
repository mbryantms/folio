//! Stored position on the list junctions (junctions-feed-issue-page).
//!
//! `issue_credits` already carries `ordinal`. The other per-issue lists —
//! characters, teams, locations, genres, tags — had no order of their own,
//! so once the CSV read-cache columns are rebuilt from the junctions
//! (`writers::rebuild_series_issue_csv_cache`) they could only be
//! alphabetized, and a sidecar rewrite would reorder a file's
//! `<Characters>` / `<Genre>` lists. `ordinal` is the position within the
//! source list (the ComicInfo CSV for scanner writes, the provider's order
//! for applies); the rebuild and the issue detail endpoint order by it.
//! Existing rows default to 0 and fall back to name order until their
//! issue is next written.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

const TABLES: [&str; 5] = [
    "issue_characters",
    "issue_teams",
    "issue_locations",
    "issue_genres",
    "issue_tags",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for t in TABLES {
            conn.execute_unprepared(&format!(
                "ALTER TABLE {t} ADD COLUMN IF NOT EXISTS ordinal integer NOT NULL DEFAULT 0"
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for t in TABLES {
            conn.execute_unprepared(&format!("ALTER TABLE {t} DROP COLUMN IF EXISTS ordinal"))
                .await?;
        }
        Ok(())
    }
}
