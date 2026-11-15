//! "Out this week" home rail (upcoming-releases 1.0): an **optional**
//! built-in rail, like the last ones added (`m20271110_000001`).
//!
//! Seeds a `kind = 'system'` saved view (`system_key =
//! 'upcoming_this_week'`) whose endpoint (`GET /me/upcoming-this-week`)
//! lists this week's releases in the caller's series. It is
//! `auto_pin = false`: users find it under "Built-in" in the pin picker
//! and opt in. The saved-views list leaves the row out while
//! `upcoming.enabled` is off, so a server that never turned the feature on
//! never offers it. No schema change.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

/// Fixed UUID like the other built-ins so tests can look the row up and
/// `user_view_pins` rows survive redeploys.
const VIEW_ID: &str = "00000000-0000-0000-0000-000000000018";

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
                     'Out this week', \
                     'This week''s new issues in your series.', \
                     ARRAY[]::text[], 'upcoming_this_week', FALSE) \
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
