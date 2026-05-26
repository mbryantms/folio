//! Metadata Providers 1.0 — M0: schema restructure.
//!
//! Foundational change that gates the ComicVine + Metron sync work.
//! Replaces the trio of fixed external-ID columns
//! (`comicvine_id`, `metron_id`, `gtin`) on `series` + `issues` with a
//! generic `external_ids` table that supports unlimited sources
//! (CV / Metron / GCD / Marvel / LoCG / MAL / AniList / MangaUpdates /
//! ISBN / UPC / ASIN / DOI), promotes nine cross-cut concepts
//! (character / team / story_arc / location / concept / object /
//! publisher / imprint / universe) to top-level entities mirroring
//! `person`'s shape, and adds the per-cover (`issue_cover`,
//! `series_cover`), per-field provenance (`field_provenance`), and
//! per-run history (`metadata_run`) tables that subsequent milestones
//! will write into.
//!
//! ## Non-destructive contract
//!
//! Existing data is preserved by backfilling everything *before*
//! dropping the old ID columns:
//!
//! 1. `external_ids` rows are inserted from the soon-to-be-dropped
//!    `{series,issue}.{comicvine_id,metron_id,gtin}` columns with
//!    `set_by='migration_v1'` so a `down()` rollback can move them back.
//! 2. New top-level entities (character / team / location / story_arc)
//!    are backfilled from existing junction strings — same DISTINCT-name
//!    + slug-allocation pattern [`m20261223_000001_person`] used.
//! 3. The existing string-keyed junctions (`issue_characters`,
//!    `issue_teams`, `issue_locations` and their `series_*` twins)
//!    get a nullable FK column alongside the name column, populated by
//!    a normalized-name JOIN — the exact precedent [`m20261225_000001_credit_person_id`]
//!    set for `person_id`. The string column stays as the denormalized
//!    cache the existing rollup pipeline reads.
//! 4. `issue.story_arc` CSVs are split into the new `story_arc` entity
//!    plus the new `issue_arcs` junction. The `issue.story_arc` and
//!    `issue.story_arc_number` columns stay as denormalized cache.
//! 5. `publisher` / `imprint` entities are seeded from
//!    `series.{publisher,imprint}` strings; `series.publisher_id` is
//!    populated via the same JOIN. String columns stay.
//! 6. `issue_cover` is seeded one row per issue with
//!    `thumbnails_generated_at IS NOT NULL`, pointing at the existing
//!    `cover.webp` path. New M4 cover writes use a different on-disk
//!    layout (`covers/{cover_id}.<ext>`); the schema is agnostic.
//! 7. `field_provenance` is seeded from the existing
//!    `issue.user_edited` JSON array — every entry becomes one row with
//!    `set_by='user'`.
//! 8. The six fixed ID columns are dropped only *after* steps 1–7
//!    succeed.
//!
//! ## CSV columns kept as denormalized read-cache
//!
//! The flat CSV columns on `issues` (`writer`, `penciller`, ...,
//! `characters`, `teams`, `locations`, `story_arc`, `genre`, `tags`)
//! are **not** touched by this migration. They stay as the
//! denormalized read-cache list-views read from, rebuilt from
//! junctions on every junction write via a helper that lands in
//! `crates/server/src/metadata/writers.rs` in M0b. Junctions become
//! sole source of truth on writes from M0c onward.
//!
//! ## Per-relationship metadata
//!
//! The existing junctions gain a few flags the providers expose:
//! - `issue_characters.is_first_appearance` + `.died_in_issue`
//! - `issue_teams.is_first_appearance` + `.disbanded_in_issue`
//! - `issue_locations.is_first_appearance`
//! - `issue_credits.ordinal` (Metron credits have stable ordering)
//!
//! All default FALSE / 0; the scanner doesn't have this signal today
//! and the M4 Apply jobs are the first writers that will populate them.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // ──────────────────────────────────────────────────────────────
        // §1  Top-level entity tables (mirror `person`'s shape)
        // ──────────────────────────────────────────────────────────────
        // All nine follow the same shape: UUID PK, URL-safe slug, display
        // name preserving the source's casing, normalized_name for dedup.
        // Provider-specific fields (bio / image_url / aliases / publisher_id)
        // are nullable additions — populated when the M4 Apply jobs first
        // touch the row.
        for (table, extra_cols) in ENTITY_TABLE_DEFS {
            let sql = format!(
                r#"CREATE TABLE IF NOT EXISTS {table} (
                    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                    slug            TEXT NOT NULL UNIQUE,
                    name            TEXT NOT NULL,
                    normalized_name TEXT NOT NULL UNIQUE,
                    aliases         JSONB NOT NULL DEFAULT '[]'::jsonb,
                    description     TEXT,
                    image_url       TEXT,
                    {extra_cols}
                    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
                    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
                )"#
            );
            db.execute_unprepared(&sql).await?;
            db.execute_unprepared(&format!(
                "CREATE INDEX IF NOT EXISTS {table}_normalized_name_idx \
                 ON {table} (normalized_name)"
            ))
            .await?;
            db.execute_unprepared(&format!(
                "CREATE INDEX IF NOT EXISTS {table}_name_trgm \
                 ON {table} USING GIN (name gin_trgm_ops)"
            ))
            .await?;
        }

        // ──────────────────────────────────────────────────────────────
        // §2  Add FK columns to existing string-keyed junctions
        // ──────────────────────────────────────────────────────────────
        // Mirrors m20261225's `person_id` pattern: nullable FK alongside
        // the name column, partial index on non-null. Backfill happens in
        // §9 once the entity tables exist. The composite (issue_id, name)
        // PK stays — the FK is a read-side accelerator + identity anchor,
        // not the dedup key.
        for (table, fk_col, target_table) in [
            ("issue_characters", "character_id", "character"),
            ("series_characters", "character_id", "character"),
            ("issue_teams", "team_id", "team"),
            ("series_teams", "team_id", "team"),
            ("issue_locations", "location_id", "location"),
            ("series_locations", "location_id", "location"),
        ] {
            db.execute_unprepared(&format!(
                "ALTER TABLE {table} \
                 ADD COLUMN IF NOT EXISTS {fk_col} UUID \
                 REFERENCES {target_table}(id) ON DELETE SET NULL"
            ))
            .await?;
            db.execute_unprepared(&format!(
                "CREATE INDEX IF NOT EXISTS {table}_{fk_col}_idx \
                 ON {table} ({fk_col}) WHERE {fk_col} IS NOT NULL"
            ))
            .await?;
        }

        // Per-relationship metadata on existing junctions.
        db.execute_unprepared(
            "ALTER TABLE issue_characters \
             ADD COLUMN IF NOT EXISTS is_first_appearance BOOLEAN NOT NULL DEFAULT FALSE, \
             ADD COLUMN IF NOT EXISTS died_in_issue BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE issue_teams \
             ADD COLUMN IF NOT EXISTS is_first_appearance BOOLEAN NOT NULL DEFAULT FALSE, \
             ADD COLUMN IF NOT EXISTS disbanded_in_issue BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE issue_locations \
             ADD COLUMN IF NOT EXISTS is_first_appearance BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE issue_credits \
             ADD COLUMN IF NOT EXISTS ordinal INTEGER NOT NULL DEFAULT 0",
        )
        .await?;

        // ──────────────────────────────────────────────────────────────
        // §3  New FK-PK junctions (no legacy data; FK-only PK)
        // ──────────────────────────────────────────────────────────────
        // story_arc / concept / object / universe junctions are
        // greenfield — no string-keyed legacy to preserve. The (issue_id,
        // entity_id) PK enforces dedupe at the schema level.
        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS issue_arcs (
                issue_id          TEXT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                arc_id            UUID NOT NULL REFERENCES story_arc(id) ON DELETE CASCADE,
                position_in_arc   INTEGER,
                PRIMARY KEY (issue_id, arc_id)
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS issue_arcs_arc_idx ON issue_arcs (arc_id)",
        )
        .await?;

        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS series_arcs (
                series_id   UUID NOT NULL REFERENCES series(id) ON DELETE CASCADE,
                arc_id      UUID NOT NULL REFERENCES story_arc(id) ON DELETE CASCADE,
                PRIMARY KEY (series_id, arc_id)
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS series_arcs_arc_idx ON series_arcs (arc_id)",
        )
        .await?;

        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS issue_concepts (
                issue_id            TEXT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                concept_id          UUID NOT NULL REFERENCES concept(id) ON DELETE CASCADE,
                is_first_appearance BOOLEAN NOT NULL DEFAULT FALSE,
                PRIMARY KEY (issue_id, concept_id)
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS issue_concepts_concept_idx ON issue_concepts (concept_id)",
        )
        .await?;

        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS series_concepts (
                series_id   UUID NOT NULL REFERENCES series(id) ON DELETE CASCADE,
                concept_id  UUID NOT NULL REFERENCES concept(id) ON DELETE CASCADE,
                PRIMARY KEY (series_id, concept_id)
            )"#,
        )
        .await?;

        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS issue_objects (
                issue_id            TEXT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                object_id           UUID NOT NULL REFERENCES object(id) ON DELETE CASCADE,
                is_first_appearance BOOLEAN NOT NULL DEFAULT FALSE,
                PRIMARY KEY (issue_id, object_id)
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS issue_objects_object_idx ON issue_objects (object_id)",
        )
        .await?;

        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS series_objects (
                series_id   UUID NOT NULL REFERENCES series(id) ON DELETE CASCADE,
                object_id   UUID NOT NULL REFERENCES object(id) ON DELETE CASCADE,
                PRIMARY KEY (series_id, object_id)
            )"#,
        )
        .await?;

        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS issue_universes (
                issue_id    TEXT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                universe_id UUID NOT NULL REFERENCES universe(id) ON DELETE CASCADE,
                PRIMARY KEY (issue_id, universe_id)
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS issue_universes_universe_idx \
             ON issue_universes (universe_id)",
        )
        .await?;

        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS series_universes (
                series_id   UUID NOT NULL REFERENCES series(id) ON DELETE CASCADE,
                universe_id UUID NOT NULL REFERENCES universe(id) ON DELETE CASCADE,
                PRIMARY KEY (series_id, universe_id)
            )"#,
        )
        .await?;

        // Reprint relationships are intentionally permissive: the
        // `reprinted_issue_id` FK is nullable so we can record reprints
        // of issues we don't have in the library (just the label, e.g.
        // "Amazing Spider-Man #1"). Synthetic UUID PK because sea-orm
        // can't tolerate nullable columns in a composite PK; dedup is
        // enforced by the unique index over the COALESCEd keys.
        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS issue_reprints (
                id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                issue_id            TEXT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                reprinted_issue_id  TEXT REFERENCES issues(id) ON DELETE SET NULL,
                reprinted_label     TEXT,
                CHECK (reprinted_issue_id IS NOT NULL OR reprinted_label IS NOT NULL)
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS issue_reprints_dedup \
             ON issue_reprints (issue_id, COALESCE(reprinted_issue_id, ''), COALESCE(reprinted_label, ''))",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS issue_reprints_reprinted_idx \
             ON issue_reprints (reprinted_issue_id) WHERE reprinted_issue_id IS NOT NULL",
        )
        .await?;

        // ──────────────────────────────────────────────────────────────
        // §4  Cover storage (replaces single cover.webp model)
        // ──────────────────────────────────────────────────────────────
        // M4's apply_cover writes to `{data_path}/thumbs/issues/{id}/
        // covers/{cover_id}.<ext>`. The backfill in §9 writes the
        // legacy `thumbs/issues/{id}/cover.webp` path for primary covers
        // that already exist on disk.
        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS issue_cover (
                id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                issue_id                 TEXT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                kind                     TEXT NOT NULL,
                ordinal                  INTEGER NOT NULL DEFAULT 0,
                source_provider          TEXT,
                source_external_id       TEXT,
                source_url               TEXT,
                variant_label            TEXT,
                variant_artist_person_id UUID REFERENCES person(id) ON DELETE SET NULL,
                local_path               TEXT NOT NULL,
                width                    INTEGER,
                height                   INTEGER,
                phash                    BIGINT,
                dhash                    BIGINT,
                ahash                    BIGINT,
                fetched_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
                is_active                BOOLEAN NOT NULL DEFAULT TRUE,
                UNIQUE (issue_id, kind, ordinal)
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS issue_cover_issue_idx ON issue_cover (issue_id) WHERE is_active",
        )
        .await?;

        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS series_cover (
                id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                series_id           UUID NOT NULL REFERENCES series(id) ON DELETE CASCADE,
                kind                TEXT NOT NULL,
                ordinal             INTEGER NOT NULL DEFAULT 0,
                source_provider     TEXT,
                source_external_id  TEXT,
                source_url          TEXT,
                local_path          TEXT NOT NULL,
                width               INTEGER,
                height              INTEGER,
                fetched_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
                is_active           BOOLEAN NOT NULL DEFAULT TRUE,
                UNIQUE (series_id, kind, ordinal)
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS series_cover_series_idx ON series_cover (series_id) WHERE is_active",
        )
        .await?;

        // ──────────────────────────────────────────────────────────────
        // §5  external_ids (replaces fixed ID columns)
        // ──────────────────────────────────────────────────────────────
        // entity_type strings match the table names (snake_case):
        // 'series'|'issue'|'person'|'character'|'team'|'story_arc'|
        // 'location'|'concept'|'object'|'publisher'|'imprint'|'universe'.
        // source strings: 'comicvine'|'metron'|'gcd'|'marvel'|'locg'|
        // 'mal'|'anilist'|'mangaupdates'|'isbn'|'upc'|'asin'|'doi'.
        //
        // entity_id is TEXT, not UUID. Every entity in the application
        // has a UUID primary key *except* `issue`, whose id is the
        // content-/path-derived BLAKE3 hex (TEXT) — that property is
        // load-bearing for the scanner's deterministic "have I seen
        // this file before?" check (see scanner_content_hash_done).
        // Folding UUIDs through `::text` is free; the alternative (a
        // surrogate uuid_generate_v5 cast for every issue lookup) is
        // a wart. Matches the existing convention every issue-keyed
        // junction (issue_credits / issue_characters / …) already uses.
        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS external_ids (
                entity_type     TEXT NOT NULL,
                entity_id       TEXT NOT NULL,
                source          TEXT NOT NULL,
                external_id     TEXT NOT NULL,
                external_url    TEXT,
                set_by          TEXT NOT NULL,
                first_set_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
                last_synced_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (entity_type, entity_id, source),
                UNIQUE (source, external_id, entity_type)
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS external_ids_lookup ON external_ids (source, external_id)",
        )
        .await?;

        // ──────────────────────────────────────────────────────────────
        // §6  field_provenance (generalizes issue.user_edited)
        // ──────────────────────────────────────────────────────────────
        // field values come from the MetadataField enum that lands in
        // M0b — never free-form strings at call sites. entity_id is
        // TEXT for the same reason external_ids.entity_id is.
        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS field_provenance (
                entity_type         TEXT NOT NULL,
                entity_id           TEXT NOT NULL,
                field               TEXT NOT NULL,
                set_by              TEXT NOT NULL,
                set_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
                source_external_id  TEXT,
                PRIMARY KEY (entity_type, entity_id, field)
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS field_provenance_by_user \
             ON field_provenance (entity_type, entity_id) WHERE set_by = 'user'",
        )
        .await?;

        // ──────────────────────────────────────────────────────────────
        // §7  metadata_run (history table)
        // ──────────────────────────────────────────────────────────────
        // Per-item detail lives on `audit_log` rows linked via
        // `payload->>'run_id'`. The dedicated table exists for fast
        // filter on run-level fields (status / scope / library_id)
        // that audit_log doesn't index well.
        db.execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS metadata_run (
                id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                scope                   TEXT NOT NULL,
                scope_entity_id         TEXT,
                library_id              UUID REFERENCES libraries(id) ON DELETE SET NULL,
                triggered_by            UUID REFERENCES users(id) ON DELETE SET NULL,
                trigger_kind            TEXT NOT NULL,
                providers               TEXT[] NOT NULL,
                status                  TEXT NOT NULL,
                started_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
                finished_at             TIMESTAMPTZ,
                items_total             INTEGER NOT NULL DEFAULT 0,
                items_matched_high      INTEGER NOT NULL DEFAULT 0,
                items_matched_medium    INTEGER NOT NULL DEFAULT 0,
                items_matched_low       INTEGER NOT NULL DEFAULT 0,
                items_no_match          INTEGER NOT NULL DEFAULT 0,
                items_applied           INTEGER NOT NULL DEFAULT 0,
                items_skipped           INTEGER NOT NULL DEFAULT 0,
                items_failed            INTEGER NOT NULL DEFAULT 0,
                error_summary           TEXT,
                resume_after            TIMESTAMPTZ
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS metadata_run_recent \
             ON metadata_run (started_at DESC)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS metadata_run_open \
             ON metadata_run (status) WHERE status NOT IN ('completed','failed','cancelled')",
        )
        .await?;

        // ──────────────────────────────────────────────────────────────
        // §8  New columns on series + issues
        // ──────────────────────────────────────────────────────────────
        db.execute_unprepared(
            "ALTER TABLE series \
             ADD COLUMN IF NOT EXISTS sort_name TEXT, \
             ADD COLUMN IF NOT EXISTS year_end INTEGER, \
             ADD COLUMN IF NOT EXISTS series_type TEXT, \
             ADD COLUMN IF NOT EXISTS aliases JSONB NOT NULL DEFAULT '[]'::jsonb, \
             ADD COLUMN IF NOT EXISTS deck TEXT, \
             ADD COLUMN IF NOT EXISTS publisher_id UUID REFERENCES publisher(id) ON DELETE SET NULL, \
             ADD COLUMN IF NOT EXISTS imprint_id UUID REFERENCES imprint(id) ON DELETE SET NULL, \
             ADD COLUMN IF NOT EXISTS last_metadata_sync_at TIMESTAMPTZ, \
             ADD COLUMN IF NOT EXISTS metadata_sync_paused BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS series_publisher_id_idx ON series (publisher_id) WHERE publisher_id IS NOT NULL",
        )
        .await?;

        db.execute_unprepared(
            "ALTER TABLE issues \
             ADD COLUMN IF NOT EXISTS deck TEXT, \
             ADD COLUMN IF NOT EXISTS store_date DATE, \
             ADD COLUMN IF NOT EXISTS foc_date DATE, \
             ADD COLUMN IF NOT EXISTS price DOUBLE PRECISION, \
             ADD COLUMN IF NOT EXISTS sku TEXT, \
             ADD COLUMN IF NOT EXISTS staff_rating DOUBLE PRECISION, \
             ADD COLUMN IF NOT EXISTS aliases JSONB NOT NULL DEFAULT '[]'::jsonb, \
             ADD COLUMN IF NOT EXISTS last_metadata_sync_at TIMESTAMPTZ",
        )
        .await?;

        // Bring the pre-existing `person` table up to the shared
        // top-level-entity shape (matches character / team / etc.).
        // m20261223 created it with the minimal id/slug/name/
        // normalized_name shape; M0's writer helpers expect the
        // richer set.
        db.execute_unprepared(
            "ALTER TABLE person \
             ADD COLUMN IF NOT EXISTS aliases JSONB NOT NULL DEFAULT '[]'::jsonb, \
             ADD COLUMN IF NOT EXISTS description TEXT, \
             ADD COLUMN IF NOT EXISTS image_url TEXT",
        )
        .await?;

        // ──────────────────────────────────────────────────────────────
        // §9  Backfill
        // ──────────────────────────────────────────────────────────────

        // §9a  Backfill character / team / location entities from
        //      existing junction strings (mirror of person backfill).
        for (entity_table, junction_tables, name_col) in [
            (
                "character",
                vec!["issue_characters", "series_characters"],
                "character",
            ),
            ("team", vec!["issue_teams", "series_teams"], "team"),
            (
                "location",
                vec!["issue_locations", "series_locations"],
                "location",
            ),
        ] {
            let union_sql = junction_tables
                .iter()
                .map(|t| {
                    format!(
                        "SELECT {name_col} AS nm FROM {t} \
                         WHERE {name_col} IS NOT NULL AND {name_col} <> ''"
                    )
                })
                .collect::<Vec<_>>()
                .join(" UNION ALL ");
            db.execute_unprepared(&format!(
                r#"
                WITH names AS ({union_sql}),
                normalized AS (
                    SELECT
                        nm                              AS display_name,
                        btrim(lower(nm))                AS normalized_name,
                        regexp_replace(
                            regexp_replace(btrim(lower(nm)), '[^a-z0-9]+', '-', 'g'),
                            '(^-+|-+$)', '', 'g'
                        )                               AS base_slug
                    FROM names
                ),
                distinct_norm AS (
                    SELECT
                        min(display_name) AS display_name,
                        normalized_name,
                        min(base_slug)    AS base_slug
                    FROM normalized
                    WHERE normalized_name <> ''
                    GROUP BY normalized_name
                ),
                slug_collisions AS (
                    SELECT
                        display_name, normalized_name, base_slug,
                        row_number() OVER (PARTITION BY base_slug ORDER BY normalized_name) AS collision_idx
                    FROM distinct_norm
                )
                INSERT INTO {entity_table} (slug, name, normalized_name)
                SELECT
                    CASE
                        WHEN base_slug = '' THEN 'untitled'
                        WHEN collision_idx = 1 THEN base_slug
                        ELSE base_slug || '-' || (collision_idx::text)
                    END AS slug,
                    display_name,
                    normalized_name
                FROM slug_collisions
                ON CONFLICT (normalized_name) DO NOTHING
                "#
            ))
            .await?;
        }

        // §9b  Backfill FK columns on existing junctions.
        for (table, fk_col, entity_table, name_col) in [
            ("issue_characters", "character_id", "character", "character"),
            (
                "series_characters",
                "character_id",
                "character",
                "character",
            ),
            ("issue_teams", "team_id", "team", "team"),
            ("series_teams", "team_id", "team", "team"),
            ("issue_locations", "location_id", "location", "location"),
            ("series_locations", "location_id", "location", "location"),
        ] {
            db.execute_unprepared(&format!(
                "UPDATE {table} j \
                 SET {fk_col} = e.id \
                 FROM {entity_table} e \
                 WHERE j.{fk_col} IS NULL \
                   AND e.normalized_name = btrim(lower(j.{name_col}))"
            ))
            .await?;
        }

        // §9c  Backfill story_arc entity + issue_arcs junction from the
        //      `issue.story_arc` CSV column.
        // ComicInfo allows `;` and `,` as separators (same convention
        // the existing scanner uses for credit CSVs). Split, trim,
        // dedup, slugify.
        db.execute_unprepared(
            r#"
            WITH split AS (
                SELECT
                    id AS issue_id,
                    btrim(unnest(string_to_array(regexp_replace(story_arc, ';', ','), ','))) AS arc_name
                FROM issues
                WHERE story_arc IS NOT NULL AND story_arc <> ''
            ),
            normalized AS (
                SELECT
                    issue_id,
                    arc_name AS display_name,
                    btrim(lower(arc_name)) AS normalized_name,
                    regexp_replace(
                        regexp_replace(btrim(lower(arc_name)), '[^a-z0-9]+', '-', 'g'),
                        '(^-+|-+$)', '', 'g'
                    ) AS base_slug
                FROM split
                WHERE arc_name <> ''
            ),
            distinct_norm AS (
                SELECT
                    min(display_name) AS display_name,
                    normalized_name,
                    min(base_slug)    AS base_slug
                FROM normalized
                GROUP BY normalized_name
            ),
            slug_collisions AS (
                SELECT
                    display_name, normalized_name, base_slug,
                    row_number() OVER (PARTITION BY base_slug ORDER BY normalized_name) AS collision_idx
                FROM distinct_norm
            )
            INSERT INTO story_arc (slug, name, normalized_name)
            SELECT
                CASE
                    WHEN base_slug = '' THEN 'untitled'
                    WHEN collision_idx = 1 THEN base_slug
                    ELSE base_slug || '-' || (collision_idx::text)
                END AS slug,
                display_name,
                normalized_name
            FROM slug_collisions
            ON CONFLICT (normalized_name) DO NOTHING
            "#,
        )
        .await?;

        db.execute_unprepared(
            r#"
            INSERT INTO issue_arcs (issue_id, arc_id)
            SELECT DISTINCT i.issue_id, sa.id
            FROM (
                SELECT
                    id AS issue_id,
                    btrim(lower(unnest(string_to_array(regexp_replace(story_arc, ';', ','), ',')))) AS normalized_name
                FROM issues
                WHERE story_arc IS NOT NULL AND story_arc <> ''
            ) i
            JOIN story_arc sa ON sa.normalized_name = i.normalized_name
            WHERE i.normalized_name <> ''
            ON CONFLICT DO NOTHING
            "#,
        )
        .await?;

        // §9d  Backfill publisher + imprint entities from series strings,
        //      then set series.publisher_id / series.imprint_id FK.
        db.execute_unprepared(
            r#"
            WITH normalized AS (
                SELECT
                    publisher AS display_name,
                    btrim(lower(publisher)) AS normalized_name,
                    regexp_replace(
                        regexp_replace(btrim(lower(publisher)), '[^a-z0-9]+', '-', 'g'),
                        '(^-+|-+$)', '', 'g'
                    ) AS base_slug
                FROM series
                WHERE publisher IS NOT NULL AND publisher <> ''
            ),
            distinct_norm AS (
                SELECT min(display_name) AS display_name, normalized_name, min(base_slug) AS base_slug
                FROM normalized
                GROUP BY normalized_name
            ),
            slug_collisions AS (
                SELECT
                    display_name, normalized_name, base_slug,
                    row_number() OVER (PARTITION BY base_slug ORDER BY normalized_name) AS collision_idx
                FROM distinct_norm
            )
            INSERT INTO publisher (slug, name, normalized_name)
            SELECT
                CASE
                    WHEN base_slug = '' THEN 'untitled'
                    WHEN collision_idx = 1 THEN base_slug
                    ELSE base_slug || '-' || (collision_idx::text)
                END,
                display_name,
                normalized_name
            FROM slug_collisions
            ON CONFLICT (normalized_name) DO NOTHING
            "#,
        )
        .await?;

        db.execute_unprepared(
            "UPDATE series s \
             SET publisher_id = p.id \
             FROM publisher p \
             WHERE s.publisher_id IS NULL \
               AND p.normalized_name = btrim(lower(s.publisher))",
        )
        .await?;

        // Imprint backfill: needs a publisher_id since imprint hangs off
        // publisher in the schema. Join via series.publisher_id.
        db.execute_unprepared(
            r#"
            WITH src AS (
                SELECT DISTINCT
                    btrim(s.imprint) AS imprint_name,
                    btrim(lower(s.imprint)) AS imprint_norm,
                    s.publisher_id
                FROM series s
                WHERE s.imprint IS NOT NULL AND s.imprint <> ''
                  AND s.publisher_id IS NOT NULL
            ),
            with_slug AS (
                SELECT
                    imprint_name AS display_name,
                    imprint_norm AS normalized_name,
                    publisher_id,
                    regexp_replace(
                        regexp_replace(imprint_norm, '[^a-z0-9]+', '-', 'g'),
                        '(^-+|-+$)', '', 'g'
                    ) AS base_slug
                FROM src
            ),
            slug_collisions AS (
                SELECT
                    display_name, normalized_name, publisher_id, base_slug,
                    row_number() OVER (PARTITION BY base_slug ORDER BY normalized_name) AS collision_idx
                FROM with_slug
            )
            INSERT INTO imprint (slug, name, normalized_name, publisher_id)
            SELECT
                CASE
                    WHEN base_slug = '' THEN 'untitled'
                    WHEN collision_idx = 1 THEN base_slug
                    ELSE base_slug || '-' || (collision_idx::text)
                END,
                display_name,
                normalized_name,
                publisher_id
            FROM slug_collisions
            ON CONFLICT (normalized_name) DO NOTHING
            "#,
        )
        .await?;

        db.execute_unprepared(
            "UPDATE series s \
             SET imprint_id = i.id \
             FROM imprint i \
             WHERE s.imprint_id IS NULL \
               AND i.normalized_name = btrim(lower(s.imprint))",
        )
        .await?;

        // §9e  Backfill external_ids from the soon-to-be-dropped ID
        //      columns. set_by='migration_v1' so a down() rollback can
        //      identify them; first_set_at = NOW() is the best we have.
        // entity_id is TEXT — series UUID and issue BLAKE3 both fit
        // directly via ::text cast.
        for (parent_table, entity_type) in [("series", "series"), ("issues", "issue")] {
            for (col, source) in [
                ("comicvine_id", "comicvine"),
                ("metron_id", "metron"),
                ("gtin", "gtin"),
            ] {
                db.execute_unprepared(&format!(
                    "INSERT INTO external_ids (entity_type, entity_id, source, external_id, set_by) \
                     SELECT '{entity_type}', id::text, '{source}', {col}::text, 'migration_v1' \
                     FROM {parent_table} \
                     WHERE {col} IS NOT NULL \
                     ON CONFLICT DO NOTHING"
                ))
                .await?;
            }
        }

        // §9f  Backfill issue_cover from existing on-disk covers.
        // The thumbnail pipeline writes to
        // `{data_path}/thumbs/issues/{id}/cover.webp` (the default
        // format). We store the relative path; the cover-serving code
        // resolves against data_path. M4's new covers use a different
        // path layout — both coexist via the local_path column.
        db.execute_unprepared(
            "INSERT INTO issue_cover (issue_id, kind, ordinal, source_provider, local_path, fetched_at) \
             SELECT id, 'primary', 0, 'archive_extracted', \
                    'thumbs/issues/' || id || '/cover.webp', \
                    thumbnails_generated_at \
             FROM issues \
             WHERE thumbnails_generated_at IS NOT NULL \
             ON CONFLICT DO NOTHING",
        )
        .await?;

        // §9g  Backfill field_provenance from issue.user_edited JSON.
        // user_edited is an array of column-name strings; each becomes
        // one row with set_by='user'.
        db.execute_unprepared(
            "INSERT INTO field_provenance (entity_type, entity_id, field, set_by, set_at) \
             SELECT 'issue', id, \
                    jsonb_array_elements_text(user_edited), \
                    'user', updated_at \
             FROM issues \
             WHERE jsonb_typeof(user_edited) = 'array' \
               AND jsonb_array_length(user_edited) > 0 \
             ON CONFLICT DO NOTHING",
        )
        .await?;

        // ──────────────────────────────────────────────────────────────
        // §10  Drop the old ID columns
        // ──────────────────────────────────────────────────────────────
        // Now that external_ids carries everything, the fixed columns
        // are pure dead weight. Drop them so future readers can't
        // accidentally rely on stale state.
        db.execute_unprepared(
            "ALTER TABLE series \
             DROP COLUMN IF EXISTS comicvine_id, \
             DROP COLUMN IF EXISTS metron_id, \
             DROP COLUMN IF EXISTS gtin",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE issues \
             DROP COLUMN IF EXISTS comicvine_id, \
             DROP COLUMN IF EXISTS metron_id, \
             DROP COLUMN IF EXISTS gtin",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Reverse roughly in inverse order. Re-add dropped columns
        // first so the rest of the down doesn't lose data, then strip
        // the new schema. The external_ids → column reverse copy is
        // best-effort: 'migration_v1' rows always restore; anything
        // set by M4+ Apply jobs won't fit the fixed-column model and
        // is dropped silently. This down() is intended for dev
        // rollback, not production data preservation.

        db.execute_unprepared(
            "ALTER TABLE series \
             ADD COLUMN IF NOT EXISTS comicvine_id BIGINT, \
             ADD COLUMN IF NOT EXISTS metron_id BIGINT, \
             ADD COLUMN IF NOT EXISTS gtin TEXT",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE issues \
             ADD COLUMN IF NOT EXISTS comicvine_id BIGINT, \
             ADD COLUMN IF NOT EXISTS metron_id BIGINT, \
             ADD COLUMN IF NOT EXISTS gtin TEXT",
        )
        .await?;

        // Reverse-backfill where possible. comicvine_id / metron_id
        // were i64; cast back via ::bigint (rows with non-numeric
        // values are skipped by the WHERE clause). entity_id matches
        // either side natively now that it's TEXT.
        for (parent_table, entity_type) in [("series", "series"), ("issues", "issue")] {
            for (col, source, is_int) in [
                ("comicvine_id", "comicvine", true),
                ("metron_id", "metron", true),
                ("gtin", "gtin", false),
            ] {
                let val_expr = if is_int {
                    "e.external_id::bigint"
                } else {
                    "e.external_id"
                };
                let cast_clause = if is_int {
                    " AND e.external_id ~ '^-?[0-9]+$'"
                } else {
                    ""
                };
                db.execute_unprepared(&format!(
                    "UPDATE {parent_table} t \
                     SET {col} = {val_expr} \
                     FROM external_ids e \
                     WHERE e.entity_type = '{entity_type}' \
                       AND e.source = '{source}' \
                       AND e.entity_id = t.id::text{cast_clause}"
                ))
                .await?;
            }
        }

        // Drop new columns on series / issues.
        db.execute_unprepared(
            "ALTER TABLE issues \
             DROP COLUMN IF EXISTS deck, \
             DROP COLUMN IF EXISTS store_date, \
             DROP COLUMN IF EXISTS foc_date, \
             DROP COLUMN IF EXISTS price, \
             DROP COLUMN IF EXISTS sku, \
             DROP COLUMN IF EXISTS staff_rating, \
             DROP COLUMN IF EXISTS aliases, \
             DROP COLUMN IF EXISTS last_metadata_sync_at",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE series \
             DROP COLUMN IF EXISTS sort_name, \
             DROP COLUMN IF EXISTS year_end, \
             DROP COLUMN IF EXISTS series_type, \
             DROP COLUMN IF EXISTS aliases, \
             DROP COLUMN IF EXISTS deck, \
             DROP COLUMN IF EXISTS publisher_id, \
             DROP COLUMN IF EXISTS imprint_id, \
             DROP COLUMN IF EXISTS last_metadata_sync_at, \
             DROP COLUMN IF EXISTS metadata_sync_paused",
        )
        .await?;

        // Drop new-junction tables (cascades from entity-table drops
        // would handle these too, but explicit is safer).
        for table in [
            "issue_universes",
            "series_universes",
            "issue_objects",
            "series_objects",
            "issue_concepts",
            "series_concepts",
            "issue_arcs",
            "series_arcs",
            "issue_reprints",
            "issue_cover",
            "series_cover",
            "field_provenance",
            "metadata_run",
            "external_ids",
        ] {
            db.execute_unprepared(&format!("DROP TABLE IF EXISTS {table} CASCADE"))
                .await?;
        }

        // Drop the FK columns + per-rel metadata on existing junctions.
        for (table, fk_col) in [
            ("issue_characters", "character_id"),
            ("series_characters", "character_id"),
            ("issue_teams", "team_id"),
            ("series_teams", "team_id"),
            ("issue_locations", "location_id"),
            ("series_locations", "location_id"),
        ] {
            db.execute_unprepared(&format!("DROP INDEX IF EXISTS {table}_{fk_col}_idx"))
                .await?;
            db.execute_unprepared(&format!(
                "ALTER TABLE {table} DROP COLUMN IF EXISTS {fk_col}"
            ))
            .await?;
        }
        db.execute_unprepared(
            "ALTER TABLE issue_characters \
             DROP COLUMN IF EXISTS is_first_appearance, \
             DROP COLUMN IF EXISTS died_in_issue",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE issue_teams \
             DROP COLUMN IF EXISTS is_first_appearance, \
             DROP COLUMN IF EXISTS disbanded_in_issue",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE issue_locations DROP COLUMN IF EXISTS is_first_appearance",
        )
        .await?;
        db.execute_unprepared("ALTER TABLE issue_credits DROP COLUMN IF EXISTS ordinal")
            .await?;

        // Drop top-level entity tables (CASCADE to clean up FKs from
        // junction tables we already dropped).
        for (table, _) in ENTITY_TABLE_DEFS {
            db.execute_unprepared(&format!("DROP TABLE IF EXISTS {table} CASCADE"))
                .await?;
        }

        Ok(())
    }
}

/// Top-level entity tables. Each gets the shared `(id, slug, name,
/// normalized_name, aliases, description, image_url, created_at,
/// updated_at)` columns plus the entity-specific extras below.
const ENTITY_TABLE_DEFS: &[(&str, &str)] = &[
    (
        "character",
        "real_name TEXT, first_appearance_issue_id TEXT,",
    ),
    ("team", ""),
    ("location", ""),
    ("story_arc", "publisher_id UUID,"),
    ("concept", ""),
    ("object", ""),
    ("publisher", "founded_year INTEGER,"),
    // imprint and universe have a publisher_id FK, but creating the FK
    // here would force CREATE-order between siblings — instead we keep
    // the column nullable + uuid here and rely on the application-layer
    // writers to set it. The FK constraint is added inline in the
    // create-table SQL via a workaround: since this slice runs *after*
    // `publisher` is created, we declare imprint/universe FKs via the
    // entity_table_defs extra_cols string itself.
    (
        "imprint",
        "publisher_id UUID REFERENCES publisher(id) ON DELETE CASCADE,",
    ),
    (
        "universe",
        "publisher_id UUID REFERENCES publisher(id) ON DELETE SET NULL,",
    ),
];
