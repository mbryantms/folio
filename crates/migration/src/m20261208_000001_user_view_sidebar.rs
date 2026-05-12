//! Saved-views polish: per-user `show_in_sidebar` flag.
//!
//! Until now every row in `user_view_pins` implicitly meant "pinned to
//! home". The user wants two independent toggles per saved view —
//! "show on home" (the existing pin) and "show in sidebar" — with any
//! combination of on/off allowed. So this migration adds a
//! `pinned BOOLEAN` plus a `show_in_sidebar BOOLEAN` to the existing
//! per-user-per-view row, and migrates the legacy semantic
//! ("row exists ⇒ pinned") by setting `pinned = TRUE` for every
//! existing row. New rows default both flags to FALSE.
//!
//! Server code now treats the table as a per-user-per-view *preference*
//! table — pin and unpin update the `pinned` column instead of
//! inserting/deleting rows; sidebar mutations work the same way.

use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum UserViewPins {
    Table,
    Pinned,
    ShowInSidebar,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UserViewPins::Table)
                    .add_column(
                        ColumnDef::new(UserViewPins::Pinned)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(UserViewPins::Table)
                    .add_column(
                        ColumnDef::new(UserViewPins::ShowInSidebar)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;

        // Existing rows were inserted to mean "pinned" — preserve that
        // by flipping the new column on for everything currently in the
        // table.
        let backend = manager.get_database_backend();
        manager
            .get_connection()
            .execute(Statement::from_string(
                backend,
                "UPDATE user_view_pins SET pinned = TRUE",
            ))
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UserViewPins::Table)
                    .drop_column(UserViewPins::ShowInSidebar)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(UserViewPins::Table)
                    .drop_column(UserViewPins::Pinned)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
