//! Annuals and specials toward series completion, per user with a
//! per-series override (remediation B9, owner decision D-7).
//!
//! Collection completeness compares the owned **main run**
//! (`issues.special_type IS NULL`) with `series.total_issues`. Some
//! collectors want their annuals / one-shots / specials counted too, so:
//!
//! - `users.count_specials_in_completeness` — the account setting,
//!   `NOT NULL DEFAULT false` (false = the behaviour before this
//!   migration, so existing accounts see no change).
//! - `user_series_completion_prefs` — one row per `(user, series)` the
//!   user has overridden: `count_specials = true` (include) or `false`
//!   (exclude). No row = inherit the account setting. The PK leads with
//!   `user_id`, so the completeness queries' lookup of the caller's rows
//!   for a page of series is an index range scan.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(DeriveIden)]
enum Users {
    Table,
    CountSpecialsInCompleteness,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::CountSpecialsInCompleteness)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        let db = manager.get_connection();
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS user_series_completion_prefs (\
                user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE, \
                series_id UUID NOT NULL REFERENCES series(id) ON DELETE CASCADE, \
                count_specials BOOLEAN NOT NULL, \
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
                PRIMARY KEY (user_id, series_id)\
            )",
        )
        .await?;
        // Series deletes cascade through this FK; index the referencing
        // column so a series purge doesn't seq-scan the table.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS user_series_completion_prefs_series_idx \
             ON user_series_completion_prefs (series_id)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS user_series_completion_prefs")
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::CountSpecialsInCompleteness)
                    .to_owned(),
            )
            .await
    }
}
