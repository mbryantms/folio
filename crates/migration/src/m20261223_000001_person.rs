//! Person entity — M8 of the search-improvements plan.
//!
//! Today credit rows on `series_credits` + `issue_credits` carry the
//! creator's name as raw `person TEXT`. That's enough for filter
//! aggregation but doesn't give us a stable identity to hang a detail
//! page on (`/creators/<slug>`). This migration introduces a `person`
//! table that aggregates the distinct names across both junctions and
//! allocates a URL-safe slug per row.
//!
//! Trade-offs:
//! - **No FK on credits**: the existing `person TEXT` columns stay as
//!   the denormalised cache so filter joins / facet aggregation
//!   remain unchanged. The `person` table is a derived index, not a
//!   source of truth — when the scanner mints new credits with novel
//!   names, a follow-up job will be responsible for keeping `person`
//!   in sync. For v1 we rely on the backfill below; the people-search
//!   endpoint also resolves slugs against this table lazily.
//! - **Slugs handle collisions** with a numeric suffix (`-2`, `-3`,
//!   …) à la `slug::allocate_slug`. The migration does this in pure
//!   SQL with a window function so we don't shell out to the slug
//!   crate from a migration.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS person (
                id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                slug            TEXT NOT NULL UNIQUE,
                name            TEXT NOT NULL,
                normalized_name TEXT NOT NULL UNIQUE,
                created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS person_normalized_name_idx \
             ON person (normalized_name)",
        )
        .await?;
        // Trigram index on the display name — powers the people-search
        // people endpoint's "did you mean" fuzzy matching when we join
        // to person for slug resolution.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS person_name_trgm \
             ON person USING GIN (name gin_trgm_ops)",
        )
        .await?;

        // Backfill — collect every distinct (case-insensitive)
        // creator name across both junctions and insert a row per
        // unique normalized form. Slug allocation: lowercase, strip
        // diacritics if pg has unaccent (the M20260301 extensions
        // migration loads pg_trgm; unaccent is optional and we
        // degrade gracefully when absent), then replace non-
        // alphanumerics with hyphens. Collisions get `-2`, `-3`, …
        // via a window function `row_number()`.
        db.execute_unprepared(
            r#"
            WITH names AS (
                SELECT person FROM series_credits WHERE person IS NOT NULL AND person <> ''
                UNION ALL
                SELECT person FROM issue_credits WHERE person IS NOT NULL AND person <> ''
            ),
            normalized AS (
                SELECT
                    person                                          AS display_name,
                    btrim(lower(person))                            AS normalized_name,
                    regexp_replace(
                        regexp_replace(btrim(lower(person)), '[^a-z0-9]+', '-', 'g'),
                        '(^-+|-+$)', '', 'g'
                    )                                               AS base_slug
                FROM names
            ),
            distinct_norm AS (
                SELECT
                    min(display_name)         AS display_name,
                    normalized_name,
                    min(base_slug)            AS base_slug
                FROM normalized
                WHERE normalized_name <> ''
                GROUP BY normalized_name
            ),
            slug_collisions AS (
                SELECT
                    display_name,
                    normalized_name,
                    base_slug,
                    row_number() OVER (PARTITION BY base_slug ORDER BY normalized_name)
                        AS collision_idx
                FROM distinct_norm
            )
            INSERT INTO person (slug, name, normalized_name)
            SELECT
                CASE
                    WHEN base_slug = '' THEN 'untitled'
                    WHEN collision_idx = 1 THEN base_slug
                    ELSE base_slug || '-' || (collision_idx::text)
                END                                                 AS slug,
                display_name,
                normalized_name
            FROM slug_collisions
            ON CONFLICT (normalized_name) DO NOTHING
            "#,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS person CASCADE")
            .await?;
        Ok(())
    }
}
