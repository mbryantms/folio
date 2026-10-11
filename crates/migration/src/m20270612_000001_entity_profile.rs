//! `entity_profile` — provider-filled profile of a character / team /
//! location / story arc (the "wiki" block of an entity landing page).
//!
//! One row per `(entity_type, entity_id)`. The entity row itself keeps
//! the short fields the rest of the app already reads (`description`,
//! `image_url`, `aliases`, `real_name`); this table carries what only
//! the landing page shows: the long summary, the wiki sections split out
//! of ComicVine's description, the fact sheet (creators, powers, teams,
//! universes, first appearance, …) and each provider's appearance count
//! for the "you own N of M" line. `requested_at` / `fetched_at` /
//! `error` are the lazy-fetch state: a page view queues the job once,
//! the job stamps the result or the failure.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum EntityProfile {
    Table,
    EntityType,
    EntityId,
    Summary,
    Sections,
    Facts,
    ProviderCounts,
    Sources,
    RequestedAt,
    FetchedAt,
    Error,
    UpdatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(EntityProfile::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(EntityProfile::EntityType).text().not_null())
                    .col(ColumnDef::new(EntityProfile::EntityId).uuid().not_null())
                    .col(ColumnDef::new(EntityProfile::Summary).text().null())
                    .col(ColumnDef::new(EntityProfile::Sections).json_binary().null())
                    .col(ColumnDef::new(EntityProfile::Facts).json_binary().null())
                    .col(
                        ColumnDef::new(EntityProfile::ProviderCounts)
                            .json_binary()
                            .null(),
                    )
                    .col(ColumnDef::new(EntityProfile::Sources).json_binary().null())
                    .col(
                        ColumnDef::new(EntityProfile::RequestedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(EntityProfile::FetchedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(ColumnDef::new(EntityProfile::Error).text().null())
                    .col(
                        ColumnDef::new(EntityProfile::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .primary_key(
                        Index::create()
                            .col(EntityProfile::EntityType)
                            .col(EntityProfile::EntityId),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(EntityProfile::Table).to_owned())
            .await
    }
}
