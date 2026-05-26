//! Metadata Providers 1.0 — M3: ranked-candidate persistence.
//!
//! Adds the storage that bridges *Search* runs (M3) to *Apply* jobs
//! (M4):
//!
//! - `metadata_run.query` (JSONB) — the search inputs (series name,
//!   year, publisher hint, etc.) so the polling endpoint can render
//!   "Searching ‹Saga (2012)› across ComicVine + Metron…" without
//!   re-deriving from `scope_entity_id`.
//! - `metadata_run_candidate` — one row per ranked result. Hangs off
//!   `metadata_run` and carries the score breakdown + the serialized
//!   `SeriesCandidate` / `IssueCandidate`. M4 Apply jobs flip
//!   `applied_at` when the user picks one.
//!
//! Ranked candidates intentionally live in their own table rather than
//! on a JSONB column of `metadata_run`: it keeps the per-row size on
//! the runs feed small (the Runs tab in M6 needs to scan many rows
//! efficiently), supports per-candidate state (`applied_at`,
//! `dismissed_at` in M4+), and lets the review queue paginate the
//! medium/low buckets cheaply.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            "ALTER TABLE metadata_run \
             ADD COLUMN IF NOT EXISTS query JSONB",
        )
        .await?;

        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS metadata_run_candidate (
                run_id          UUID NOT NULL REFERENCES metadata_run(id) ON DELETE CASCADE,
                ordinal         INT NOT NULL,
                source          TEXT NOT NULL,
                external_id     TEXT NOT NULL,
                bucket          TEXT NOT NULL,
                score           REAL NOT NULL,
                score_breakdown JSONB NOT NULL DEFAULT '{}'::jsonb,
                candidate       JSONB NOT NULL,
                applied_at      TIMESTAMPTZ,
                dismissed_at    TIMESTAMPTZ,
                PRIMARY KEY (run_id, ordinal)
            )"#,
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS metadata_run_candidate_review \
             ON metadata_run_candidate (bucket) WHERE applied_at IS NULL AND dismissed_at IS NULL",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS metadata_run_candidate")
            .await?;
        db.execute_unprepared("ALTER TABLE metadata_run DROP COLUMN IF EXISTS query")
            .await?;
        Ok(())
    }
}
