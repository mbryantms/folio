//! `saved_view_groups` + `saved_view_group_items` — per-user buckets for
//! organizing the Views index (Settings → Views).
//!
//! A user with dozens of imported CBL reading lists needs a way to sort
//! them into named categories ("Marvel events", "DC Year One", "Finished")
//! without touching the lists themselves. Groups are **per user** and
//! scoped to one index tab (`scope` ∈ `filter` / `cbl` / `collection`) so a
//! group never shows up empty on a tab it doesn't belong to. Membership
//! lives in a separate table keyed by `(user_id, view_id)` — like
//! `user_view_pins` — so shared system views can be bucketed per user and
//! a view belongs to at most one group. A membership row with a NULL
//! `group_id` pins the view's position inside the "ungrouped" remainder
//! of its `scope`, so hand-arranged order survives there too; views with
//! no row at all trail the arranged ones in sort order. Deleting a group
//! cascades its items, which simply returns those views to the ungrouped
//! section; deleting a saved view drops its membership row.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS saved_view_groups (\
                id UUID PRIMARY KEY, \
                user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE, \
                scope TEXT NOT NULL CHECK (scope IN ('filter', 'cbl', 'collection')), \
                name TEXT NOT NULL, \
                position INTEGER NOT NULL DEFAULT 0, \
                collapsed BOOLEAN NOT NULL DEFAULT false, \
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now()\
            )",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS saved_view_groups_user_scope_pos_idx \
             ON saved_view_groups (user_id, scope, position)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS saved_view_group_items (\
                user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE, \
                view_id UUID NOT NULL REFERENCES saved_views(id) ON DELETE CASCADE, \
                scope TEXT NOT NULL CHECK (scope IN ('filter', 'cbl', 'collection')), \
                group_id UUID REFERENCES saved_view_groups(id) ON DELETE CASCADE, \
                position INTEGER NOT NULL DEFAULT 0, \
                PRIMARY KEY (user_id, view_id)\
            )",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS saved_view_group_items_group_pos_idx \
             ON saved_view_group_items (group_id, position)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS saved_view_group_items_user_scope_pos_idx \
             ON saved_view_group_items (user_id, scope, position)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS saved_view_group_items")
            .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS saved_view_groups")
            .await?;
        Ok(())
    }
}
