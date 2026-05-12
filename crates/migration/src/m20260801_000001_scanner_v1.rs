//! Library Scanner v1 — schema foundation.
//!
//! Adds the columns and tables the spec needs but the Phase 1a stub did not:
//! - `series.folder_path`, `last_scanned_at`, `match_key`, soft-delete columns
//! - `issues.removed_at`, `removal_confirmed_at`, `superseded_by`,
//!   `special_type`, `hash_algorithm`
//! - `libraries.ignore_globs`, `report_missing_comicinfo`, `file_watch_enabled`,
//!   `soft_delete_days`
//! - new `library_health_issues` table (spec §10.2)
//!
//! References: library-scanner-spec.md §4.7 (soft-delete), §6.2 (superseded),
//! §6.5 (special_type), §7 (folder_path, match_key), §10 (health), §11 (config),
//! §14.2 (hash_algorithm forward-compat).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Series {
    Table,
    LibraryId,
    FolderPath,
    LastScannedAt,
    MatchKey,
    RemovedAt,
    RemovalConfirmedAt,
}

#[derive(Iden)]
enum Issues {
    Table,
    RemovedAt,
    RemovalConfirmedAt,
    SupersededBy,
    SpecialType,
    HashAlgorithm,
}

#[derive(Iden)]
enum Libraries {
    Table,
    Id,
    IgnoreGlobs,
    ReportMissingComicinfo,
    FileWatchEnabled,
    SoftDeleteDays,
}

#[derive(Iden)]
enum LibraryHealthIssues {
    Table,
    Id,
    LibraryId,
    ScanId,
    Kind,
    Payload,
    Severity,
    Fingerprint,
    FirstSeenAt,
    LastSeenAt,
    ResolvedAt,
    DismissedAt,
}

#[derive(Iden)]
enum ScanRuns {
    Table,
    Id,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ───── series ─────
        manager
            .alter_table(
                Table::alter()
                    .table(Series::Table)
                    .add_column(ColumnDef::new(Series::FolderPath).text().null())
                    .add_column(
                        ColumnDef::new(Series::LastScannedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .add_column(ColumnDef::new(Series::MatchKey).text().null())
                    .add_column(
                        ColumnDef::new(Series::RemovedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(Series::RemovalConfirmedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("series_folder_path_idx")
                    .table(Series::Table)
                    .col(Series::LibraryId)
                    .col(Series::FolderPath)
                    .to_owned(),
            )
            .await?;

        // Partial index — only rows where removed_at is set are tracked.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS series_removed_at_idx \
                 ON series(library_id, removed_at) \
                 WHERE removed_at IS NOT NULL",
            )
            .await?;

        // ───── issues ─────
        manager
            .alter_table(
                Table::alter()
                    .table(Issues::Table)
                    .add_column(
                        ColumnDef::new(Issues::RemovedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(Issues::RemovalConfirmedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .add_column(ColumnDef::new(Issues::SupersededBy).text().null())
                    .add_column(ColumnDef::new(Issues::SpecialType).text().null())
                    .add_column(
                        ColumnDef::new(Issues::HashAlgorithm)
                            .small_integer()
                            .not_null()
                            .default(1),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS issues_removed_at_idx \
                 ON issues(library_id, removed_at) \
                 WHERE removed_at IS NOT NULL",
            )
            .await?;

        // ───── libraries ─────
        manager
            .alter_table(
                Table::alter()
                    .table(Libraries::Table)
                    .add_column(
                        ColumnDef::new(Libraries::IgnoreGlobs)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'[]'::jsonb")),
                    )
                    .add_column(
                        ColumnDef::new(Libraries::ReportMissingComicinfo)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .add_column(
                        ColumnDef::new(Libraries::FileWatchEnabled)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .add_column(
                        ColumnDef::new(Libraries::SoftDeleteDays)
                            .integer()
                            .not_null()
                            .default(30),
                    )
                    .to_owned(),
            )
            .await?;

        // ───── library_health_issues ─────
        manager
            .create_table(
                Table::create()
                    .table(LibraryHealthIssues::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(LibraryHealthIssues::Id)
                            .uuid()
                            .not_null()
                            .primary_key()
                            .default(Expr::cust("gen_random_uuid()")),
                    )
                    .col(
                        ColumnDef::new(LibraryHealthIssues::LibraryId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(LibraryHealthIssues::ScanId).uuid().null())
                    .col(ColumnDef::new(LibraryHealthIssues::Kind).text().not_null())
                    .col(
                        ColumnDef::new(LibraryHealthIssues::Payload)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'{}'::jsonb")),
                    )
                    .col(
                        ColumnDef::new(LibraryHealthIssues::Severity)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(LibraryHealthIssues::Fingerprint)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(LibraryHealthIssues::FirstSeenAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(LibraryHealthIssues::LastSeenAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(LibraryHealthIssues::ResolvedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(LibraryHealthIssues::DismissedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(LibraryHealthIssues::Table, LibraryHealthIssues::LibraryId)
                            .to(Libraries::Table, Libraries::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(LibraryHealthIssues::Table, LibraryHealthIssues::ScanId)
                            .to(ScanRuns::Table, ScanRuns::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .index(
                        Index::create()
                            .name("library_health_issues_fingerprint_uniq")
                            .unique()
                            .col(LibraryHealthIssues::LibraryId)
                            .col(LibraryHealthIssues::Fingerprint),
                    )
                    .to_owned(),
            )
            .await?;

        // Partial index over open issues for fast admin-UI queries.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS library_health_issues_open_idx \
                 ON library_health_issues(library_id, severity) \
                 WHERE resolved_at IS NULL AND dismissed_at IS NULL",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(LibraryHealthIssues::Table).to_owned())
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Libraries::Table)
                    .drop_column(Libraries::SoftDeleteDays)
                    .drop_column(Libraries::FileWatchEnabled)
                    .drop_column(Libraries::ReportMissingComicinfo)
                    .drop_column(Libraries::IgnoreGlobs)
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS issues_removed_at_idx")
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Issues::Table)
                    .drop_column(Issues::HashAlgorithm)
                    .drop_column(Issues::SpecialType)
                    .drop_column(Issues::SupersededBy)
                    .drop_column(Issues::RemovalConfirmedAt)
                    .drop_column(Issues::RemovedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS series_removed_at_idx")
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("series_folder_path_idx")
                    .table(Series::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Series::Table)
                    .drop_column(Series::RemovalConfirmedAt)
                    .drop_column(Series::RemovedAt)
                    .drop_column(Series::MatchKey)
                    .drop_column(Series::LastScannedAt)
                    .drop_column(Series::FolderPath)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
