//! Series and Issue tables (Phase 1a).
//!
//! Issue stable id = BLAKE3 hex of either path or content (§5.1.2). We store the
//! ID itself as the BLAKE3 hex string (text), not a UUID, so it's deterministic
//! across rescans.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

// `SeriesGroup` keeps its prefix because sea-orm's `Iden` derive maps
// variant casing to column identifiers; renaming would change the column name.
#[allow(clippy::enum_variant_names)]
#[derive(Iden)]
enum Series {
    Table,
    Id,
    LibraryId,
    Name,
    NormalizedName,
    Year,
    Volume,
    Publisher,
    Imprint,
    Status, // continuing | ended | cancelled | hiatus
    TotalIssues,
    AgeRating,
    Summary,
    LanguageCode,
    ComicvineId,
    MetronId,
    Gtin,
    SeriesGroup,
    AlternateNames, // jsonb array of strings
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum Issues {
    Table,
    Id, // BLAKE3 hex (64 chars)
    LibraryId,
    SeriesId,
    FilePath,
    FileSize,
    FileMtime,
    State,       // active | encrypted | malformed | tombstoned
    ContentHash, // same as id when dedupe_by_content; differs only if collision tracking later
    Title,
    SortNumber, // numeric for ordering: parsed from `Number`
    NumberRaw,
    Volume,
    Year,
    Month,
    Day,
    Summary,
    Notes,
    LanguageCode,
    Format,
    BlackAndWhite,
    Manga,
    AgeRating,
    PageCount,
    Pages,        // jsonb array of per-page metadata (§5.1: no separate table)
    ComicInfoRaw, // jsonb full blob for forward-compat
    AlternateSeries,
    StoryArc,
    StoryArcNumber,
    Characters,
    Teams,
    Locations,
    Tags,
    Genre,
    Writer,
    Penciller,
    Inker,
    Colorist,
    Letterer,
    CoverArtist,
    Editor,
    Translator,
    Publisher,
    Imprint,
    ScanInformation,
    CommunityRating,
    Review,
    WebUrl,
    ComicvineId,
    MetronId,
    Gtin,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum Libraries {
    Table,
    Id,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ───── series ─────
        manager
            .create_table(
                Table::create()
                    .table(Series::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Series::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Series::LibraryId).uuid().not_null())
                    .col(ColumnDef::new(Series::Name).text().not_null())
                    .col(ColumnDef::new(Series::NormalizedName).text().not_null())
                    .col(ColumnDef::new(Series::Year).integer().null())
                    .col(ColumnDef::new(Series::Volume).integer().null())
                    .col(ColumnDef::new(Series::Publisher).text().null())
                    .col(ColumnDef::new(Series::Imprint).text().null())
                    .col(
                        ColumnDef::new(Series::Status)
                            .text()
                            .not_null()
                            .default("continuing"),
                    )
                    .col(ColumnDef::new(Series::TotalIssues).integer().null())
                    .col(ColumnDef::new(Series::AgeRating).text().null())
                    .col(ColumnDef::new(Series::Summary).text().null())
                    .col(
                        ColumnDef::new(Series::LanguageCode)
                            .text()
                            .not_null()
                            .default("eng"),
                    )
                    .col(ColumnDef::new(Series::ComicvineId).big_integer().null())
                    .col(ColumnDef::new(Series::MetronId).big_integer().null())
                    .col(ColumnDef::new(Series::Gtin).text().null())
                    .col(ColumnDef::new(Series::SeriesGroup).text().null())
                    .col(
                        ColumnDef::new(Series::AlternateNames)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'[]'::jsonb")),
                    )
                    .col(
                        ColumnDef::new(Series::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Series::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Series::Table, Series::LibraryId)
                            .to(Libraries::Table, Libraries::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("series_library_normalized_uniq")
                    .unique()
                    .table(Series::Table)
                    .col(Series::LibraryId)
                    .col(Series::NormalizedName)
                    .col(Series::Year)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("series_normalized_name_idx")
                    .table(Series::Table)
                    .col(Series::NormalizedName)
                    .to_owned(),
            )
            .await?;

        // ───── issues ─────
        manager
            .create_table(
                Table::create()
                    .table(Issues::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Issues::Id).text().not_null().primary_key())
                    .col(ColumnDef::new(Issues::LibraryId).uuid().not_null())
                    .col(ColumnDef::new(Issues::SeriesId).uuid().not_null())
                    .col(ColumnDef::new(Issues::FilePath).text().not_null())
                    .col(ColumnDef::new(Issues::FileSize).big_integer().not_null())
                    .col(
                        ColumnDef::new(Issues::FileMtime)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Issues::State)
                            .text()
                            .not_null()
                            .default("active"),
                    )
                    .col(ColumnDef::new(Issues::ContentHash).text().not_null())
                    .col(ColumnDef::new(Issues::Title).text().null())
                    .col(ColumnDef::new(Issues::SortNumber).double().null())
                    .col(ColumnDef::new(Issues::NumberRaw).text().null())
                    .col(ColumnDef::new(Issues::Volume).integer().null())
                    .col(ColumnDef::new(Issues::Year).integer().null())
                    .col(ColumnDef::new(Issues::Month).integer().null())
                    .col(ColumnDef::new(Issues::Day).integer().null())
                    .col(ColumnDef::new(Issues::Summary).text().null())
                    .col(ColumnDef::new(Issues::Notes).text().null())
                    .col(ColumnDef::new(Issues::LanguageCode).text().null())
                    .col(ColumnDef::new(Issues::Format).text().null())
                    .col(ColumnDef::new(Issues::BlackAndWhite).boolean().null())
                    .col(ColumnDef::new(Issues::Manga).text().null())
                    .col(ColumnDef::new(Issues::AgeRating).text().null())
                    .col(ColumnDef::new(Issues::PageCount).integer().null())
                    .col(
                        ColumnDef::new(Issues::Pages)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'[]'::jsonb")),
                    )
                    .col(
                        ColumnDef::new(Issues::ComicInfoRaw)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'{}'::jsonb")),
                    )
                    .col(ColumnDef::new(Issues::AlternateSeries).text().null())
                    .col(ColumnDef::new(Issues::StoryArc).text().null())
                    .col(ColumnDef::new(Issues::StoryArcNumber).text().null())
                    .col(ColumnDef::new(Issues::Characters).text().null())
                    .col(ColumnDef::new(Issues::Teams).text().null())
                    .col(ColumnDef::new(Issues::Locations).text().null())
                    .col(ColumnDef::new(Issues::Tags).text().null())
                    .col(ColumnDef::new(Issues::Genre).text().null())
                    .col(ColumnDef::new(Issues::Writer).text().null())
                    .col(ColumnDef::new(Issues::Penciller).text().null())
                    .col(ColumnDef::new(Issues::Inker).text().null())
                    .col(ColumnDef::new(Issues::Colorist).text().null())
                    .col(ColumnDef::new(Issues::Letterer).text().null())
                    .col(ColumnDef::new(Issues::CoverArtist).text().null())
                    .col(ColumnDef::new(Issues::Editor).text().null())
                    .col(ColumnDef::new(Issues::Translator).text().null())
                    .col(ColumnDef::new(Issues::Publisher).text().null())
                    .col(ColumnDef::new(Issues::Imprint).text().null())
                    .col(ColumnDef::new(Issues::ScanInformation).text().null())
                    .col(ColumnDef::new(Issues::CommunityRating).double().null())
                    .col(ColumnDef::new(Issues::Review).text().null())
                    .col(ColumnDef::new(Issues::WebUrl).text().null())
                    .col(ColumnDef::new(Issues::ComicvineId).big_integer().null())
                    .col(ColumnDef::new(Issues::MetronId).big_integer().null())
                    .col(ColumnDef::new(Issues::Gtin).text().null())
                    .col(
                        ColumnDef::new(Issues::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Issues::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Issues::Table, Issues::LibraryId)
                            .to(Libraries::Table, Libraries::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Issues::Table, Issues::SeriesId)
                            .to(Series::Table, Series::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("issues_series_sortnum_idx")
                    .table(Issues::Table)
                    .col(Issues::SeriesId)
                    .col(Issues::SortNumber)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("issues_library_idx")
                    .table(Issues::Table)
                    .col(Issues::LibraryId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("issues_file_path_uniq")
                    .unique()
                    .table(Issues::Table)
                    .col(Issues::FilePath)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Issues::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Series::Table).to_owned())
            .await
    }
}
