//! Replace `saved_views.cbl_list_id` FK action from `SET NULL` to `CASCADE`.
//!
//! The original `SET NULL` action collided with the `saved_views_kind_chk`
//! CHECK constraint (`kind = 'cbl' ↔ cbl_list_id IS NOT NULL`): when a user
//! deleted a CBL list, the cascade tried to NULL out the linked saved
//! view's `cbl_list_id`, which the CHECK rejected, aborting the whole
//! DELETE. The cbl_lists row stayed in place, and each subsequent
//! "remove + re-add" cycle silently stacked another cbl_lists row, which
//! the On Deck rail surfaced as a separate `CblNext` card — the
//! "duplicate issue in On Deck" bug observed 2026-05-14.
//!
//! A `kind='cbl'` saved view is just a thin wrapper around the underlying
//! list, so cascading the delete is the right semantic: if the list is
//! gone, the saved view has nothing to render.
//!
//! Postgres-only migration: sea-orm's portable `alter_table` doesn't
//! expose "drop FK by name + recreate" in a single atomic step, so we
//! issue the raw SQL directly.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE saved_views DROP CONSTRAINT IF EXISTS fk_saved_views_cbl_list;
                ALTER TABLE saved_views
                    ADD CONSTRAINT fk_saved_views_cbl_list
                    FOREIGN KEY (cbl_list_id) REFERENCES cbl_lists(id)
                    ON DELETE CASCADE;
                "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE saved_views DROP CONSTRAINT IF EXISTS fk_saved_views_cbl_list;
                ALTER TABLE saved_views
                    ADD CONSTRAINT fk_saved_views_cbl_list
                    FOREIGN KEY (cbl_list_id) REFERENCES cbl_lists(id)
                    ON DELETE SET NULL;
                "#,
            )
            .await?;
        Ok(())
    }
}
