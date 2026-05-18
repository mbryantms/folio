//! Saved smart views — characters / teams / locations junctions.
//!
//! Mirrors [`m20261203_000001_metadata_junctions`](crate::m20261203_000001_metadata_junctions)
//! for three more ComicInfo fields. Each gets two junction tables:
//!
//! - `issue_{name}` — per-issue rows written by the scanner's
//!   `replace_issue_metadata` when ingesting/refreshing the underlying
//!   CBZ's ComicInfo. The CSV columns on `issues` (`characters` /
//!   `teams` / `locations`) stay as the source-of-truth, same pattern
//!   as `issues.genre` / `issues.tags`.
//! - `series_{name}` — per-series rollup, recomputed in
//!   `rollup_series_metadata` as the DISTINCT union of each series's
//!   active issues' rows. Drives `Field::Characters` /
//!   `Field::Teams` / `Field::Locations` in the saved-views filter
//!   compiler via `EXISTS` joins.
//!
//! No backfill: existing libraries get their junctions populated on
//! next scan. Until then, saved-view conditions on these three fields
//! match no series.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Series {
    Table,
    Id,
}

#[derive(Iden)]
enum Issues {
    Table,
    Id,
}

#[derive(Iden)]
enum SeriesCharacters {
    Table,
    SeriesId,
    Character,
}

#[derive(Iden)]
enum SeriesTeams {
    Table,
    SeriesId,
    Team,
}

#[derive(Iden)]
enum SeriesLocations {
    Table,
    SeriesId,
    Location,
}

#[derive(Iden)]
enum IssueCharacters {
    Table,
    IssueId,
    Character,
}

#[derive(Iden)]
enum IssueTeams {
    Table,
    IssueId,
    Team,
}

#[derive(Iden)]
enum IssueLocations {
    Table,
    IssueId,
    Location,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ───── series_characters ─────
        manager
            .create_table(
                Table::create()
                    .table(SeriesCharacters::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(SeriesCharacters::SeriesId).uuid().not_null())
                    .col(
                        ColumnDef::new(SeriesCharacters::Character)
                            .text()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(SeriesCharacters::SeriesId)
                            .col(SeriesCharacters::Character),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_series_characters_series")
                            .from(SeriesCharacters::Table, SeriesCharacters::SeriesId)
                            .to(Series::Table, Series::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("series_characters_character_idx")
                    .table(SeriesCharacters::Table)
                    .col(SeriesCharacters::Character)
                    .to_owned(),
            )
            .await?;

        // ───── series_teams ─────
        manager
            .create_table(
                Table::create()
                    .table(SeriesTeams::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(SeriesTeams::SeriesId).uuid().not_null())
                    .col(ColumnDef::new(SeriesTeams::Team).text().not_null())
                    .primary_key(
                        Index::create()
                            .col(SeriesTeams::SeriesId)
                            .col(SeriesTeams::Team),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_series_teams_series")
                            .from(SeriesTeams::Table, SeriesTeams::SeriesId)
                            .to(Series::Table, Series::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("series_teams_team_idx")
                    .table(SeriesTeams::Table)
                    .col(SeriesTeams::Team)
                    .to_owned(),
            )
            .await?;

        // ───── series_locations ─────
        manager
            .create_table(
                Table::create()
                    .table(SeriesLocations::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(SeriesLocations::SeriesId).uuid().not_null())
                    .col(ColumnDef::new(SeriesLocations::Location).text().not_null())
                    .primary_key(
                        Index::create()
                            .col(SeriesLocations::SeriesId)
                            .col(SeriesLocations::Location),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_series_locations_series")
                            .from(SeriesLocations::Table, SeriesLocations::SeriesId)
                            .to(Series::Table, Series::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("series_locations_location_idx")
                    .table(SeriesLocations::Table)
                    .col(SeriesLocations::Location)
                    .to_owned(),
            )
            .await?;

        // ───── issue_characters ─────
        manager
            .create_table(
                Table::create()
                    .table(IssueCharacters::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(IssueCharacters::IssueId).text().not_null())
                    .col(ColumnDef::new(IssueCharacters::Character).text().not_null())
                    .primary_key(
                        Index::create()
                            .col(IssueCharacters::IssueId)
                            .col(IssueCharacters::Character),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_issue_characters_issue")
                            .from(IssueCharacters::Table, IssueCharacters::IssueId)
                            .to(Issues::Table, Issues::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("issue_characters_character_idx")
                    .table(IssueCharacters::Table)
                    .col(IssueCharacters::Character)
                    .to_owned(),
            )
            .await?;

        // ───── issue_teams ─────
        manager
            .create_table(
                Table::create()
                    .table(IssueTeams::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(IssueTeams::IssueId).text().not_null())
                    .col(ColumnDef::new(IssueTeams::Team).text().not_null())
                    .primary_key(
                        Index::create()
                            .col(IssueTeams::IssueId)
                            .col(IssueTeams::Team),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_issue_teams_issue")
                            .from(IssueTeams::Table, IssueTeams::IssueId)
                            .to(Issues::Table, Issues::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("issue_teams_team_idx")
                    .table(IssueTeams::Table)
                    .col(IssueTeams::Team)
                    .to_owned(),
            )
            .await?;

        // ───── issue_locations ─────
        manager
            .create_table(
                Table::create()
                    .table(IssueLocations::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(IssueLocations::IssueId).text().not_null())
                    .col(ColumnDef::new(IssueLocations::Location).text().not_null())
                    .primary_key(
                        Index::create()
                            .col(IssueLocations::IssueId)
                            .col(IssueLocations::Location),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_issue_locations_issue")
                            .from(IssueLocations::Table, IssueLocations::IssueId)
                            .to(Issues::Table, Issues::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("issue_locations_location_idx")
                    .table(IssueLocations::Table)
                    .col(IssueLocations::Location)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(IssueLocations::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(IssueTeams::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(IssueCharacters::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(SeriesLocations::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(SeriesTeams::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(SeriesCharacters::Table).to_owned())
            .await?;
        Ok(())
    }
}
