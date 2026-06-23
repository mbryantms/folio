//! Per-(series, provider) issue-range mapping — provider series-boundary
//! divergence support.
//!
//! Metadata providers disagree on *series boundaries*. ComicVine is a
//! "lumper" (e.g. all 173 issues of Fantastic Four including the
//! legacy-renumbered #600–611 live in one volume); Metron and GCD are
//! "splitters" (#600–611 are a separate series, "Fantastic Four (2012)",
//! start year 2012). The single per-source slot on `external_ids`
//! (PK `(entity_type, entity_id, source)`) can only record ONE provider
//! series per local series per source, so the divergent range can never
//! match the splitter.
//!
//! This table records the *exceptions* to the default whole-series
//! mapping: per local series and per source, a contiguous issue-number
//! range that routes to a DIFFERENT provider series. The series-level
//! `external_ids` row stays the implicit "whole series" default; a range
//! row overrides only the issues whose canonical number falls inside it.
//! An empty table reproduces today's behaviour exactly.
//!
//! `range_low` / `range_high` are *canonical* issue-number strings
//! (see `metadata::matcher::canonical_issue_number`) so comparisons line
//! up with the scanner's stored `number_raw`. NULL on either bound means
//! open-ended. `declared_year` is the sub-series' start year, consulted
//! by the issue-search year gate so the splitter candidate (2012) isn't
//! dropped against the parent series year (2001). The local `series` row
//! is never mutated — keeping `series_id` / issue ids / `normalized_name`
//! stable, which is what keeps CBL reading-list resolution unaffected.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // source strings mirror external_ids.source:
        // 'comicvine'|'metron'|'gcd'|'marvel'|'locg'|'mal'|'anilist'|
        // 'mangaupdates'|'isbn'|'upc'|'asin'|'doi'.
        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS series_provider_range (
                id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                series_id            UUID NOT NULL REFERENCES series(id) ON DELETE CASCADE,
                source               TEXT NOT NULL,
                provider_series_id   TEXT NOT NULL,
                provider_series_url  TEXT,
                provider_series_name TEXT,
                range_low            TEXT,
                range_high           TEXT,
                declared_year        INTEGER,
                set_by               TEXT NOT NULL,
                first_set_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
                last_synced_at       TIMESTAMPTZ NOT NULL DEFAULT now()
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS series_provider_range_by_series \
             ON series_provider_range (series_id, source)",
        )
        .await?;
        // Backstop the auto-split / detect check-then-insert against
        // duplicate identical mappings under concurrency (the apply hook +
        // the manual detect endpoint aren't serialized). App-side overlap
        // validation still gives the friendly 409; this catches the race.
        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS series_provider_range_uniq \
             ON series_provider_range (series_id, source, range_low, range_high)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS series_provider_range")
            .await?;
        Ok(())
    }
}
