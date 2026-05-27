//! Matching-accuracy 1.0 — M0: per-run match-outcome telemetry.
//!
//! One row per completed metadata search run. Captures the **shape**
//! of the result (`single_good`, `multi_good`, `single_bad_cover`,
//! `multi_bad_cover`, `no_match`) plus the top + runner-up score so
//! the admin dashboard can render bucket distribution over rolling
//! windows + a 90-day "match quality" trend.
//!
//! Lands before any matcher tuning so we have a **before/after**
//! baseline once the ComicTagger-derived heuristics (M2 / M4 / M5)
//! ship. Without this row the only way to validate the plan would be
//! manual eyeballing of `metadata_run` counts, which doesn't separate
//! the "1 candidate but cover doesn't match" case from "1 candidate +
//! decisive cover match" — the very signal M4 will introduce.
//!
//! Retention: 90 days. Pruned by the existing scan-runs nightly cron
//! ([`crate::jobs::scheduler::register_scan_runs_prune`]'s sibling).
//! Pre-`metadata_run` CASCADE drop is also wired (the FK does the
//! heavy lifting for any explicit run deletion), so the prune is
//! belt-and-braces.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS metadata_match_outcome (
                id              UUID PRIMARY KEY,
                run_id          UUID NOT NULL REFERENCES metadata_run(id) ON DELETE CASCADE,
                scope           TEXT NOT NULL,
                outcome_kind    TEXT NOT NULL,
                top_score       REAL NOT NULL,
                top_hamming     INTEGER,
                second_score    REAL,
                second_hamming  INTEGER,
                candidate_count INTEGER NOT NULL,
                created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )"#,
        )
        .await?;

        // Date-range scan for the dashboard "last 7d / 28d" tiles. Index
        // on (created_at) is the only one that pays — outcome_kind has
        // only 5 distinct values so a btree on it would be skipped by
        // the planner.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS metadata_match_outcome_created_at_idx \
             ON metadata_match_outcome (created_at)",
        )
        .await?;

        // The FK already declares ON DELETE CASCADE, but we also want
        // a fast lookup by run for cases where the dashboard joins
        // back into `metadata_run` (e.g. the per-library filter).
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS metadata_match_outcome_run_id_idx \
             ON metadata_match_outcome (run_id)",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS metadata_match_outcome")
            .await?;
        Ok(())
    }
}
