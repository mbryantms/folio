//! Thumbnail pipeline (M1): per-issue generation state.
//!
//! Lets the post-scan worker pre-generate cover + per-page strip thumbs and
//! persist a "done" marker so the admin UI can show progress and trigger
//! catchup. The actual binary blobs live on disk under `/data/thumbs/`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Issues {
    Table,
    ThumbnailsGeneratedAt,
    ThumbnailVersion,
    ThumbnailsError,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Issues::Table)
                    .add_column(
                        ColumnDef::new(Issues::ThumbnailsGeneratedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(Issues::ThumbnailVersion)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .add_column(ColumnDef::new(Issues::ThumbnailsError).text().null())
                    .to_owned(),
            )
            .await?;

        // Partial index: the "what's still missing?" query for the admin UI
        // and the version-bump catchup sweep both filter on (library_id,
        // generated_at IS NULL). Keep it scoped to active issues so removed
        // rows don't bloat the index.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS issues_thumbs_pending_idx \
                 ON issues(library_id) \
                 WHERE thumbnails_generated_at IS NULL AND state = 'active'",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS issues_thumbs_pending_idx")
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Issues::Table)
                    .drop_column(Issues::ThumbnailsGeneratedAt)
                    .drop_column(Issues::ThumbnailVersion)
                    .drop_column(Issues::ThumbnailsError)
                    .to_owned(),
            )
            .await
    }
}
