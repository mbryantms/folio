//! Index `issues.content_hash` so the scanner's dedupe-by-content lookup
//! stays fast once `id` and `content_hash` are allowed to diverge.
//!
//! Historically the scanner conflated `id` (PK) with `content_hash` —
//! both were set to the file's BLAKE3 hash and an update path mutated
//! `id` whenever bytes changed. That mutation failed silently when the
//! UPDATE's WHERE clause used the new hash (RecordNotUpdated), so
//! retagged files never picked up their ComicInfo metadata. The fix
//! keeps `id` stable as a per-row identifier and stores the live
//! fingerprint on `content_hash`. The dedupe-by-content lookup then
//! filters by `content_hash` instead of `find_by_id`, which needs an
//! index to stay sub-millisecond at production scale.
//!
//! Non-unique on purpose: application-level DuplicateContent emits a
//! soft health-issue when two files share bytes. A UNIQUE constraint
//! would turn that into a transaction abort and regress current
//! behavior.
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_issues_content_hash \
             ON issues (content_hash)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS idx_issues_content_hash")
            .await?;
        Ok(())
    }
}
