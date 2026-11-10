//! "New in series you're reading" home rail — an **optional** built-in
//! rail, like the last ones added (Similar series in `m20270504`, Random
//! picks / On this day / Recently read in `m20271020_000001`).
//!
//! Seeds a `kind = 'system'` saved view (`system_key = 'new_in_series'`)
//! whose endpoint (`GET /me/new-in-series`) lists issues added in the last
//! 30 days to series the caller has started and not finished. It is
//! `auto_pin = false`: existing and new users find it under "Built-in" in
//! the pin picker and opt in. No schema change and no index — the query
//! rides `issues_active_created_idx` and the `progress_records` primary
//! key.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

/// Fixed UUID like the other built-ins so tests can look the row up and
/// `user_view_pins` rows survive redeploys.
const VIEW_ID: &str = "00000000-0000-0000-0000-000000000017";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(&format!(
                "INSERT INTO saved_views \
                    (id, user_id, kind, name, description, custom_tags, system_key, auto_pin) \
                 VALUES \
                    ('{VIEW_ID}'::uuid, NULL, 'system', \
                     'New in series you''re reading', \
                     'Issues added in the last 30 days to series you''ve started, that you haven''t finished.', \
                     ARRAY[]::text[], 'new_in_series', FALSE) \
                 ON CONFLICT (id) DO NOTHING"
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        // Pins cascade via fk_user_view_pins_view; sidebar entries aren't
        // FK-enforced, so sweep them (as `m20271020_000001` does).
        conn.execute_unprepared(&format!(
            "DELETE FROM user_sidebar_entries WHERE kind = 'view' AND ref_id = '{VIEW_ID}'"
        ))
        .await?;
        conn.execute_unprepared(&format!(
            "DELETE FROM saved_views WHERE id = '{VIEW_ID}'::uuid"
        ))
        .await?;
        Ok(())
    }
}
