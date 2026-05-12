//! Continue Reading + On Deck home rails — system-view discriminator.
//!
//! Adds a new `kind = 'system'` discriminator to `saved_views` plus a
//! `system_key` column that identifies which built-in rail a row represents
//! (`'continue_reading'`, `'on_deck'`). System rows are admin/global
//! (`user_id IS NULL`) and surface on the home page through the existing
//! lazy pin seed in `saved_views::list` (any system view with
//! `auto_pin = true` lands in a fresh user's `user_view_pins` set on first
//! GET).
//!
//! The pre-existing `saved_views_kind_chk` constraint enforced strict shape
//! per kind (filter columns populated XOR `cbl_list_id` populated). We
//! relax it to permit a third kind that has neither.

use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum SavedViews {
    Table,
    SystemKey,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SavedViews::Table)
                    .add_column(ColumnDef::new(SavedViews::SystemKey).text().null())
                    .to_owned(),
            )
            .await?;

        let backend = manager.get_database_backend();
        let conn = manager.get_connection();

        // Replace the polymorphic check constraint with one that admits
        // `kind = 'system'`. System rows carry only `name`/`description`/
        // `system_key`; all other discriminator columns stay NULL.
        conn.execute_unprepared(
            "ALTER TABLE saved_views DROP CONSTRAINT IF EXISTS saved_views_kind_chk",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE saved_views ADD CONSTRAINT saved_views_kind_chk CHECK (\
                (kind = 'filter_series' AND match_mode IS NOT NULL AND conditions IS NOT NULL \
                    AND sort_field IS NOT NULL AND sort_order IS NOT NULL AND result_limit IS NOT NULL \
                    AND cbl_list_id IS NULL AND system_key IS NULL) \
                OR (kind = 'cbl' AND match_mode IS NULL AND conditions IS NULL \
                    AND sort_field IS NULL AND sort_order IS NULL AND result_limit IS NULL \
                    AND cbl_list_id IS NOT NULL AND system_key IS NULL) \
                OR (kind = 'system' AND match_mode IS NULL AND conditions IS NULL \
                    AND sort_field IS NULL AND sort_order IS NULL AND result_limit IS NULL \
                    AND cbl_list_id IS NULL AND system_key IS NOT NULL)\
             )",
        )
        .await?;

        // One row per system_key — a future ALTER might allow per-user
        // overrides, but for now system rails are admin/global and unique.
        conn.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS saved_views_system_key_uniq \
             ON saved_views(system_key) WHERE system_key IS NOT NULL",
        )
        .await?;

        // Seed the two built-in rails. Fixed UUIDs let tests look them up
        // without races and let `user_view_pins` rows survive across re-
        // deploys. `auto_pin = true` so the existing lazy-seed code in
        // `saved_views::list` pins them to every user on first touch.
        for (id, system_key, name, description) in &[
            (
                "00000000-0000-0000-0000-000000000010",
                "continue_reading",
                "Continue reading",
                "Issues you've started but haven't finished.",
            ),
            (
                "00000000-0000-0000-0000-000000000011",
                "on_deck",
                "On deck",
                "Up next in your series and reading lists.",
            ),
        ] {
            conn.execute(Statement::from_sql_and_values(
                backend,
                r"INSERT INTO saved_views
                    (id, user_id, kind, name, description, custom_tags, system_key, auto_pin)
                  VALUES
                    ($1::uuid, NULL, 'system', $2, $3, ARRAY[]::text[], $4, TRUE)
                  ON CONFLICT (id) DO NOTHING",
                [
                    (*id).into(),
                    (*name).into(),
                    (*description).into(),
                    (*system_key).into(),
                ],
            ))
            .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        // Drop seeded rows first so the constraint flip below is clean.
        conn.execute_unprepared(
            "DELETE FROM saved_views WHERE id IN (\
                '00000000-0000-0000-0000-000000000003'::uuid, \
                '00000000-0000-0000-0000-000000000004'::uuid)",
        )
        .await?;
        conn.execute_unprepared("DROP INDEX IF EXISTS saved_views_system_key_uniq")
            .await?;
        conn.execute_unprepared(
            "ALTER TABLE saved_views DROP CONSTRAINT IF EXISTS saved_views_kind_chk",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE saved_views ADD CONSTRAINT saved_views_kind_chk CHECK (\
                (kind = 'filter_series' AND match_mode IS NOT NULL AND conditions IS NOT NULL \
                    AND sort_field IS NOT NULL AND sort_order IS NOT NULL AND result_limit IS NOT NULL \
                    AND cbl_list_id IS NULL) \
                OR (kind = 'cbl' AND match_mode IS NULL AND conditions IS NULL \
                    AND sort_field IS NULL AND sort_order IS NULL AND result_limit IS NULL \
                    AND cbl_list_id IS NOT NULL)\
             )",
        )
        .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SavedViews::Table)
                    .drop_column(SavedViews::SystemKey)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
