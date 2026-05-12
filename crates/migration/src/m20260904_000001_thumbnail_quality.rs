//! Per-library thumbnail encoder quality.
//!
//! Adds separate admin-controlled quality sliders for cover thumbnails and
//! reader page-strip thumbnails. Defaults match the previous hard-coded
//! encoder values.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Libraries {
    Table,
    ThumbnailCoverQuality,
    ThumbnailPageQuality,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Libraries::Table)
                    .add_column(
                        ColumnDef::new(Libraries::ThumbnailCoverQuality)
                            .integer()
                            .not_null()
                            .default(80),
                    )
                    .add_column(
                        ColumnDef::new(Libraries::ThumbnailPageQuality)
                            .integer()
                            .not_null()
                            .default(50),
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
                    .drop_column(Libraries::ThumbnailCoverQuality)
                    .drop_column(Libraries::ThumbnailPageQuality)
                    .to_owned(),
            )
            .await
    }
}
