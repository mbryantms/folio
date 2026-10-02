//! "Similar series" home rail (WP-7.4) — an **optional** built-in rail.
//!
//! Seeds a `kind = 'system'` saved view (`system_key = 'similar_series'`)
//! whose endpoint (`GET /me/similar-series`) lists content-based
//! neighbours of the series the user read most recently ("Because you
//! read …"). Unlike Continue reading / On deck / New issues it is
//! `auto_pin = false`: it shows up under "Built-in" in the pin picker and
//! each user opts in. No schema change — the neighbour cache is
//! in-process (see `crates/server/src/similarity.rs`).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Fixed UUID like the other built-ins so tests can look the row
        // up and `user_view_pins` rows survive redeploys.
        manager
            .get_connection()
            .execute_unprepared(
                "INSERT INTO saved_views \
                    (id, user_id, kind, name, description, custom_tags, system_key, auto_pin) \
                 VALUES \
                    ('00000000-0000-0000-0000-000000000013'::uuid, NULL, 'system', \
                     'Similar series', \
                     'Series like the one you read most recently, with why they match.', \
                     ARRAY[]::text[], 'similar_series', FALSE) \
                 ON CONFLICT (id) DO NOTHING",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Pins cascade via fk_user_view_pins_view.
        manager
            .get_connection()
            .execute_unprepared(
                "DELETE FROM saved_views WHERE id = '00000000-0000-0000-0000-000000000013'::uuid",
            )
            .await?;
        Ok(())
    }
}
