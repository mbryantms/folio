//! Search foundation (§6.3): generated `tsvector` columns + GIN indexes for
//! per-context search on series and issues.
//!
//! Phase 1b implements the per-context `/series?q=` and `/series/{id}/issues?q=`
//! endpoints; the unified `/search` lands in Phase 6 (UNION ALL across entities).
//!
//! Weighting per spec §6.3:
//!   Series weight A: name, alternate names
//!         weight B: publisher, year (as text), characters, teams, locations
//!         weight C: tags, genres, story arcs
//!         weight D: summary
//!   Issue  weight A: title, series name (joined into the issue's doc)
//!         weight B: number, characters, teams, locations
//!         weight C: tags, story arcs, creators
//!         weight D: summary
//!
//! The tsvector config is `simple` (no stemming) for now — Phase 6 picks per-row
//! based on `language_code`. Simple is safe for mixed-language libraries and
//! deterministic across rescans.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();

        // ───── series.search_doc ─────
        // `unaccent()` is STABLE (depends on a dictionary), and `jsonb_path_query_array`
        // is STABLE — Postgres rejects either inside a STORED generated expression. We
        // use `to_tsvector('simple', ...)` directly (IMMUTABLE) and skip accent folding
        // for now; query-side `unaccent` (in the API) still works because the wrapper
        // there is applied to the search input, not the indexed column.
        // Alternate names: indexed by the trigger we'd add for collection/list membership;
        // for v1 they're empty so we omit them from the doc and re-add when needed.
        let series_doc = "ALTER TABLE series \
            ADD COLUMN search_doc tsvector \
            GENERATED ALWAYS AS ( \
                setweight(to_tsvector('simple', coalesce(name, '')), 'A') || \
                setweight(to_tsvector('simple', coalesce(publisher, '')), 'B') || \
                setweight(to_tsvector('simple', coalesce(year::text, '')), 'B') || \
                setweight(to_tsvector('simple', coalesce(summary, '')), 'D') \
            ) STORED";
        conn.execute(sea_orm::Statement::from_string(
            backend,
            series_doc.to_string(),
        ))
        .await?;
        conn.execute(sea_orm::Statement::from_string(
            backend,
            "CREATE INDEX series_search_doc_gin ON series USING GIN (search_doc)".to_string(),
        ))
        .await?;

        // ───── issues.search_doc ─────
        // We can't include the series name in a STORED generated column without a
        // join (Postgres restricts generated columns to the same row). Caller-side
        // queries that want series-name boosting should JOIN series and search both
        // search_docs together. For Phase 1b this is enough.
        let issue_doc = "ALTER TABLE issues \
            ADD COLUMN search_doc tsvector \
            GENERATED ALWAYS AS ( \
                setweight(to_tsvector('simple', coalesce(title, '')), 'A') || \
                setweight(to_tsvector('simple', coalesce(number_raw, '')), 'B') || \
                setweight(to_tsvector('simple', coalesce(characters, '')), 'B') || \
                setweight(to_tsvector('simple', coalesce(teams, '')), 'B') || \
                setweight(to_tsvector('simple', coalesce(locations, '')), 'B') || \
                setweight(to_tsvector('simple', coalesce(tags, '')), 'C') || \
                setweight(to_tsvector('simple', coalesce(story_arc, '')), 'C') || \
                setweight(to_tsvector('simple', coalesce(writer, '')), 'C') || \
                setweight(to_tsvector('simple', coalesce(penciller, '')), 'C') || \
                setweight(to_tsvector('simple', coalesce(summary, '')), 'D') \
            ) STORED";
        conn.execute(sea_orm::Statement::from_string(
            backend,
            issue_doc.to_string(),
        ))
        .await?;
        conn.execute(sea_orm::Statement::from_string(
            backend,
            "CREATE INDEX issues_search_doc_gin ON issues USING GIN (search_doc)".to_string(),
        ))
        .await?;

        // pg_trgm helper for autocomplete (used in Phase 6, but the index is cheap).
        conn.execute(sea_orm::Statement::from_string(
            backend,
            "CREATE INDEX series_normalized_name_trgm \
             ON series USING GIN (normalized_name gin_trgm_ops)"
                .to_string(),
        ))
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();
        for sql in [
            "DROP INDEX IF EXISTS series_normalized_name_trgm",
            "DROP INDEX IF EXISTS issues_search_doc_gin",
            "DROP INDEX IF EXISTS series_search_doc_gin",
            "ALTER TABLE issues DROP COLUMN IF EXISTS search_doc",
            "ALTER TABLE series DROP COLUMN IF EXISTS search_doc",
        ] {
            conn.execute(sea_orm::Statement::from_string(backend, sql.to_string()))
                .await?;
        }
        Ok(())
    }
}
