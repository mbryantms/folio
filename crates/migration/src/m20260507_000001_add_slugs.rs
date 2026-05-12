//! Add `slug` columns and uniqueness constraints to libraries, series, and
//! issues — first migration of the human-readable-URLs work
//! (`~/.claude/plans/let-s-create-a-new-merry-finch.md`).
//!
//! Strategy:
//!   1. Add `slug TEXT NULL` to each table.
//!   2. Backfill existing rows with a Postgres-side slugifier (`unaccent` +
//!      regex). Collisions get a numeric `-2`, `-3`, … suffix in stable PK
//!      order. This is the v1 backfill — new rows allocated through
//!      `crate::slug::allocate_slug` use the smarter natural-disambiguator
//!      path (year/volume/publisher), but existing rows stay deterministic
//!      and the admin override can rename later.
//!   3. ALTER COLUMN slug SET NOT NULL.
//!   4. Add UNIQUE constraints — global on libraries(slug) and series(slug),
//!      composite on issues(series_id, slug).
//!
//! Issue slug source picks the first non-empty of `number_raw`, `title`, or
//! the issue id (BLAKE3 hex prefix). Issues with no number and no title get
//! a deterministic slug from the hash so the UNIQUE constraint always
//! holds.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        // ───── 1. Add nullable slug columns. ─────
        for sql in [
            "ALTER TABLE libraries ADD COLUMN slug TEXT NULL",
            "ALTER TABLE series ADD COLUMN slug TEXT NULL",
            "ALTER TABLE issues ADD COLUMN slug TEXT NULL",
        ] {
            conn.execute_unprepared(sql).await?;
        }

        // ───── 2a. Postgres-side slugifier helper. ─────
        // ASCII-folds via `unaccent`, lowercases, replaces runs of
        // non-alphanumeric with hyphens, trims edge hyphens, falls back to
        // `untitled` for empty inputs. Mirrors `slugify_segment` in
        // `crates/server/src/slug.rs` closely enough for backfill; new
        // rows always go through the Rust helper for canonical results.
        // Marked IMMUTABLE because it depends only on its argument and is
        // safe to inline in expression indexes / generated columns.
        conn.execute_unprepared(
            r#"
            CREATE OR REPLACE FUNCTION fl_slugify(input TEXT)
            RETURNS TEXT AS $$
                SELECT CASE
                    WHEN length(s) = 0 THEN 'untitled'
                    ELSE substring(s FROM 1 FOR 80)
                END
                FROM (
                    SELECT trim(both '-' FROM
                        regexp_replace(
                            regexp_replace(
                                lower(unaccent(coalesce(input, ''))),
                                '[^a-z0-9]+', '-', 'g'
                            ),
                            '-+', '-', 'g'
                        )
                    ) AS s
                ) AS t
            $$ LANGUAGE SQL IMMUTABLE
            "#,
        )
        .await?;

        // ───── 2b. Backfill libraries. ─────
        // Each library's slug = fl_slugify(name); duplicates within the
        // same base get `-2`, `-3`, … in `created_at, id` order so the
        // assignment is deterministic and idempotent.
        conn.execute_unprepared(
            r#"
            WITH ranked AS (
                SELECT id,
                       fl_slugify(name) AS base,
                       row_number() OVER (
                           PARTITION BY fl_slugify(name)
                           ORDER BY created_at, id
                       ) AS rn
                FROM libraries
            )
            UPDATE libraries
            SET slug = CASE WHEN ranked.rn = 1
                            THEN ranked.base
                            ELSE ranked.base || '-' || ranked.rn::text
                       END
            FROM ranked
            WHERE libraries.id = ranked.id
            "#,
        )
        .await?;

        // ───── 2c. Backfill series. ─────
        // Series uniqueness is GLOBAL, so the partition is on the base
        // slug alone (not scoped by library). Two libraries holding "Saga"
        // collapse onto `saga` / `saga-2`, etc.
        conn.execute_unprepared(
            r#"
            WITH ranked AS (
                SELECT id,
                       fl_slugify(name) AS base,
                       row_number() OVER (
                           PARTITION BY fl_slugify(name)
                           ORDER BY created_at, id
                       ) AS rn
                FROM series
            )
            UPDATE series
            SET slug = CASE WHEN ranked.rn = 1
                            THEN ranked.base
                            ELSE ranked.base || '-' || ranked.rn::text
                       END
            FROM ranked
            WHERE series.id = ranked.id
            "#,
        )
        .await?;

        // ───── 2d. Backfill issues. ─────
        // Issue uniqueness is per series. Source priority for the base
        // slug:
        //   1. `number_raw` (the most common case — issues numbered "1",
        //      "1.5", "Annual 1", etc).
        //   2. `title` (specials/one-shots without a number).
        //   3. First 8 chars of the BLAKE3 id (truly anonymous archives).
        //      We always have an id, so the column is never empty.
        // Numeric collision suffix, scoped per series.
        conn.execute_unprepared(
            r#"
            WITH base AS (
                SELECT id, series_id,
                       fl_slugify(
                           coalesce(
                               nullif(trim(coalesce(number_raw, '')), ''),
                               nullif(trim(coalesce(title, '')), ''),
                               substring(id FROM 1 FOR 8)
                           )
                       ) AS s,
                       created_at
                FROM issues
            ),
            ranked AS (
                SELECT id, s,
                       row_number() OVER (
                           PARTITION BY series_id, s
                           ORDER BY created_at, id
                       ) AS rn
                FROM base
            )
            UPDATE issues
            SET slug = CASE WHEN ranked.rn = 1
                            THEN ranked.s
                            ELSE ranked.s || '-' || ranked.rn::text
                       END
            FROM ranked
            WHERE issues.id = ranked.id
            "#,
        )
        .await?;

        // ───── 3. Enforce NOT NULL. ─────
        for sql in [
            "ALTER TABLE libraries ALTER COLUMN slug SET NOT NULL",
            "ALTER TABLE series ALTER COLUMN slug SET NOT NULL",
            "ALTER TABLE issues ALTER COLUMN slug SET NOT NULL",
        ] {
            conn.execute_unprepared(sql).await?;
        }

        // ───── 4. Uniqueness. ─────
        for sql in [
            "CREATE UNIQUE INDEX libraries_slug_uniq ON libraries(slug)",
            "CREATE UNIQUE INDEX series_slug_uniq ON series(slug)",
            "CREATE UNIQUE INDEX issues_series_slug_uniq ON issues(series_id, slug)",
        ] {
            conn.execute_unprepared(sql).await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for sql in [
            "DROP INDEX IF EXISTS issues_series_slug_uniq",
            "DROP INDEX IF EXISTS series_slug_uniq",
            "DROP INDEX IF EXISTS libraries_slug_uniq",
            "ALTER TABLE issues DROP COLUMN IF EXISTS slug",
            "ALTER TABLE series DROP COLUMN IF EXISTS slug",
            "ALTER TABLE libraries DROP COLUMN IF EXISTS slug",
            "DROP FUNCTION IF EXISTS fl_slugify(TEXT)",
        ] {
            conn.execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
