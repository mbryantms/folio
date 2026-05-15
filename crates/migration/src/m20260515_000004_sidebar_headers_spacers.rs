//! Sidebar customization — custom headers + spacers.
//!
//! Today's sidebar groups entries by `kind` client-side, which produces
//! duplicated section labels when the user interleaves kinds (e.g. a
//! library appears between two saved views → "Libraries"/"Saved views"/
//! "Libraries" stacks). The fix:
//!
//!   1. Lift section labels out of the client and into stored data.
//!      Add two new discriminator values to `user_sidebar_entries.kind`:
//!      `'header'` (a labelled section title row) and `'spacer'` (a
//!      visual gap with no label).
//!   2. Allow a free-form `label` override on every entry so users can
//!      rename built-in section titles or label their custom headers.
//!      `label` is required for `kind = 'header'`; ignored for
//!      `kind = 'spacer'`; optional override for other kinds (the
//!      server falls back to the resolved label when null).
//!
//! Headers and spacers carry a synthetic `ref_id` (the client generates
//! a UUID on insert) so the composite PK `(user_id, kind, ref_id)`
//! stays tight without a separate auto-increment column.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum UserSidebarEntries {
    Table,
    Label,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UserSidebarEntries::Table)
                    .add_column(ColumnDef::new(UserSidebarEntries::Label).text().null())
                    .to_owned(),
            )
            .await?;
        let conn = manager.get_connection();
        conn.execute_unprepared(
            "ALTER TABLE user_sidebar_entries \
             DROP CONSTRAINT IF EXISTS user_sidebar_entries_kind_chk",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE user_sidebar_entries \
             ADD CONSTRAINT user_sidebar_entries_kind_chk \
             CHECK (kind IN ('builtin', 'library', 'view', 'page', 'header', 'spacer'))",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        // Drop any rows that use the new kinds — leaving them would
        // produce stale references after the constraint tightens.
        conn.execute_unprepared(
            "DELETE FROM user_sidebar_entries WHERE kind IN ('header', 'spacer')",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE user_sidebar_entries \
             DROP CONSTRAINT IF EXISTS user_sidebar_entries_kind_chk",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE user_sidebar_entries \
             ADD CONSTRAINT user_sidebar_entries_kind_chk \
             CHECK (kind IN ('builtin', 'library', 'view', 'page'))",
        )
        .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(UserSidebarEntries::Table)
                    .drop_column(UserSidebarEntries::Label)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
