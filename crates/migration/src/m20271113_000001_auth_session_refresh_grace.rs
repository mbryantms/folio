//! Refresh-token grace window (SESS-GRACE).
//!
//! A refresh rotates the session's token before its response leaves the
//! server, so a response that never arrives leaves the browser holding the
//! previous, now refused, token. The session row now remembers that
//! previous token for a short while after each rotation:
//!
//! - `auth_sessions.previous_refresh_token_hash TEXT NULL` — SHA-256 of
//!   the token the last normal rotation replaced. Cleared when the grace
//!   path consumes it, so it is accepted at most once per rotation.
//! - `auth_sessions.refresh_rotated_at TIMESTAMPTZ NULL` — when the last
//!   rotation happened; the grace window is measured from it. NULL on rows
//!   that have not been refreshed since this migration.
//! - `auth_sessions_previous_refresh_hash_idx` — partial index for the
//!   grace arm of the refresh `UPDATE` (`WHERE … OR previous_… = $hash`).
//! - CHECK: a remembered previous hash always has a rotation time.
//!
//! See `docs/architecture/auth-model.md` § Refresh.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE auth_sessions \
             ADD COLUMN IF NOT EXISTS previous_refresh_token_hash TEXT NULL",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE auth_sessions \
             ADD COLUMN IF NOT EXISTS refresh_rotated_at TIMESTAMPTZ NULL",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE auth_sessions \
             DROP CONSTRAINT IF EXISTS auth_sessions_previous_refresh_rotated_check",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE auth_sessions \
             ADD CONSTRAINT auth_sessions_previous_refresh_rotated_check \
             CHECK (previous_refresh_token_hash IS NULL OR refresh_rotated_at IS NOT NULL)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS auth_sessions_previous_refresh_hash_idx \
             ON auth_sessions (previous_refresh_token_hash) \
             WHERE previous_refresh_token_hash IS NOT NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS auth_sessions_previous_refresh_hash_idx")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE auth_sessions \
             DROP CONSTRAINT IF EXISTS auth_sessions_previous_refresh_rotated_check",
        )
        .await?;
        db.execute_unprepared("ALTER TABLE auth_sessions DROP COLUMN IF EXISTS refresh_rotated_at")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE auth_sessions DROP COLUMN IF EXISTS previous_refresh_token_hash",
        )
        .await?;
        Ok(())
    }
}
