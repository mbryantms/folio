//! Suggestions carry scope and arc targets (WP-7.6, roadmap M7b).
//!
//! WP-7.5 gave `series_relationship` arc targets and scope columns; the
//! WP-7.6 detectors propose both, so `series_relationship_suggestion` gets
//! the same shape:
//!
//! - `to_arc_id` (nullable FK → `story_arc`, ON DELETE CASCADE) and a
//!   nullable `to_series_id`; exactly one target (`num_nonnulls = 1`), and
//!   an arc target only for `tie_in_to` (the arc tie-in detector);
//! - `from_range` / `to_range` / `coverage` / `qualifier`, with the same
//!   CHECKs as `series_relationship` (`m20270505_000001_relationship_taxonomy`).
//!   No `note`: suggestions explain themselves through `reason`;
//! - the old `UNIQUE (from, to, kind)` becomes two partial unique indexes,
//!   one per target type (the upsert's `ON CONFLICT` targets).
//!
//! The canonical-form CHECKs are unchanged: an arc row is always
//! `tie_in_to` (directional, canonical), so `from < to` never applies to it.
//!
//! **Down is lossy**: arc-target rows are deleted and the scope columns
//! dropped (CI's migration round-trip runs it).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for sql in UP {
            db.execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for sql in DOWN {
            db.execute_unprepared(sql).await?;
        }
        Ok(())
    }
}

const UP: &[&str] = &[
    "ALTER TABLE series_relationship_suggestion \
       DROP CONSTRAINT IF EXISTS series_relationship_suggestion_uniq",
    "ALTER TABLE series_relationship_suggestion ALTER COLUMN to_series_id DROP NOT NULL",
    "ALTER TABLE series_relationship_suggestion \
       ADD COLUMN IF NOT EXISTS to_arc_id UUID REFERENCES story_arc(id) ON DELETE CASCADE, \
       ADD COLUMN IF NOT EXISTS from_range TEXT, \
       ADD COLUMN IF NOT EXISTS to_range TEXT, \
       ADD COLUMN IF NOT EXISTS coverage TEXT, \
       ADD COLUMN IF NOT EXISTS qualifier TEXT",
    "ALTER TABLE series_relationship_suggestion
       ADD CONSTRAINT series_relationship_suggestion_target_chk
           CHECK (num_nonnulls(to_series_id, to_arc_id) = 1),
       ADD CONSTRAINT series_relationship_suggestion_arc_kind_chk
           CHECK (to_arc_id IS NULL OR kind = 'tie_in_to'),
       ADD CONSTRAINT series_relationship_suggestion_coverage_chk
           CHECK (coverage IS NULL OR (coverage IN ('full', 'partial', 'unknown')
                  AND kind IN ('collects', 'collected_in', 'reprints', 'reprinted_in'))),
       ADD CONSTRAINT series_relationship_suggestion_qualifier_chk
           CHECK (qualifier IS NULL
                  OR (kind IN ('continues', 'continued_by')
                      AND qualifier IN ('relaunch', 'retitle', 'merge', 'split', 'numbering'))
                  OR (kind IN ('tie_in_to', 'has_tie_in')
                      AND qualifier IN ('main', 'tie_in', 'prelude', 'aftermath'))),
       ADD CONSTRAINT series_relationship_suggestion_range_len_chk
           CHECK (char_length(from_range) <= 100 AND char_length(to_range) <= 100)",
    "CREATE UNIQUE INDEX IF NOT EXISTS series_relationship_suggestion_series_uniq \
       ON series_relationship_suggestion (from_series_id, to_series_id, kind) \
       WHERE to_series_id IS NOT NULL",
    "CREATE UNIQUE INDEX IF NOT EXISTS series_relationship_suggestion_arc_uniq \
       ON series_relationship_suggestion (from_series_id, to_arc_id, kind) \
       WHERE to_arc_id IS NOT NULL",
    // FK cascade on arc delete.
    "CREATE INDEX IF NOT EXISTS series_relationship_suggestion_to_arc \
       ON series_relationship_suggestion (to_arc_id) WHERE to_arc_id IS NOT NULL",
];

const DOWN: &[&str] = &[
    "DELETE FROM series_relationship_suggestion WHERE to_arc_id IS NOT NULL",
    "ALTER TABLE series_relationship_suggestion \
       DROP CONSTRAINT IF EXISTS series_relationship_suggestion_target_chk, \
       DROP CONSTRAINT IF EXISTS series_relationship_suggestion_arc_kind_chk, \
       DROP CONSTRAINT IF EXISTS series_relationship_suggestion_coverage_chk, \
       DROP CONSTRAINT IF EXISTS series_relationship_suggestion_qualifier_chk, \
       DROP CONSTRAINT IF EXISTS series_relationship_suggestion_range_len_chk",
    "DROP INDEX IF EXISTS series_relationship_suggestion_to_arc",
    "DROP INDEX IF EXISTS series_relationship_suggestion_arc_uniq",
    "DROP INDEX IF EXISTS series_relationship_suggestion_series_uniq",
    "ALTER TABLE series_relationship_suggestion \
       DROP COLUMN IF EXISTS to_arc_id, \
       DROP COLUMN IF EXISTS from_range, \
       DROP COLUMN IF EXISTS to_range, \
       DROP COLUMN IF EXISTS coverage, \
       DROP COLUMN IF EXISTS qualifier",
    "ALTER TABLE series_relationship_suggestion ALTER COLUMN to_series_id SET NOT NULL",
    "ALTER TABLE series_relationship_suggestion \
       ADD CONSTRAINT series_relationship_suggestion_uniq \
       UNIQUE (from_series_id, to_series_id, kind)",
];
