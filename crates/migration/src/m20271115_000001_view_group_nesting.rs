//! Nested view groups (folders inside folders on the reading lists page).
//!
//! - `saved_view_groups.parent_id UUID NULL` — the enclosing group, NULL
//!   for a top-level group. `position` becomes the ordinal among siblings
//!   (same user, scope and parent). Every existing row is top-level and
//!   keeps its position: positions were dense per `(user, scope)`, which
//!   is exactly "dense among the top-level siblings".
//! - `saved_view_groups_id_user_scope_key` — UNIQUE `(id, user_id, scope)`,
//!   the target of the composite foreign key below (`id` alone is already
//!   unique, so this never refuses a row).
//! - `saved_view_groups_parent_fkey` — `(parent_id, user_id, scope)` →
//!   `(id, user_id, scope)`, so the database itself refuses a parent that
//!   belongs to another user or another scope. `ON DELETE NO ACTION`,
//!   deliberately: deleting a group that still has child groups fails
//!   instead of cascading away a subtree or leaving orphans behind. The
//!   delete handler moves the children up one level first, in the same
//!   transaction. NO ACTION (not RESTRICT) is checked at the end of the
//!   statement, so deleting a user — which cascades to all of their
//!   groups in one statement — removes a whole tree without tripping it.
//! - `saved_view_groups_parent_not_self` — CHECK `parent_id <> id`.
//! - `saved_view_groups_parent_idx` — the foreign key's referencing
//!   side, for the lookup a parent delete runs.
//!
//! Depth (at most four levels) and cycles are enforced by the API under
//! a per-`(user, scope)` advisory lock; see `api::view_groups`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE saved_view_groups ADD COLUMN IF NOT EXISTS parent_id UUID NULL",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE saved_view_groups \
             DROP CONSTRAINT IF EXISTS saved_view_groups_id_user_scope_key",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE saved_view_groups \
             ADD CONSTRAINT saved_view_groups_id_user_scope_key UNIQUE (id, user_id, scope)",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE saved_view_groups \
             DROP CONSTRAINT IF EXISTS saved_view_groups_parent_fkey",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE saved_view_groups \
             ADD CONSTRAINT saved_view_groups_parent_fkey \
             FOREIGN KEY (parent_id, user_id, scope) \
             REFERENCES saved_view_groups (id, user_id, scope) \
             ON DELETE NO ACTION",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE saved_view_groups \
             DROP CONSTRAINT IF EXISTS saved_view_groups_parent_not_self",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE saved_view_groups \
             ADD CONSTRAINT saved_view_groups_parent_not_self CHECK (parent_id <> id)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS saved_view_groups_parent_idx \
             ON saved_view_groups (parent_id) WHERE parent_id IS NOT NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS saved_view_groups_parent_idx")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE saved_view_groups \
             DROP CONSTRAINT IF EXISTS saved_view_groups_parent_not_self",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE saved_view_groups \
             DROP CONSTRAINT IF EXISTS saved_view_groups_parent_fkey",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE saved_view_groups \
             DROP CONSTRAINT IF EXISTS saved_view_groups_id_user_scope_key",
        )
        .await?;
        // Flattening loses the nesting, not the groups: every group
        // survives as a top-level one, numbered in tree order (a parent,
        // then its subtree) so the flat board reads as the tree did.
        db.execute_unprepared(
            "WITH RECURSIVE t AS ( \
                 SELECT id, user_id, scope, \
                        lpad(position::text, 10, '0') || id::text AS path \
                 FROM saved_view_groups WHERE parent_id IS NULL \
               UNION ALL \
                 SELECT g.id, g.user_id, g.scope, \
                        t.path || '/' || lpad(g.position::text, 10, '0') || g.id::text \
                 FROM saved_view_groups g JOIN t ON g.parent_id = t.id \
             ), numbered AS ( \
                 SELECT id, (row_number() OVER (PARTITION BY user_id, scope ORDER BY path) - 1)::int AS pos \
                 FROM t \
             ) \
             UPDATE saved_view_groups s SET position = numbered.pos \
             FROM numbered WHERE s.id = numbered.id",
        )
        .await?;
        db.execute_unprepared("ALTER TABLE saved_view_groups DROP COLUMN IF EXISTS parent_id")
            .await?;
        Ok(())
    }
}
