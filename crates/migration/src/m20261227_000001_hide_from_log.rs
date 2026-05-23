//! Adds `hidden_from_log BOOLEAN NOT NULL DEFAULT false` to both
//! `markers` and `reading_sessions` so individual events can be hidden
//! from the reading-log feed without deleting the underlying row.
//!
//! `progress_records.is_backfill` (m20261226) already covers the
//! `issue_finished` event kind, so this migration is scoped to the two
//! tables that still need a hide flag. Same shape: nullable-NO
//! default-false BOOLEAN with a partial index on the rare `true` case
//! so cleanup queries ("show me everything I hid") stay cheap without
//! bloating the main feed scans.
//!
//! Read-path: every reading-log fetcher AND every stats query that
//! reads from `reading_sessions` (heatmap, daily pages, streak,
//! dow_hour, completion) filters `hidden_from_log = false` by
//! default. The user can opt in to seeing hidden rows via
//! `GET /me/reading-log?include_hidden=true` so they can audit /
//! unhide; that path does not affect stats surfaces (those always
//! exclude hidden rows — the user's intent in hiding is "this didn't
//! really happen as activity").
//!
//! `series_finished` is intentionally NOT covered. It's a derived
//! event (MAX(`finished_at`) per series); there's no single row to
//! flag. Users who want to remove a specific series-finish event
//! would need to mark contributing issues as backfill instead. A
//! future migration could add a `(user_id, series_id) hidden series_finish`
//! join table if demand surfaces.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for table in ["markers", "reading_sessions"] {
            db.execute_unprepared(&format!(
                "ALTER TABLE {table} \
                 ADD COLUMN IF NOT EXISTS hidden_from_log BOOLEAN \
                 NOT NULL DEFAULT FALSE"
            ))
            .await?;
            db.execute_unprepared(&format!(
                "CREATE INDEX IF NOT EXISTS {table}_hidden_idx \
                 ON {table} (user_id) \
                 WHERE hidden_from_log = TRUE"
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for table in ["markers", "reading_sessions"] {
            db.execute_unprepared(&format!(
                "DROP INDEX IF EXISTS {table}_hidden_idx"
            ))
            .await?;
            db.execute_unprepared(&format!(
                "ALTER TABLE {table} DROP COLUMN IF EXISTS hidden_from_log"
            ))
            .await?;
        }
        Ok(())
    }
}
