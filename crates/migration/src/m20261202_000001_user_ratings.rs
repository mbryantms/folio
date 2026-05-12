//! Per-user ratings on issues and series.
//!
//! Distinct from ComicInfo's `CommunityRating` (parsed into
//! `issues.community_rating`). This table holds the calling user's *own*
//! rating, surfaced in the issue and series Edit drawers via a star
//! widget. 0..=5 with half-star precision; the column is `REAL` so we can
//! pick a different scale later without a schema change.
//!
//! `target_type` is `'issue'` or `'series'`. We deliberately don't split
//! into two tables: the access pattern (look up "what did this user rate
//! across these N targets") is identical and the index covers both.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum UserRatings {
    Table,
    UserId,
    TargetType,
    TargetId,
    Rating,
    CreatedAt,
    UpdatedAt,
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
                    .table(UserRatings::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(UserRatings::UserId).uuid().not_null())
                    .col(ColumnDef::new(UserRatings::TargetType).text().not_null())
                    .col(ColumnDef::new(UserRatings::TargetId).text().not_null())
                    .col(ColumnDef::new(UserRatings::Rating).double().not_null())
                    .col(
                        ColumnDef::new(UserRatings::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserRatings::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(UserRatings::UserId)
                            .col(UserRatings::TargetType)
                            .col(UserRatings::TargetId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_ratings_user")
                            .from(UserRatings::Table, UserRatings::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Lookup index for "all of user X's ratings of type Y" — drives the
        // bulk-fetch on the series page where we want every issue's rating
        // in one round trip.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("user_ratings_by_user_type_idx")
                    .table(UserRatings::Table)
                    .col(UserRatings::UserId)
                    .col(UserRatings::TargetType)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UserRatings::Table).to_owned())
            .await
    }
}
