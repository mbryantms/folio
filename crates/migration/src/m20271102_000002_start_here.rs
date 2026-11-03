//! Start Here + default pins M2 — featured reading lists + the Start Here rail.
//!
//! - `saved_views.featured_position INT NULL`: an admin-curated order for
//!   shared reading lists recommended to new readers. Only a shared
//!   (`user_id IS NULL`) CBL view can be featured (CHECK below); NULL means
//!   "not featured".
//! - Seeds the `start_here` system rail. It is NOT `auto_pin`: the lazy pin
//!   seed adds it only to a brand-new account's home page (first seed), so
//!   existing users opt in from Manage pins.

use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

pub(crate) const START_HERE_ID: &str = "00000000-0000-0000-0000-000000000040";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let conn = manager.get_connection();
        conn.execute_unprepared(
            "ALTER TABLE saved_views ADD COLUMN IF NOT EXISTS featured_position INTEGER NULL",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE saved_views DROP CONSTRAINT IF EXISTS saved_views_featured_shared_cbl",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE saved_views ADD CONSTRAINT saved_views_featured_shared_cbl \
             CHECK (featured_position IS NULL \
                    OR (user_id IS NULL AND kind = 'cbl' AND featured_position >= 0))",
        )
        .await?;
        conn.execute_raw(Statement::from_sql_and_values(
            backend,
            r"INSERT INTO saved_views
                (id, user_id, kind, name, description, custom_tags, system_key, auto_pin)
              VALUES
                ($1::uuid, NULL, 'system', $2, $3, ARRAY[]::text[], 'start_here', FALSE)
              ON CONFLICT (id) DO NOTHING",
            [
                START_HERE_ID.into(),
                "Start here".into(),
                "Reading lists picked as good first reads.".into(),
            ],
        ))
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let conn = manager.get_connection();
        // Pins cascade; sidebar entries aren't FK-enforced, sweep them.
        conn.execute_raw(Statement::from_sql_and_values(
            backend,
            "DELETE FROM user_sidebar_entries WHERE kind = 'view' AND ref_id = $1",
            [START_HERE_ID.into()],
        ))
        .await?;
        conn.execute_raw(Statement::from_sql_and_values(
            backend,
            "DELETE FROM saved_views WHERE id = $1::uuid",
            [START_HERE_ID.into()],
        ))
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE saved_views DROP CONSTRAINT IF EXISTS saved_views_featured_shared_cbl",
        )
        .await?;
        conn.execute_unprepared("ALTER TABLE saved_views DROP COLUMN IF EXISTS featured_position")
            .await?;
        Ok(())
    }
}
