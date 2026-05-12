//! Saved smart views — M4: CBL reading-list backend.
//!
//! Four new tables:
//!
//!   - `catalog_sources` — admin-managed GitHub repos that ship `.cbl`
//!     files (e.g., `DieselTech/CBL-ReadingLists`). The fetcher caches the
//!     repo's tree in `index_json` keyed by `index_etag` so browsing the
//!     catalog is one HTTP round-trip per stale-cache window.
//!   - `cbl_lists` — one row per imported list. `source_kind` discriminates
//!     between user upload, direct URL, and catalog-sourced; the latter
//!     also carries `(catalog_source_id, catalog_path, github_blob_sha)`
//!     for change detection.
//!   - `cbl_entries` — one row per `<Book>` in the file. `match_status`
//!     classifies the resolution outcome (`matched | ambiguous | missing
//!     | manual`). Ambiguity is preserved verbatim in
//!     `ambiguous_candidates` so the Resolution UI can replay the
//!     matcher's top picks without re-running it.
//!   - `cbl_refresh_log` — append-only history of refresh runs with the
//!     structural diff against the previous import. Drives the History
//!     tab and post-refresh toasts.
//!
//! Also: the FK from `saved_views.cbl_list_id` (added by M3 as a bare
//! UUID column) is enforced here, now that `cbl_lists` exists. Seeds the
//! DieselTech catalog source as the bundled default.

use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum CatalogSources {
    Table,
    Id,
    DisplayName,
    GithubOwner,
    GithubRepo,
    GithubBranch,
    Enabled,
    LastIndexedAt,
    IndexEtag,
    IndexJson,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum CblLists {
    Table,
    Id,
    OwnerUserId,
    SourceKind,
    SourceUrl,
    CatalogSourceId,
    CatalogPath,
    GithubBlobSha,
    SourceEtag,
    SourceLastModified,
    RawSha256,
    RawXml,
    ParsedName,
    ParsedMatchersPresent,
    NumIssuesDeclared,
    Description,
    ImportedAt,
    LastRefreshedAt,
    LastMatchRunAt,
    RefreshSchedule,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum CblEntries {
    Table,
    Id,
    CblListId,
    Position,
    SeriesName,
    IssueNumber,
    Volume,
    Year,
    CvSeriesId,
    CvIssueId,
    MetronSeriesId,
    MetronIssueId,
    MatchedIssueId,
    MatchStatus,
    MatchMethod,
    MatchConfidence,
    AmbiguousCandidates,
    UserResolvedAt,
    MatchedAt,
}

#[derive(Iden)]
enum CblRefreshLog {
    Table,
    Id,
    CblListId,
    RanAt,
    Trigger,
    UpstreamChanged,
    PrevBlobSha,
    NewBlobSha,
    AddedCount,
    RemovedCount,
    ReorderedCount,
    RematchedCount,
    DiffSummary,
}

#[derive(Iden)]
enum SavedViews {
    Table,
    CblListId,
}

#[derive(Iden)]
enum Issues {
    Table,
    Id,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ───── catalog_sources ─────
        manager
            .create_table(
                Table::create()
                    .table(CatalogSources::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(CatalogSources::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(CatalogSources::DisplayName)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(CatalogSources::GithubOwner)
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(CatalogSources::GithubRepo).text().not_null())
                    .col(
                        ColumnDef::new(CatalogSources::GithubBranch)
                            .text()
                            .not_null()
                            .default("main"),
                    )
                    .col(
                        ColumnDef::new(CatalogSources::Enabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(CatalogSources::LastIndexedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(ColumnDef::new(CatalogSources::IndexEtag).text().null())
                    .col(
                        ColumnDef::new(CatalogSources::IndexJson)
                            .json_binary()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(CatalogSources::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(CatalogSources::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("catalog_sources_owner_repo_uniq")
                    .unique()
                    .table(CatalogSources::Table)
                    .col(CatalogSources::GithubOwner)
                    .col(CatalogSources::GithubRepo)
                    .col(CatalogSources::GithubBranch)
                    .to_owned(),
            )
            .await?;

        // ───── cbl_lists ─────
        manager
            .create_table(
                Table::create()
                    .table(CblLists::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(CblLists::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(CblLists::OwnerUserId).uuid().null())
                    .col(ColumnDef::new(CblLists::SourceKind).text().not_null())
                    .col(ColumnDef::new(CblLists::SourceUrl).text().null())
                    .col(ColumnDef::new(CblLists::CatalogSourceId).uuid().null())
                    .col(ColumnDef::new(CblLists::CatalogPath).text().null())
                    .col(ColumnDef::new(CblLists::GithubBlobSha).text().null())
                    .col(ColumnDef::new(CblLists::SourceEtag).text().null())
                    .col(ColumnDef::new(CblLists::SourceLastModified).text().null())
                    .col(ColumnDef::new(CblLists::RawSha256).binary().not_null())
                    .col(ColumnDef::new(CblLists::RawXml).text().not_null())
                    .col(ColumnDef::new(CblLists::ParsedName).text().not_null())
                    .col(
                        ColumnDef::new(CblLists::ParsedMatchersPresent)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(CblLists::NumIssuesDeclared).integer().null())
                    .col(ColumnDef::new(CblLists::Description).text().null())
                    .col(
                        ColumnDef::new(CblLists::ImportedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(CblLists::LastRefreshedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(CblLists::LastMatchRunAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(ColumnDef::new(CblLists::RefreshSchedule).text().null())
                    .col(
                        ColumnDef::new(CblLists::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(CblLists::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_cbl_lists_user")
                            .from(CblLists::Table, CblLists::OwnerUserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_cbl_lists_catalog_source")
                            .from(CblLists::Table, CblLists::CatalogSourceId)
                            .to(CatalogSources::Table, CatalogSources::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("cbl_lists_owner_idx")
                    .table(CblLists::Table)
                    .col(CblLists::OwnerUserId)
                    .to_owned(),
            )
            .await?;
        // CHECK: source_kind is one of the three allowed values; catalog
        // kind requires its FK fields, URL/upload don't.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE cbl_lists ADD CONSTRAINT cbl_lists_source_chk CHECK (\
                    source_kind IN ('upload', 'url', 'catalog') AND (\
                        (source_kind = 'catalog' AND catalog_source_id IS NOT NULL AND catalog_path IS NOT NULL) \
                     OR (source_kind = 'url' AND source_url IS NOT NULL) \
                     OR (source_kind = 'upload')\
                    )\
                 )",
            )
            .await?;

        // ───── cbl_entries ─────
        manager
            .create_table(
                Table::create()
                    .table(CblEntries::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(CblEntries::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(CblEntries::CblListId).uuid().not_null())
                    .col(ColumnDef::new(CblEntries::Position).integer().not_null())
                    .col(ColumnDef::new(CblEntries::SeriesName).text().not_null())
                    .col(ColumnDef::new(CblEntries::IssueNumber).text().not_null())
                    .col(ColumnDef::new(CblEntries::Volume).text().null())
                    .col(ColumnDef::new(CblEntries::Year).text().null())
                    .col(ColumnDef::new(CblEntries::CvSeriesId).integer().null())
                    .col(ColumnDef::new(CblEntries::CvIssueId).integer().null())
                    .col(ColumnDef::new(CblEntries::MetronSeriesId).integer().null())
                    .col(ColumnDef::new(CblEntries::MetronIssueId).integer().null())
                    .col(ColumnDef::new(CblEntries::MatchedIssueId).text().null())
                    .col(
                        ColumnDef::new(CblEntries::MatchStatus)
                            .text()
                            .not_null()
                            .default("missing"),
                    )
                    .col(ColumnDef::new(CblEntries::MatchMethod).text().null())
                    .col(ColumnDef::new(CblEntries::MatchConfidence).float().null())
                    .col(
                        ColumnDef::new(CblEntries::AmbiguousCandidates)
                            .json_binary()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(CblEntries::UserResolvedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(CblEntries::MatchedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_cbl_entries_list")
                            .from(CblEntries::Table, CblEntries::CblListId)
                            .to(CblLists::Table, CblLists::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_cbl_entries_issue")
                            .from(CblEntries::Table, CblEntries::MatchedIssueId)
                            .to(Issues::Table, Issues::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("cbl_entries_list_position_uniq")
                    .unique()
                    .table(CblEntries::Table)
                    .col(CblEntries::CblListId)
                    .col(CblEntries::Position)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("cbl_entries_status_idx")
                    .table(CblEntries::Table)
                    .col(CblEntries::CblListId)
                    .col(CblEntries::MatchStatus)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("cbl_entries_cv_issue_idx")
                    .table(CblEntries::Table)
                    .col(CblEntries::CvIssueId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("cbl_entries_matched_issue_idx")
                    .table(CblEntries::Table)
                    .col(CblEntries::MatchedIssueId)
                    .to_owned(),
            )
            .await?;

        // ───── cbl_refresh_log ─────
        manager
            .create_table(
                Table::create()
                    .table(CblRefreshLog::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(CblRefreshLog::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(CblRefreshLog::CblListId).uuid().not_null())
                    .col(
                        ColumnDef::new(CblRefreshLog::RanAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(CblRefreshLog::Trigger).text().not_null())
                    .col(
                        ColumnDef::new(CblRefreshLog::UpstreamChanged)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(CblRefreshLog::PrevBlobSha).text().null())
                    .col(ColumnDef::new(CblRefreshLog::NewBlobSha).text().null())
                    .col(
                        ColumnDef::new(CblRefreshLog::AddedCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(CblRefreshLog::RemovedCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(CblRefreshLog::ReorderedCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(CblRefreshLog::RematchedCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(CblRefreshLog::DiffSummary)
                            .json_binary()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_cbl_refresh_log_list")
                            .from(CblRefreshLog::Table, CblRefreshLog::CblListId)
                            .to(CblLists::Table, CblLists::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("cbl_refresh_log_list_ran_idx")
                    .table(CblRefreshLog::Table)
                    .col(CblRefreshLog::CblListId)
                    .col(CblRefreshLog::RanAt)
                    .to_owned(),
            )
            .await?;

        // ───── enforce saved_views.cbl_list_id FK now that cbl_lists exists ─────
        manager
            .alter_table(
                Table::alter()
                    .table(SavedViews::Table)
                    .add_foreign_key(
                        TableForeignKey::new()
                            .name("fk_saved_views_cbl_list")
                            .from_tbl(SavedViews::Table)
                            .from_col(SavedViews::CblListId)
                            .to_tbl(CblLists::Table)
                            .to_col(CblLists::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        // ───── seed DieselTech catalog source ─────
        let backend = manager.get_database_backend();
        manager
            .get_connection()
            .execute(Statement::from_sql_and_values(
                backend,
                r"INSERT INTO catalog_sources
                    (id, display_name, github_owner, github_repo, github_branch, enabled)
                  VALUES
                    ($1::uuid, $2, $3, $4, 'main', true)
                  ON CONFLICT (id) DO NOTHING",
                [
                    "00000000-0000-0000-0000-000000001001".into(),
                    "DieselTech CBL Reading Lists".into(),
                    "DieselTech".into(),
                    "CBL-ReadingLists".into(),
                ],
            ))
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SavedViews::Table)
                    .drop_foreign_key(Alias::new("fk_saved_views_cbl_list"))
                    .to_owned(),
            )
            .await
            .ok();
        manager
            .drop_table(Table::drop().table(CblRefreshLog::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(CblEntries::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(CblLists::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(CatalogSources::Table).to_owned())
            .await?;
        Ok(())
    }
}
