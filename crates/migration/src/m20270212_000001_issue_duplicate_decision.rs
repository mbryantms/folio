//! `issue_duplicate_decision` — the admin's verdict on an issue surfaced
//! by the Duplicates page (roadmap WP-3.3, audit R15 / UX-9 / DI-21).
//!
//! One row per issue:
//!
//! - `keep`   — reviewed and intentionally kept. A duplicate group whose
//!   active members are all `keep` drops off the Duplicates page; a new
//!   copy arriving later (no decision yet) brings the group back.
//! - `remove` — soft-removed from the Duplicates page. The issue's
//!   `removed_at` is set at the same time; this row is what stops the
//!   scanner's reconcile pass from "restoring" it on the next scan
//!   (the file is still on disk, so the presence-driven restore rule
//!   would otherwise undo the removal). Restoring from the Removed tab
//!   or clearing the decision deletes the row.
//!
//! The row is deleted with its issue (`ON DELETE CASCADE`).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS issue_duplicate_decision (
                    issue_id    TEXT PRIMARY KEY REFERENCES issues(id) ON DELETE CASCADE,
                    library_id  UUID NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
                    decision    TEXT NOT NULL CHECK (decision IN ('keep', 'remove')),
                    decided_by  UUID NULL REFERENCES users(id) ON DELETE SET NULL,
                    decided_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
                );
                CREATE INDEX IF NOT EXISTS issue_duplicate_decision_library_idx
                    ON issue_duplicate_decision (library_id, decision);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS issue_duplicate_decision")
            .await?;
        Ok(())
    }
}
