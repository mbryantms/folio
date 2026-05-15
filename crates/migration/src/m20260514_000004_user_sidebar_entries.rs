//! Navigation customization M1 — per-user sidebar layout overrides.
//!
//! Today the left-nav order is hardcoded in `web/components/library/main-nav.ts`
//! (Browse built-ins → Libraries → Saved views, each section pre-ordered)
//! and only saved views have a visibility toggle (`user_view_pins.show_in_sidebar`).
//! Users have asked to reorder and hide built-ins + libraries too.
//!
//! Schema strategy: this table stores **explicit overrides only**. A user
//! with zero rows here gets the same default layout as today. Missing
//! refs (e.g. a library the user can see but hasn't customized)
//! transparently fall back to defaults in
//! `crate::sidebar_layout::compute_layout` — that way newly-added
//! libraries or saved views auto-appear without seed migrations or
//! fan-out backfills.
//!
//! `kind` discriminates which registry `ref_id` refers to:
//!   - `'builtin'`  → fixed key in {'home','bookmarks','collections','want_to_read','all_libraries'}
//!   - `'library'`  → libraries.id
//!   - `'view'`     → saved_views.id
//!
//! `ref_id` is TEXT (not UUID) because built-in keys are short strings.
//! `position` is a global ordering across all kinds — when the user drops
//! a saved view between two built-ins, both rows persist with the
//! resulting positions, no per-section quirks.
//!
//! This migration introduces the table only. The reader
//! (`compute_layout`) and writer (`PATCH /me/sidebar-layout`) land in the
//! same milestone but in separate commits. The `user_view_pins.show_in_sidebar`
//! column stays in place for now; a follow-on migration drops it once
//! `compute_layout` is the only reader.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum UserSidebarEntries {
    Table,
    UserId,
    Kind,
    RefId,
    Visible,
    Position,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UserSidebarEntries::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(UserSidebarEntries::UserId).uuid().not_null())
                    .col(ColumnDef::new(UserSidebarEntries::Kind).text().not_null())
                    .col(ColumnDef::new(UserSidebarEntries::RefId).text().not_null())
                    .col(
                        ColumnDef::new(UserSidebarEntries::Visible)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(UserSidebarEntries::Position)
                            .integer()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(UserSidebarEntries::UserId)
                            .col(UserSidebarEntries::Kind)
                            .col(UserSidebarEntries::RefId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_sidebar_entries_user")
                            .from(UserSidebarEntries::Table, UserSidebarEntries::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Constrain `kind` to the three discriminator values so a typo on
        // the API surface fails fast at INSERT time instead of producing
        // ghost rows that the resolver silently ignores.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE user_sidebar_entries \
                 ADD CONSTRAINT user_sidebar_entries_kind_chk \
                 CHECK (kind IN ('builtin', 'library', 'view'))",
            )
            .await?;

        // Lookup-by-user is the common read pattern (compute_layout pulls
        // every row for one user). Position-ordered, so the resolver gets
        // rows pre-sorted without a Sort node.
        manager
            .create_index(
                Index::create()
                    .name("user_sidebar_entries_user_position_idx")
                    .table(UserSidebarEntries::Table)
                    .col(UserSidebarEntries::UserId)
                    .col(UserSidebarEntries::Position)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("user_sidebar_entries_user_position_idx")
                    .table(UserSidebarEntries::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(UserSidebarEntries::Table).to_owned())
            .await?;
        Ok(())
    }
}
