//! Per-library thumbnail settings: enable/disable + format selector.
//!
//! Adds two columns to `libraries`:
//!   - `thumbnails_enabled` (bool, default true) — when false, the post-scan
//!     worker skips enqueueing for this library and the catchup sweep ignores
//!     it. Existing on-disk thumbnails keep serving.
//!   - `thumbnail_format` (text, default "webp") — `webp` | `jpeg` | `png`.
//!     Changing the format does not auto-regenerate; admins use force-recreate
//!     on the thumbnails tab when they want to apply.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Libraries {
    Table,
    ThumbnailsEnabled,
    ThumbnailFormat,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Libraries::Table)
                    .add_column(
                        ColumnDef::new(Libraries::ThumbnailsEnabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .add_column(
                        ColumnDef::new(Libraries::ThumbnailFormat)
                            .text()
                            .not_null()
                            .default("webp"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Libraries::Table)
                    .drop_column(Libraries::ThumbnailsEnabled)
                    .drop_column(Libraries::ThumbnailFormat)
                    .to_owned(),
            )
            .await
    }
}
