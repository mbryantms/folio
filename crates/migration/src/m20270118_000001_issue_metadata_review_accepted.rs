//! "Mark metadata complete" escape hatch (metadata-at-scale B4).
//!
//! `metadata_completeness` is computed per-request from field presence, so an
//! issue whose provider record is genuinely thin (or has no provider data at
//! all) sits in the `needs_metadata` worklist forever. These columns record an
//! operator's reversible "this is as complete as it'll get" acknowledgement;
//! the DTO/filter layer then reports such an issue as `accepted` instead of
//! `needs_metadata`. Nothing about field presence changes — the detail view
//! still lists the real gaps — so this never pollutes `field_provenance`.
//!
//! Both nullable: NULL = not acknowledged (the default + un-accept state).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Issues {
    Table,
    MetadataReviewAcceptedAt,
    MetadataReviewAcceptedBy,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Issues::Table)
                    .add_column(
                        ColumnDef::new(Issues::MetadataReviewAcceptedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(Issues::MetadataReviewAcceptedBy)
                            .uuid()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Issues::Table)
                    .drop_column(Issues::MetadataReviewAcceptedAt)
                    .drop_column(Issues::MetadataReviewAcceptedBy)
                    .to_owned(),
            )
            .await
    }
}
