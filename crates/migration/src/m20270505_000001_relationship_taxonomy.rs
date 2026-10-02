//! Relationship taxonomy, arc targets and scoped links (WP-7.5, roadmap M7b).
//!
//! **Kinds.** `series_relationship.kind` grows from nine kinds to 31, in
//! four UI groups (Story · Publication history · Editions & contents ·
//! Advanced). `prequel_of` stops being the inverse of `sequel_of`: the new
//! pairs are `sequel_of ↔ has_sequel` and `prequel_of ↔ has_prequel` (a
//! *narrative* prequel: written later, set earlier). Publication continuity
//! (`continues ↔ continued_by`) is now distinct from a narrative sequel. The
//! CHECK mirrors `server::relationships::RelationshipKind`.
//!
//! **Arc targets.** A row may point at a `story_arc` (`to_arc_id`) instead of
//! a series; exactly one of `to_series_id` / `to_arc_id` is set. Arc rows are
//! one-directional (no inverse row: an arc isn't a series) and only
//! `tie_in_to`. The old `UNIQUE (from, to, kind)` becomes two partial unique
//! indexes, one per target type.
//!
//! **Scope.** Nullable `from_range` / `to_range` (issue-number ranges,
//! ≤ 100 chars), `coverage` (`full | partial | unknown`, collects / reprints
//! and their inverses only), `qualifier` (continuation: `relaunch | retitle |
//! merge | split | numbering`; tie-in role: `main | tie_in | prelude |
//! aftermath`) and `note` (≤ 500 chars).
//!
//! **Data.** In `series_relationship`:
//! - every `prequel_of` row becomes `has_sequel` (under the old model
//!   `prequel_of` was only ever the inverse half of `sequel_of`);
//! - every `source = 'suggested'` `sequel_of` / `has_sequel` pair whose
//!   accepted suggestion came from name/volume continuation or a shared
//!   provider volume becomes `continues` / `continued_by`. Manual
//!   `sequel_of` stays (it was the admin's explicit choice).
//!
//! In `series_relationship_suggestion`: pending / stale / accepted
//! `sequel_of` rows from those sources become `continues` (accepted rows so
//! the "never re-suggest" dedupe keeps matching), and `accepted_kind =
//! 'prequel_of'` becomes `has_sequel` (same meaning). Rejected and modified
//! rows keep their kind.
//!
//! **Down is lossy** (documented in `docs/dev/series-relationships.md`):
//! arc-target rows and every scope value are dropped; `has_sequel` /
//! `continued_by` become `prequel_of`, `continues` / `has_prequel` become
//! `sequel_of`, and every other new kind becomes `see_also`, keeping one row
//! where two now collide on the old unique key. Suggestions: `continues` →
//! `sequel_of` (dropped when a `sequel_of` row for the pair already exists),
//! rows of any other new kind are deleted, and `accepted_kind` is mapped
//! like the edges.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

/// Every kind (both halves of every pair).
const ALL_KINDS: &str = "'sequel_of', 'has_sequel', 'prequel_of', 'has_prequel', \
     'spin_off_of', 'has_spin_off', 'side_story_of', 'has_side_story', \
     'tie_in_to', 'has_tie_in', 'crossover_with', 'companion_to', \
     'same_universe', 'see_also', \
     'continues', 'continued_by', 'annual_of', 'has_annual', \
     'supplement_to', 'has_supplement', \
     'collects', 'collected_in', 'reprints', 'reprinted_in', \
     'alternate_edition_of', 'translation_of', 'has_translation', \
     'adaptation_of', 'adapted_as', 'reimagining_of', 'reimagined_as'";

/// The kinds a suggestion row may carry: one direction of every directional
/// pair, plus the self-inverse kinds (stored with `from < to`).
const CANONICAL_KINDS: &str = "'sequel_of', 'prequel_of', 'spin_off_of', 'side_story_of', \
     'tie_in_to', 'continues', 'annual_of', 'supplement_to', 'collects', \
     'reprints', 'translation_of', 'adaptation_of', 'reimagining_of', \
     'crossover_with', 'companion_to', 'same_universe', 'see_also', \
     'alternate_edition_of'";

const SELF_INVERSE_KINDS: &str =
    "'crossover_with', 'companion_to', 'same_universe', 'see_also', 'alternate_edition_of'";

const OLD_KINDS: &str = "'sequel_of', 'prequel_of', 'spin_off_of', 'has_spin_off', \
     'crossover_with', 'collects', 'collected_in', 'same_universe', 'see_also'";

const OLD_SUGGESTION_KINDS: &str =
    "'sequel_of', 'spin_off_of', 'collects', 'crossover_with', 'same_universe', 'see_also'";

/// `evidence.sources[]` names a continuation source (name/volume
/// continuation or a shared provider volume) — the only two sources that
/// ever emitted `sequel_of`. Alias `g` = the suggestion row.
const CONTINUATION_EVIDENCE: &str = "EXISTS (
    SELECT 1 FROM jsonb_array_elements(
        CASE WHEN jsonb_typeof(g.evidence -> 'sources') = 'array'
             THEN g.evidence -> 'sources' ELSE '[]'::jsonb END) AS e(src)
     WHERE e.src ->> 'source' IN ('name_continuation', 'provider_volume'))";

/// New kind → the closest pre-WP-7.5 kind (`down`).
const DOWN_KIND_CASE: &str = "CASE kind
    WHEN 'has_sequel'   THEN 'prequel_of'
    WHEN 'continued_by' THEN 'prequel_of'
    WHEN 'continues'    THEN 'sequel_of'
    WHEN 'has_prequel'  THEN 'sequel_of'
    WHEN 'sequel_of'      THEN 'sequel_of'
    WHEN 'prequel_of'     THEN 'prequel_of'
    WHEN 'spin_off_of'    THEN 'spin_off_of'
    WHEN 'has_spin_off'   THEN 'has_spin_off'
    WHEN 'crossover_with' THEN 'crossover_with'
    WHEN 'collects'       THEN 'collects'
    WHEN 'collected_in'   THEN 'collected_in'
    WHEN 'same_universe'  THEN 'same_universe'
    ELSE 'see_also' END";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for sql in up_statements() {
            db.execute_unprepared(&sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for sql in down_statements() {
            db.execute_unprepared(&sql).await?;
        }
        Ok(())
    }
}

fn up_statements() -> Vec<String> {
    vec![
        // ── series_relationship: widen ──
        "ALTER TABLE series_relationship \
           DROP CONSTRAINT IF EXISTS series_relationship_kind_chk, \
           DROP CONSTRAINT IF EXISTS series_relationship_uniq"
            .to_owned(),
        "ALTER TABLE series_relationship ALTER COLUMN to_series_id DROP NOT NULL".to_owned(),
        "ALTER TABLE series_relationship \
           ADD COLUMN IF NOT EXISTS to_arc_id UUID REFERENCES story_arc(id) ON DELETE CASCADE, \
           ADD COLUMN IF NOT EXISTS from_range TEXT, \
           ADD COLUMN IF NOT EXISTS to_range TEXT, \
           ADD COLUMN IF NOT EXISTS coverage TEXT, \
           ADD COLUMN IF NOT EXISTS qualifier TEXT, \
           ADD COLUMN IF NOT EXISTS note TEXT"
            .to_owned(),
        // ── data (a): the old inverse half of sequel_of ──
        "UPDATE series_relationship SET kind = 'has_sequel' WHERE kind = 'prequel_of'".to_owned(),
        // ── data (b): accepted continuation suggestions → continues ──
        format!(
            "WITH cont AS (
                SELECT g.from_series_id, g.to_series_id
                  FROM series_relationship_suggestion g
                 WHERE g.status = 'accepted' AND g.kind = 'sequel_of'
                   AND {CONTINUATION_EVIDENCE}
             )
             UPDATE series_relationship r
                SET kind = CASE r.kind WHEN 'sequel_of' THEN 'continues' ELSE 'continued_by' END
               FROM cont c
              WHERE r.source = 'suggested'
                AND ((r.kind = 'sequel_of'
                      AND r.from_series_id = c.from_series_id
                      AND r.to_series_id = c.to_series_id)
                  OR (r.kind = 'has_sequel'
                      AND r.from_series_id = c.to_series_id
                      AND r.to_series_id = c.from_series_id))"
        ),
        // ── series_relationship: constraints ──
        format!(
            "ALTER TABLE series_relationship
               ADD CONSTRAINT series_relationship_kind_chk CHECK (kind IN ({ALL_KINDS})),
               ADD CONSTRAINT series_relationship_target_chk
                   CHECK (num_nonnulls(to_series_id, to_arc_id) = 1),
               ADD CONSTRAINT series_relationship_arc_kind_chk
                   CHECK (to_arc_id IS NULL OR kind = 'tie_in_to'),
               ADD CONSTRAINT series_relationship_coverage_chk
                   CHECK (coverage IS NULL OR (coverage IN ('full', 'partial', 'unknown')
                          AND kind IN ('collects', 'collected_in', 'reprints', 'reprinted_in'))),
               ADD CONSTRAINT series_relationship_qualifier_chk
                   CHECK (qualifier IS NULL
                          OR (kind IN ('continues', 'continued_by')
                              AND qualifier IN ('relaunch', 'retitle', 'merge', 'split', 'numbering'))
                          OR (kind IN ('tie_in_to', 'has_tie_in')
                              AND qualifier IN ('main', 'tie_in', 'prelude', 'aftermath'))),
               ADD CONSTRAINT series_relationship_range_len_chk
                   CHECK (char_length(from_range) <= 100 AND char_length(to_range) <= 100),
               ADD CONSTRAINT series_relationship_note_len_chk
                   CHECK (char_length(note) <= 500)"
        ),
        "CREATE UNIQUE INDEX IF NOT EXISTS series_relationship_series_uniq \
           ON series_relationship (from_series_id, to_series_id, kind) \
           WHERE to_series_id IS NOT NULL"
            .to_owned(),
        "CREATE UNIQUE INDEX IF NOT EXISTS series_relationship_arc_uniq \
           ON series_relationship (from_series_id, to_arc_id, kind) \
           WHERE to_arc_id IS NOT NULL"
            .to_owned(),
        // "Which series tie in to arc X" (the arc page's tie-in list, FK cascade).
        "CREATE INDEX IF NOT EXISTS series_relationship_to_arc \
           ON series_relationship (to_arc_id) WHERE to_arc_id IS NOT NULL"
            .to_owned(),
        // ── series_relationship_suggestion ──
        "ALTER TABLE series_relationship_suggestion \
           DROP CONSTRAINT IF EXISTS series_relationship_suggestion_kind_chk, \
           DROP CONSTRAINT IF EXISTS series_relationship_suggestion_canonical_chk, \
           DROP CONSTRAINT IF EXISTS series_relationship_suggestion_accepted_kind_chk"
            .to_owned(),
        format!(
            "UPDATE series_relationship_suggestion g SET kind = 'continues'
              WHERE g.kind = 'sequel_of'
                AND g.status IN ('pending', 'stale', 'accepted')
                AND {CONTINUATION_EVIDENCE}"
        ),
        "UPDATE series_relationship_suggestion SET accepted_kind = 'has_sequel' \
          WHERE accepted_kind = 'prequel_of'"
            .to_owned(),
        format!(
            "ALTER TABLE series_relationship_suggestion
               ADD CONSTRAINT series_relationship_suggestion_kind_chk
                   CHECK (kind IN ({CANONICAL_KINDS})),
               ADD CONSTRAINT series_relationship_suggestion_canonical_chk
                   CHECK (kind NOT IN ({SELF_INVERSE_KINDS}) OR from_series_id < to_series_id),
               ADD CONSTRAINT series_relationship_suggestion_accepted_kind_chk
                   CHECK (accepted_kind IS NULL OR accepted_kind IN ({ALL_KINDS}))"
        ),
    ]
}

fn down_statements() -> Vec<String> {
    vec![
        // ── series_relationship ──
        "DELETE FROM series_relationship WHERE to_arc_id IS NOT NULL".to_owned(),
        "ALTER TABLE series_relationship \
           DROP CONSTRAINT IF EXISTS series_relationship_kind_chk, \
           DROP CONSTRAINT IF EXISTS series_relationship_target_chk, \
           DROP CONSTRAINT IF EXISTS series_relationship_arc_kind_chk, \
           DROP CONSTRAINT IF EXISTS series_relationship_coverage_chk, \
           DROP CONSTRAINT IF EXISTS series_relationship_qualifier_chk, \
           DROP CONSTRAINT IF EXISTS series_relationship_range_len_chk, \
           DROP CONSTRAINT IF EXISTS series_relationship_note_len_chk"
            .to_owned(),
        // Keep one row per (from, to, old kind): prefer a row already of the
        // old kind, then the oldest.
        format!(
            "WITH m AS (
                SELECT id, from_series_id, to_series_id, kind, created_at,
                       {DOWN_KIND_CASE} AS old_kind
                  FROM series_relationship
             ), ranked AS (
                SELECT id, row_number() OVER (
                         PARTITION BY from_series_id, to_series_id, old_kind
                         ORDER BY (kind = old_kind) DESC, created_at, id) AS rn
                  FROM m
             )
             DELETE FROM series_relationship r USING ranked k
              WHERE r.id = k.id AND k.rn > 1"
        ),
        format!(
            "UPDATE series_relationship SET kind = {DOWN_KIND_CASE} \
              WHERE kind NOT IN ({OLD_KINDS})"
        ),
        "DROP INDEX IF EXISTS series_relationship_to_arc".to_owned(),
        "DROP INDEX IF EXISTS series_relationship_arc_uniq".to_owned(),
        "DROP INDEX IF EXISTS series_relationship_series_uniq".to_owned(),
        "ALTER TABLE series_relationship \
           DROP COLUMN IF EXISTS to_arc_id, \
           DROP COLUMN IF EXISTS from_range, \
           DROP COLUMN IF EXISTS to_range, \
           DROP COLUMN IF EXISTS coverage, \
           DROP COLUMN IF EXISTS qualifier, \
           DROP COLUMN IF EXISTS note"
            .to_owned(),
        "ALTER TABLE series_relationship ALTER COLUMN to_series_id SET NOT NULL".to_owned(),
        format!(
            "ALTER TABLE series_relationship
               ADD CONSTRAINT series_relationship_uniq UNIQUE (from_series_id, to_series_id, kind),
               ADD CONSTRAINT series_relationship_kind_chk CHECK (kind IN ({OLD_KINDS}))"
        ),
        // ── series_relationship_suggestion ──
        "ALTER TABLE series_relationship_suggestion \
           DROP CONSTRAINT IF EXISTS series_relationship_suggestion_kind_chk, \
           DROP CONSTRAINT IF EXISTS series_relationship_suggestion_canonical_chk, \
           DROP CONSTRAINT IF EXISTS series_relationship_suggestion_accepted_kind_chk"
            .to_owned(),
        "DELETE FROM series_relationship_suggestion g \
          WHERE g.kind = 'continues' \
            AND EXISTS (SELECT 1 FROM series_relationship_suggestion o \
                         WHERE o.from_series_id = g.from_series_id \
                           AND o.to_series_id = g.to_series_id \
                           AND o.kind = 'sequel_of')"
            .to_owned(),
        "UPDATE series_relationship_suggestion SET kind = 'sequel_of' WHERE kind = 'continues'"
            .to_owned(),
        format!(
            "DELETE FROM series_relationship_suggestion WHERE kind NOT IN ({OLD_SUGGESTION_KINDS})"
        ),
        format!(
            "UPDATE series_relationship_suggestion
                SET accepted_kind = {}
              WHERE accepted_kind IS NOT NULL AND accepted_kind NOT IN ({OLD_KINDS})",
            DOWN_KIND_CASE.replace("CASE kind", "CASE accepted_kind")
        ),
        format!(
            "ALTER TABLE series_relationship_suggestion
               ADD CONSTRAINT series_relationship_suggestion_kind_chk
                   CHECK (kind IN ({OLD_SUGGESTION_KINDS})),
               ADD CONSTRAINT series_relationship_suggestion_canonical_chk
                   CHECK (kind NOT IN ('crossover_with', 'same_universe', 'see_also')
                          OR from_series_id < to_series_id),
               ADD CONSTRAINT series_relationship_suggestion_accepted_kind_chk
                   CHECK (accepted_kind IS NULL OR accepted_kind IN ({OLD_KINDS}))"
        ),
    ]
}
