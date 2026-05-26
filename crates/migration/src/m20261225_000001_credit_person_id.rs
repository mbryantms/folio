//! Adds `person_id` to `series_credits` + `issue_credits` so credit
//! rows carry the canonical `person.id` (and via that, the creator's
//! slug) instead of relying on a name-string lookup. This closes the
//! "scanner mints new credits but the person table doesn't grow"
//! follow-up flagged in [`m20261223_000001_person`]'s doc comment.
//!
//! With this column in place, the API can JOIN credits to `person`
//! directly on `person_id` and surface `creator_slug` alongside the
//! existing `person TEXT` cache. Credit chips in the UI then link
//! straight to `/creators/<slug>` — same shape every other detail-page
//! navigation uses — without needing a name-resolving redirect route.
//!
//! `person TEXT` stays as the denormalised cache so filter/facet
//! aggregation queries (which group by name, not id) keep their
//! current cost. FK is `ON DELETE SET NULL` so admin-side person
//! cleanup doesn't blow away credits.
//!
//! Backfill matches on `btrim(lower(person)) = person.normalized_name`,
//! the same normalization the M8 migration used when populating the
//! `person` table in the first place — every existing credit
//! resolves on a fresh deploy.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        for table in ["series_credits", "issue_credits"] {
            db.execute_unprepared(&format!(
                "ALTER TABLE {table} \
                 ADD COLUMN IF NOT EXISTS person_id UUID \
                 REFERENCES person(id) ON DELETE SET NULL"
            ))
            .await?;

            // Index supports the API-side JOIN on person and the
            // scanner's bulk UPDATE during series rollup. Partial — most
            // pre-backfill rows will have `person_id` set after this
            // migration runs, but the partial index is still cheap and
            // keeps the index tight for the rare unresolved-name case.
            db.execute_unprepared(&format!(
                "CREATE INDEX IF NOT EXISTS {table}_person_id_idx \
                 ON {table} (person_id) \
                 WHERE person_id IS NOT NULL"
            ))
            .await?;

            // Backfill from the existing name cache. `normalized_name`
            // on `person` is `btrim(lower(...))`, so we mirror it here.
            db.execute_unprepared(&format!(
                "UPDATE {table} c \
                 SET person_id = p.id \
                 FROM person p \
                 WHERE c.person_id IS NULL \
                   AND p.normalized_name = btrim(lower(c.person))"
            ))
            .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for table in ["series_credits", "issue_credits"] {
            db.execute_unprepared(&format!("DROP INDEX IF EXISTS {table}_person_id_idx"))
                .await?;
            db.execute_unprepared(&format!(
                "ALTER TABLE {table} DROP COLUMN IF EXISTS person_id"
            ))
            .await?;
        }
        Ok(())
    }
}
