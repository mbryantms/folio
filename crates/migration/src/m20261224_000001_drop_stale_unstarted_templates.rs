//! Drop the two unused built-in filter templates: **Unstarted** (id
//! `…0004`, formerly "Want to Read" before the M3 collections rename)
//! and **Stale** (id `…0005`). Both were seeded by
//! [`m20261207_000001_built_in_templates`] but never gained user
//! traction — Unstarted is awkward next to the per-user "Want to Read"
//! collection (`kind='collection'`, `system_key='want_to_read'`) that
//! every account auto-seeds, and Stale is a curator tool that belongs
//! in admin tooling, not a user-facing rail. Removing them cleans up
//! the `/settings/views` catalog.
//!
//! `user_view_pins.saved_view_id` is `ON DELETE CASCADE`, so any pin
//! rows that referenced these views vanish with the parent. Sidebar
//! entries (`user_sidebar_entries.ref_id` when `kind='view'`) are
//! not FK-enforced (the `ref_id` column is TEXT — built-in keys and
//! library/view UUIDs share one column); we sweep them in a
//! follow-up DELETE for cleanliness — a dangling sidebar entry would
//! render as a missing row otherwise.
//!
//! `down` re-seeds both rows verbatim from the M9 templates so a
//! rollback restores the prior state.

use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

const UNSTARTED_ID: &str = "00000000-0000-0000-0000-000000000004";
const STALE_ID: &str = "00000000-0000-0000-0000-000000000005";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let conn = manager.get_connection();

        // Sweep sidebar entries pointing at these views — no FK, so
        // they'd otherwise become dangling references. `ref_id` is
        // TEXT (built-in keys share the column with UUID-keyed views),
        // so compare as strings.
        conn.execute_raw(Statement::from_sql_and_values(
            backend,
            "DELETE FROM user_sidebar_entries \
             WHERE kind = 'view' AND ref_id = ANY($1::text[])",
            [vec![UNSTARTED_ID.to_string(), STALE_ID.to_string()].into()],
        ))
        .await?;

        // Drop the views. `user_view_pins` cascades.
        conn.execute_raw(Statement::from_sql_and_values(
            backend,
            "DELETE FROM saved_views WHERE id = ANY($1::uuid[])",
            [vec![UNSTARTED_ID.to_string(), STALE_ID.to_string()].into()],
        ))
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let conn = manager.get_connection();

        // Re-seed verbatim from the M9 template migration. `auto_pin =
        // FALSE`, `is_system` defaults from the column. Rollback only
        // restores the catalog rows; previously-pinned users won't get
        // their pin rows back (cascaded out on `up`).
        let rows: &[(&str, &str, &str, &str, &str, &str, i32)] = &[
            (
                UNSTARTED_ID,
                "Unstarted",
                "Series in your library you haven't started yet.",
                r#"[{"group_id":0,"field":"read_progress","op":"equals","value":0}]"#,
                "created_at",
                "desc",
                12,
            ),
            (
                STALE_ID,
                "Stale",
                "Series the scanner hasn't seen activity on in a while.",
                "[]",
                "updated_at",
                "asc",
                12,
            ),
        ];
        for (id, name, desc, conditions, sort_field, sort_order, limit) in rows {
            conn.execute_raw(Statement::from_sql_and_values(
                backend,
                r"INSERT INTO saved_views
                    (id, user_id, kind, name, description, custom_tags,
                     match_mode, conditions, sort_field, sort_order,
                     result_limit, auto_pin)
                  VALUES
                    ($1::uuid, NULL, 'filter_series', $2, $3, ARRAY[]::text[],
                     'all', $4::jsonb, $5, $6, $7, FALSE)
                  ON CONFLICT (id) DO NOTHING",
                [
                    (*id).into(),
                    (*name).into(),
                    (*desc).into(),
                    (*conditions).into(),
                    (*sort_field).into(),
                    (*sort_order).into(),
                    (*limit).into(),
                ],
            ))
            .await?;
        }
        Ok(())
    }
}
