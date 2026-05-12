//! Per-library opt-in for auto-generating page thumbnails on scan.
//!
//! Cover thumbnails are always generated post-scan (cheap, one image per
//! issue). Page-strip thumbnails are pricier (one image per page, dozens
//! per issue) and were previously only generated on demand via the
//! "Queue page maps" admin button. This flag lets a library opt into
//! auto-generation as part of the post-scan pipeline.
//!
//! Default `false` so existing libraries keep their current behavior.
//! New libraries can opt in via the create-library dialog; existing ones
//! can flip the toggle from the library settings page.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Libraries {
    Table,
    GeneratePageThumbsOnScan,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Libraries::Table)
                    .add_column(
                        ColumnDef::new(Libraries::GeneratePageThumbsOnScan)
                            .boolean()
                            .not_null()
                            .default(false),
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
                    .drop_column(Libraries::GeneratePageThumbsOnScan)
                    .to_owned(),
            )
            .await
    }
}
