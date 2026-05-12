//! Scanner performance state.
//!
//! Adds durable tables for two scanner follow-ups:
//! - `scanner_folder_state` records per-folder dirty/watcher state.
//! - `issue_paths` records one issue across one or more on-disk paths so
//!   move/alias semantics can replace duplicate-content skips.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE TABLE IF NOT EXISTS scanner_folder_state (
                    library_id uuid NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
                    folder_path text NOT NULL,
                    dirty boolean NOT NULL DEFAULT false,
                    watcher_status text NOT NULL DEFAULT 'unknown',
                    sidecar_hash text NULL,
                    last_seen_at timestamptz NULL,
                    last_changed_at timestamptz NULL,
                    PRIMARY KEY (library_id, folder_path)
                );

                CREATE INDEX IF NOT EXISTS scanner_folder_state_dirty_idx
                    ON scanner_folder_state(library_id, dirty)
                    WHERE dirty = true;

                CREATE TABLE IF NOT EXISTS issue_paths (
                    issue_id text NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                    file_path text NOT NULL PRIMARY KEY,
                    is_primary boolean NOT NULL DEFAULT false,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    missing_at timestamptz NULL
                );

                CREATE INDEX IF NOT EXISTS issue_paths_issue_idx
                    ON issue_paths(issue_id);

                CREATE UNIQUE INDEX IF NOT EXISTS issue_paths_one_primary_idx
                    ON issue_paths(issue_id)
                    WHERE is_primary = true;

                INSERT INTO issue_paths (issue_id, file_path, is_primary, created_at)
                SELECT id, file_path, true, created_at
                FROM issues
                ON CONFLICT (file_path) DO NOTHING;
                "#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                DROP TABLE IF EXISTS issue_paths;
                DROP TABLE IF EXISTS scanner_folder_state;
                "#,
            )
            .await?;
        Ok(())
    }
}
