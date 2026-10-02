//! `series_relationship_suggestion` — candidate relationships proposed by the
//! suggestion engine (WP-7.2, spec §5.7 / Phase 7).
//!
//! The engine never writes `series_relationship` itself: it upserts rows
//! here, and an admin accepts (→ `create_pair` with `source = 'suggested'`)
//! or rejects them (WP-7.3 builds the review UI on top).
//!
//! **Canonical form.** One row per `(from, to, kind)` with:
//! - self-inverse kinds (`crossover_with`, `same_universe`, `see_also`)
//!   stored with `from_series_id < to_series_id`, so A→B and B→A dedupe;
//! - directional kinds stored in one fixed direction only — `sequel_of`
//!   (never `prequel_of`), `spin_off_of` (never `has_spin_off`) and
//!   `collects` (never `collected_in`).
//!
//! Both rules are CHECK constraints so a writer bug can't fork a pair.
//!
//! **Append-only.** Rows are never deleted (spec §5.7: "never delete
//! suggestions, just mark status"); only the series FK cascade removes them.
//! A rejected row is the engine's memory: re-runs skip any `(from, to, kind)`
//! whose row is not `pending`. `accepted_kind` is set when an admin accepted
//! with a different kind (`status = 'modified'`).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS series_relationship_suggestion (
                id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                from_series_id  UUID NOT NULL REFERENCES series(id) ON DELETE CASCADE,
                to_series_id    UUID NOT NULL REFERENCES series(id) ON DELETE CASCADE,
                kind            TEXT NOT NULL,
                confidence      REAL NOT NULL,
                bucket          TEXT NOT NULL,
                reason          TEXT NOT NULL,
                evidence        JSONB NOT NULL DEFAULT '{}'::jsonb,
                status          TEXT NOT NULL DEFAULT 'pending',
                accepted_kind   TEXT,
                created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
                reviewed_at     TIMESTAMPTZ,
                reviewed_by     UUID REFERENCES users(id) ON DELETE SET NULL,
                CONSTRAINT series_relationship_suggestion_uniq
                    UNIQUE (from_series_id, to_series_id, kind),
                CONSTRAINT series_relationship_suggestion_no_self
                    CHECK (from_series_id <> to_series_id),
                CONSTRAINT series_relationship_suggestion_kind_chk
                    CHECK (kind IN ('sequel_of', 'spin_off_of', 'collects',
                                    'crossover_with', 'same_universe', 'see_also')),
                CONSTRAINT series_relationship_suggestion_canonical_chk
                    CHECK (kind NOT IN ('crossover_with', 'same_universe', 'see_also')
                           OR from_series_id < to_series_id),
                CONSTRAINT series_relationship_suggestion_accepted_kind_chk
                    CHECK (accepted_kind IS NULL OR accepted_kind IN (
                        'sequel_of', 'prequel_of', 'spin_off_of', 'has_spin_off',
                        'crossover_with', 'collects', 'collected_in',
                        'same_universe', 'see_also')),
                CONSTRAINT series_relationship_suggestion_status_chk
                    CHECK (status IN ('pending', 'accepted', 'rejected', 'modified')),
                CONSTRAINT series_relationship_suggestion_bucket_chk
                    CHECK (bucket IN ('high', 'medium', 'low')),
                CONSTRAINT series_relationship_suggestion_confidence_chk
                    CHECK (confidence >= 0 AND confidence <= 1)
            )"#,
        )
        .await?;
        // Reverse lookup (cascade deletes, "suggestions touching series X").
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS series_relationship_suggestion_to_series \
             ON series_relationship_suggestion (to_series_id)",
        )
        .await?;
        // The review list: pending first, by confidence (keyset on
        // `(confidence DESC, id)`).
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS series_relationship_suggestion_status_conf \
             ON series_relationship_suggestion (status, confidence DESC, id)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS series_relationship_suggestion")
            .await?;
        Ok(())
    }
}
