//! Cover colour for library covers (roadmap WP-9.7).
//!
//! The web app shows a cover's average colour as its placeholder and
//! washes a hovered card in its most vibrant one. Until now the browser
//! worked both out from the decoded image and kept them per device, so a
//! cover seen for the first time had neither. `issues.cover_tone` holds
//! the pair (twelve hex digits, `metadata::cover_tone`) so every card
//! list can send it with the cover.
//!
//! On `issues`, not `issue_cover`: the card lists select `IssueCardRow`
//! straight from `issues`, and the colour is of the cover the issue
//! *displays* (a chosen cover page included), which is the thumbnail
//! job's output, not the archive page the `issue_cover` hashes describe.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE issues ADD COLUMN IF NOT EXISTS cover_tone TEXT")
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE issues DROP COLUMN IF EXISTS cover_tone")
            .await?;
        Ok(())
    }
}
