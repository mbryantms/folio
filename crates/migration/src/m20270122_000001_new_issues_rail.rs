//! "New issues" home rail — issue-level recently-added system view.
//!
//! The existing built-in "Recently Added" / "Recently Updated" rails are
//! `filter_series` views: they sort *series* and render one series cover
//! each, so a freshly-ingested issue only surfaces indirectly (its series
//! bubbles up). This migration seeds a `kind = 'system'` rail
//! (`system_key = 'new_issues'`) whose endpoint (`GET /me/recent-issues`)
//! lists the newest *issues* across the library, reverse-chronological by
//! ingest time with a per-series cap so one big import can't flood the
//! rail. `auto_pin = true` → the lazy pin seed in `saved_views::list`
//! rolls it out to existing users on their next home-page visit.
//!
//! With an issue-level "New issues" rail alongside them, the two old
//! rails' names turn ambiguous (added *what*?), so they're renamed to
//! say what they list ("Recently added series" / "Recently updated
//! series"). Guarded on the current name, mirroring
//! `m20261215_000003_rename_unstarted_template`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        // Fixed UUID like the other built-ins so tests can look the row
        // up without races and `user_view_pins` rows survive redeploys.
        conn.execute_unprepared(
            "INSERT INTO saved_views \
                (id, user_id, kind, name, description, custom_tags, system_key, auto_pin) \
             VALUES \
                ('00000000-0000-0000-0000-000000000012'::uuid, NULL, 'system', \
                 'New issues', 'The latest issues added to your library.', \
                 ARRAY[]::text[], 'new_issues', TRUE) \
             ON CONFLICT (id) DO NOTHING",
        )
        .await?;

        conn.execute_unprepared(
            "UPDATE saved_views SET name = 'Recently added series' \
             WHERE id = '00000000-0000-0000-0000-000000000001' AND name = 'Recently Added'",
        )
        .await?;
        conn.execute_unprepared(
            "UPDATE saved_views SET name = 'Recently updated series' \
             WHERE id = '00000000-0000-0000-0000-000000000002' AND name = 'Recently Updated'",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        // Pins cascade via fk_user_view_pins_view.
        conn.execute_unprepared(
            "DELETE FROM saved_views WHERE id = '00000000-0000-0000-0000-000000000012'::uuid",
        )
        .await?;
        conn.execute_unprepared(
            "UPDATE saved_views SET name = 'Recently Added' \
             WHERE id = '00000000-0000-0000-0000-000000000001' AND name = 'Recently added series'",
        )
        .await?;
        conn.execute_unprepared(
            "UPDATE saved_views SET name = 'Recently Updated' \
             WHERE id = '00000000-0000-0000-0000-000000000002' AND name = 'Recently updated series'",
        )
        .await?;
        Ok(())
    }
}
