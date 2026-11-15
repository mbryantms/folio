//! Cover colour for upcoming releases (roadmap WP-9.6).
//!
//! An upcoming release's cover is a hotlink to Metron's image host,
//! which sends no CORS headers, so the browser cannot read its pixels
//! and the card had no colour to answer in. The calendar sweep reads
//! each cover once and stores its two colours here:
//!
//! - `cover_tone`: twelve hex digits, the average colour then the most
//!   vibrant one (`metadata::cover_tone`), the same encoding the web
//!   app's own cache uses.
//! - `cover_tone_url`: the `image_url` those colours were read from. A
//!   release whose image changes is read again.
//! - `cover_tone_checked_at`: the last attempt, so a cover that cannot
//!   be fetched or decoded is retried once a day, not every minute.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE release_calendar_issue
                    ADD COLUMN IF NOT EXISTS cover_tone TEXT,
                    ADD COLUMN IF NOT EXISTS cover_tone_url TEXT,
                    ADD COLUMN IF NOT EXISTS cover_tone_checked_at TIMESTAMPTZ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE release_calendar_issue
                    DROP COLUMN IF EXISTS cover_tone,
                    DROP COLUMN IF EXISTS cover_tone_url,
                    DROP COLUMN IF EXISTS cover_tone_checked_at",
            )
            .await?;
        Ok(())
    }
}
