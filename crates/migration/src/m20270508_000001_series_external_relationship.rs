//! Provider links and external targets (WP-7.8, roadmap M7b).
//!
//! - `series_external_relationship`: a relationship from a local series to
//!   a **provider** series that is not (yet) in the library — "Continued
//!   by: Saga (2018), not in your library". One-directional (there is no
//!   local series to hold an inverse half). `set_by = 'provider'` rows come
//!   from provider data (Metron series `associated`); `set_by = 'user'` rows
//!   are added by an admin. When the target series shows up locally
//!   (matched through `external_ids`), a user row is promoted to a real
//!   `series_relationship` pair and deleted; a provider row is marked
//!   (`promoted_series_id`) and feeds the suggestion engine. Precedent:
//!   `series_provider_range` also stores provider series that may not be
//!   local. Removing a provider row only **dismisses** it (`dismissed_at`),
//!   so the next apply doesn't bring it back (rejection memory); user rows
//!   are deleted.
//! - `issue_reprints.reprinted_source` / `reprinted_external_id`: the
//!   provider id of a reprinted issue that isn't in the library yet, so a
//!   label-only reprint row can be resolved to `reprinted_issue_id` once
//!   the issue is scanned in.
//!
//! Down drops the table, the index and the two columns (lossless for
//! everything that existed before).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(&format!(
            "CREATE TABLE IF NOT EXISTS series_external_relationship (
                id                   UUID PRIMARY KEY,
                from_series_id       UUID NOT NULL REFERENCES series(id) ON DELETE CASCADE,
                kind                 TEXT NOT NULL,
                qualifier            TEXT,
                source               TEXT NOT NULL,
                provider_series_id   TEXT NOT NULL,
                provider_series_name TEXT,
                provider_series_url  TEXT,
                provider_year        INTEGER,
                set_by               TEXT NOT NULL,
                confidence           REAL,
                evidence             JSONB NOT NULL DEFAULT '{{}}'::jsonb,
                created_by           UUID REFERENCES users(id) ON DELETE SET NULL,
                promoted_series_id   UUID REFERENCES series(id) ON DELETE SET NULL,
                dismissed_at         TIMESTAMPTZ,
                dismissed_by         UUID REFERENCES users(id) ON DELETE SET NULL,
                first_set_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
                last_synced_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
                CONSTRAINT series_external_relationship_uniq
                    UNIQUE (from_series_id, kind, source, provider_series_id),
                CONSTRAINT series_external_relationship_kind_chk
                    CHECK (kind IN ({ALL_KINDS})),
                CONSTRAINT series_external_relationship_qualifier_chk
                    CHECK (qualifier IS NULL
                           OR (kind IN ('continues', 'continued_by')
                               AND qualifier IN ('relaunch', 'retitle', 'merge', 'split', 'numbering'))
                           OR (kind IN ('tie_in_to', 'has_tie_in')
                               AND qualifier IN ('main', 'tie_in', 'prelude', 'aftermath'))),
                CONSTRAINT series_external_relationship_source_chk
                    CHECK (source IN ('metron', 'comicvine', 'gcd')),
                CONSTRAINT series_external_relationship_set_by_chk
                    CHECK (set_by IN ('user', 'provider')),
                CONSTRAINT series_external_relationship_confidence_chk
                    CHECK (confidence IS NULL OR (confidence >= 0 AND confidence <= 1)),
                CONSTRAINT series_external_relationship_pid_chk
                    CHECK (char_length(provider_series_id) BETWEEN 1 AND 64),
                CONSTRAINT series_external_relationship_name_chk
                    CHECK (char_length(provider_series_name) <= 300),
                CONSTRAINT series_external_relationship_url_chk
                    CHECK (char_length(provider_series_url) <= 500)
            )"
        ))
        .await?;
        // Promotion looks rows up by the provider series they point at.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS series_external_relationship_provider \
               ON series_external_relationship (source, provider_series_id)",
        )
        .await?;
        // FK SET NULL on series delete.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS series_external_relationship_promoted \
               ON series_external_relationship (promoted_series_id) \
               WHERE promoted_series_id IS NOT NULL",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE issue_reprints \
               ADD COLUMN IF NOT EXISTS reprinted_source TEXT, \
               ADD COLUMN IF NOT EXISTS reprinted_external_id TEXT",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS issue_reprints_pending_external \
               ON issue_reprints (reprinted_source, reprinted_external_id) \
               WHERE reprinted_issue_id IS NULL AND reprinted_external_id IS NOT NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS issue_reprints_pending_external")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE issue_reprints \
               DROP COLUMN IF EXISTS reprinted_source, \
               DROP COLUMN IF EXISTS reprinted_external_id",
        )
        .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS series_external_relationship")
            .await?;
        Ok(())
    }
}

/// Every relationship kind (both halves of every pair) — the same list as
/// `series_relationship_kind_chk` (`m20270505_000001_relationship_taxonomy`).
const ALL_KINDS: &str = "'sequel_of', 'has_sequel', 'prequel_of', 'has_prequel', \
     'spin_off_of', 'has_spin_off', 'side_story_of', 'has_side_story', \
     'tie_in_to', 'has_tie_in', 'crossover_with', 'companion_to', \
     'same_universe', 'see_also', \
     'continues', 'continued_by', 'annual_of', 'has_annual', \
     'supplement_to', 'has_supplement', \
     'collects', 'collected_in', 'reprints', 'reprinted_in', \
     'alternate_edition_of', 'translation_of', 'has_translation', \
     'adaptation_of', 'adapted_as', 'reimagining_of', 'reimagined_as'";
