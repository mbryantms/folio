//! Saved smart views — M1: normalized metadata junction tables.
//!
//! Replaces the CSV-shaped `series.genre` / `series.tags` columns and the
//! per-issue CSVs (which stay on `issues` as the parsed-from-ComicInfo source
//! of truth) with proper relations that filter views can index against.
//!
//! Six junction tables — three each at series + issue level — keyed by a
//! composite PK so dedupe is enforced by the schema. Series-level rows are
//! pure aggregations of their issue children; the scanner replaces them on
//! every series rollup. There is no admin override path: to add a genre to a
//! series, edit the underlying issues. The pre-existing `series.genre` /
//! `series.tags` override columns are dropped here.
//!
//! Credits collapse the eight ComicInfo role columns
//! (`writer / penciller / inker / colorist / letterer / cover_artist /
//! editor / translator`) into a single `(role, person)` pair so a future
//! filter view ("all series with writer X") can run against one indexed
//! table instead of eight column-per-table scans.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Series {
    Table,
    Id,
    Genre,
    Tags,
}

#[derive(Iden)]
enum Issues {
    Table,
    Id,
}

#[derive(Iden)]
enum SeriesGenres {
    Table,
    SeriesId,
    Genre,
}

#[derive(Iden)]
enum SeriesTags {
    Table,
    SeriesId,
    Tag,
}

#[derive(Iden)]
enum SeriesCredits {
    Table,
    SeriesId,
    Role,
    Person,
}

#[derive(Iden)]
enum IssueGenres {
    Table,
    IssueId,
    Genre,
}

#[derive(Iden)]
enum IssueTags {
    Table,
    IssueId,
    Tag,
}

#[derive(Iden)]
enum IssueCredits {
    Table,
    IssueId,
    Role,
    Person,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ───── series_genres ─────
        manager
            .create_table(
                Table::create()
                    .table(SeriesGenres::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(SeriesGenres::SeriesId).uuid().not_null())
                    .col(ColumnDef::new(SeriesGenres::Genre).text().not_null())
                    .primary_key(
                        Index::create()
                            .col(SeriesGenres::SeriesId)
                            .col(SeriesGenres::Genre),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_series_genres_series")
                            .from(SeriesGenres::Table, SeriesGenres::SeriesId)
                            .to(Series::Table, Series::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("series_genres_genre_idx")
                    .table(SeriesGenres::Table)
                    .col(SeriesGenres::Genre)
                    .to_owned(),
            )
            .await?;

        // ───── series_tags ─────
        manager
            .create_table(
                Table::create()
                    .table(SeriesTags::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(SeriesTags::SeriesId).uuid().not_null())
                    .col(ColumnDef::new(SeriesTags::Tag).text().not_null())
                    .primary_key(
                        Index::create()
                            .col(SeriesTags::SeriesId)
                            .col(SeriesTags::Tag),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_series_tags_series")
                            .from(SeriesTags::Table, SeriesTags::SeriesId)
                            .to(Series::Table, Series::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("series_tags_tag_idx")
                    .table(SeriesTags::Table)
                    .col(SeriesTags::Tag)
                    .to_owned(),
            )
            .await?;

        // ───── series_credits ─────
        manager
            .create_table(
                Table::create()
                    .table(SeriesCredits::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(SeriesCredits::SeriesId).uuid().not_null())
                    .col(ColumnDef::new(SeriesCredits::Role).text().not_null())
                    .col(ColumnDef::new(SeriesCredits::Person).text().not_null())
                    .primary_key(
                        Index::create()
                            .col(SeriesCredits::SeriesId)
                            .col(SeriesCredits::Role)
                            .col(SeriesCredits::Person),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_series_credits_series")
                            .from(SeriesCredits::Table, SeriesCredits::SeriesId)
                            .to(Series::Table, Series::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        // Drives "all series with writer = X" filter queries — `(role, person)`
        // is the typical predicate, so order matters.
        manager
            .create_index(
                Index::create()
                    .name("series_credits_role_person_idx")
                    .table(SeriesCredits::Table)
                    .col(SeriesCredits::Role)
                    .col(SeriesCredits::Person)
                    .to_owned(),
            )
            .await?;

        // ───── issue_genres ─────
        manager
            .create_table(
                Table::create()
                    .table(IssueGenres::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(IssueGenres::IssueId).text().not_null())
                    .col(ColumnDef::new(IssueGenres::Genre).text().not_null())
                    .primary_key(
                        Index::create()
                            .col(IssueGenres::IssueId)
                            .col(IssueGenres::Genre),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_issue_genres_issue")
                            .from(IssueGenres::Table, IssueGenres::IssueId)
                            .to(Issues::Table, Issues::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("issue_genres_genre_idx")
                    .table(IssueGenres::Table)
                    .col(IssueGenres::Genre)
                    .to_owned(),
            )
            .await?;

        // ───── issue_tags ─────
        manager
            .create_table(
                Table::create()
                    .table(IssueTags::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(IssueTags::IssueId).text().not_null())
                    .col(ColumnDef::new(IssueTags::Tag).text().not_null())
                    .primary_key(Index::create().col(IssueTags::IssueId).col(IssueTags::Tag))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_issue_tags_issue")
                            .from(IssueTags::Table, IssueTags::IssueId)
                            .to(Issues::Table, Issues::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("issue_tags_tag_idx")
                    .table(IssueTags::Table)
                    .col(IssueTags::Tag)
                    .to_owned(),
            )
            .await?;

        // ───── issue_credits ─────
        manager
            .create_table(
                Table::create()
                    .table(IssueCredits::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(IssueCredits::IssueId).text().not_null())
                    .col(ColumnDef::new(IssueCredits::Role).text().not_null())
                    .col(ColumnDef::new(IssueCredits::Person).text().not_null())
                    .primary_key(
                        Index::create()
                            .col(IssueCredits::IssueId)
                            .col(IssueCredits::Role)
                            .col(IssueCredits::Person),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_issue_credits_issue")
                            .from(IssueCredits::Table, IssueCredits::IssueId)
                            .to(Issues::Table, Issues::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("issue_credits_role_person_idx")
                    .table(IssueCredits::Table)
                    .col(IssueCredits::Role)
                    .col(IssueCredits::Person)
                    .to_owned(),
            )
            .await?;

        // ───── drop legacy CSV-shaped override columns ─────
        manager
            .alter_table(
                Table::alter()
                    .table(Series::Table)
                    .drop_column(Series::Genre)
                    .drop_column(Series::Tags)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Restore the CSV columns first so the down sequence matches the up
        // sequence in reverse.
        manager
            .alter_table(
                Table::alter()
                    .table(Series::Table)
                    .add_column(ColumnDef::new(Series::Genre).text().null())
                    .add_column(ColumnDef::new(Series::Tags).text().null())
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(IssueCredits::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(IssueTags::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(IssueGenres::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(SeriesCredits::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(SeriesTags::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(SeriesGenres::Table).to_owned())
            .await?;

        Ok(())
    }
}
