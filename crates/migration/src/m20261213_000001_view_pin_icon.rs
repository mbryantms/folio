//! Per-user-per-view icon override.
//!
//! Adds `icon TEXT NULL` to `user_view_pins` so users can pick the icon
//! that appears on each rail header (home page) and in the sidebar
//! "Saved views" section. NULL falls back to a kind-based default
//! (Sparkles for system, Filter for filter_series, ListOrdered for cbl)
//! resolved client-side.
//!
//! Stored per-user-per-view because:
//!   - For system views (`user_id IS NULL`), users can't edit the row
//!     itself, but they can still want a custom icon on *their* home.
//!   - For user-authored views, the same user might want different
//!     icons on home vs. sidebar — but until a use case appears we
//!     share one icon across both surfaces.
//!
//! No CHECK constraint on the value; the client validates against a
//! known-keys registry (`web/components/library/rail-icons.ts`). Unknown
//! keys silently fall back to the kind default — same forward-compat
//! pattern as the saved-view kind discriminator.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum UserViewPins {
    Table,
    Icon,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UserViewPins::Table)
                    .add_column(ColumnDef::new(UserViewPins::Icon).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UserViewPins::Table)
                    .drop_column(UserViewPins::Icon)
                    .to_owned(),
            )
            .await
    }
}
