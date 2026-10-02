//! `series_relationship` — typed, directed edges between series (WP-7.1,
//! spec §5.2 / Phase 7).
//!
//! Every edge is stored together with its inverse (`A sequel_of B` ⇔
//! `B prequel_of A`), written and deleted in the same transaction by
//! `server::relationships::{create_pair, delete_pair}`, so traversal is
//! symmetric without UNION queries. Self-inverse kinds (`crossover_with`,
//! `same_universe`, `see_also`) store the reverse row with the same kind.
//!
//! `source` distinguishes hand-made edges (`manual`) from accepted
//! suggestions (`suggested`, WP-7.2) — `confidence` is the suggestion
//! engine's score and stays NULL for manual rows. `created_by` keeps the
//! admin who made the edge (SET NULL when the account is deleted).
//!
//! The kind CHECK mirrors `server::relationships::RelationshipKind`; adding
//! a kind needs both a migration and the enum variant.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS series_relationship (
                id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                from_series_id  UUID NOT NULL REFERENCES series(id) ON DELETE CASCADE,
                to_series_id    UUID NOT NULL REFERENCES series(id) ON DELETE CASCADE,
                kind            TEXT NOT NULL,
                source          TEXT NOT NULL DEFAULT 'manual',
                confidence      REAL,
                created_by      UUID REFERENCES users(id) ON DELETE SET NULL,
                created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
                CONSTRAINT series_relationship_uniq
                    UNIQUE (from_series_id, to_series_id, kind),
                CONSTRAINT series_relationship_no_self
                    CHECK (from_series_id <> to_series_id),
                CONSTRAINT series_relationship_kind_chk
                    CHECK (kind IN ('sequel_of', 'prequel_of', 'spin_off_of',
                                    'has_spin_off', 'crossover_with', 'collects',
                                    'collected_in', 'same_universe', 'see_also')),
                CONSTRAINT series_relationship_source_chk
                    CHECK (source IN ('manual', 'suggested')),
                CONSTRAINT series_relationship_confidence_chk
                    CHECK (confidence IS NULL OR (confidence >= 0 AND confidence <= 1))
            )"#,
        )
        .await?;
        // `from_series_id` is covered by the unique constraint's leading
        // column; the reverse lookup (cascade deletes, "who points at me")
        // needs its own index.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS series_relationship_to_series \
             ON series_relationship (to_series_id)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS series_relationship")
            .await?;
        Ok(())
    }
}
