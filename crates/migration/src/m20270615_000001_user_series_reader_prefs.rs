//! `user_series_reader_prefs` — per-user, per-series reader overrides that
//! follow the account (post-audit T4.4, decision D9a).
//!
//! The reader's per-series choices (reading direction, fit mode, view
//! mode, cover solo) lived only in the browser's localStorage, so a
//! series read right-to-left on the desktop opened left-to-right on the
//! phone. One row per `(user, series)` with a nullable column per slice:
//! NULL means "no override, fall through to the user default". A row with
//! every slice NULL is deleted rather than kept.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS user_series_reader_prefs (\
                    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE, \
                    series_id UUID NOT NULL REFERENCES series(id) ON DELETE CASCADE, \
                    reading_direction TEXT NULL, \
                    fit_mode TEXT NULL, \
                    view_mode TEXT NULL, \
                    cover_solo BOOLEAN NULL, \
                    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
                    PRIMARY KEY (user_id, series_id)\
                )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS user_series_reader_prefs")
            .await?;
        Ok(())
    }
}
