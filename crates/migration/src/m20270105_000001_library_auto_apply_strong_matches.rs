//! Matching-accuracy-1.0 M12 — opt-in auto-apply on `SingleGoodMatch`.
//!
//! Adds `libraries.metadata_auto_apply_strong_matches BOOLEAN NOT
//! NULL DEFAULT FALSE`. When this is on AND a non-manual search
//! (weekly cron / bulk-fetch toolbar) ends with the strict
//! `MatchOutcomeKind::SingleGood` outcome, the orchestrator
//! auto-enqueues an apply job for the top candidate.
//!
//! Three layers of safety prevent surprise writes:
//!
//! 1. Per-library opt-in — default OFF, so the operator-explicit-
//!    consent baseline is preserved.
//! 2. `override_user_edits=false` on the auto-apply path — the
//!    user-edit precedence rule still fires, so pinned fields stay
//!    sacred even on auto-apply.
//! 3. Distinct audit action `admin.{series,issue}.metadata_auto_apply`
//!    (vs the manual `metadata_apply`) so operators can grep + flip
//!    the toggle back off if anything goes wrong.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE libraries \
             ADD COLUMN IF NOT EXISTS metadata_auto_apply_strong_matches BOOLEAN \
             NOT NULL DEFAULT FALSE",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE libraries DROP COLUMN IF EXISTS metadata_auto_apply_strong_matches",
        )
        .await?;
        Ok(())
    }
}
