//! Mylar3's native tagger files its ComicVine issue id in `<Genre>` as
//! `CVDB<id>` next to the real genres. The scanner ingested those as
//! genres: a 1987 The Flash showed `Superhero, CVDB123919, CVDB131553, …`
//! as its genre chips, and 271 issues across 14 series carried one.
//!
//! `parsers::comicinfo` now strips the marker at parse time and uses it
//! as the issue's ComicVine id when nothing better is present. This is the
//! one-off repair for rows scanned before that rule — rescans don't
//! revisit unchanged files, so without it the chips would wait for a
//! forced scan of each series.
//!
//! Order matters: the id is harvested into `external_ids` (file tier,
//! `set_by = 'comicinfo'`, never over an existing ComicVine id) *before*
//! the junction rows are deleted; then the issue genre CSV cache is
//! rebuilt from the junction with the same expression
//! `writers::csv_cache_set_clause` uses, so the column stays strictly
//! derived.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        // 1. Keep the identifier: issue-level ComicVine id from the marker
        //    for issues that have none yet.
        conn.execute_unprepared(
            r"INSERT INTO external_ids
                     (entity_type, entity_id, source, external_id, external_url,
                      set_by, first_set_at, last_synced_at)
              SELECT DISTINCT ON (ig.issue_id)
                     'issue', ig.issue_id, 'comicvine',
                     substring(ig.genre from 5),
                     'https://comicvine.gamespot.com/issue/4000-' || substring(ig.genre from 5) || '/',
                     'comicinfo', now(), now()
                FROM issue_genres ig
               WHERE ig.genre ~* '^cvdb[0-9]+$'
                 AND NOT EXISTS (
                     SELECT 1 FROM external_ids e
                      WHERE e.entity_type = 'issue'
                        AND e.entity_id = ig.issue_id
                        AND e.source = 'comicvine')
               ORDER BY ig.issue_id, ig.ordinal
              ON CONFLICT DO NOTHING",
        )
        .await?;

        // 2. Drop the marker from both junctions.
        conn.execute_unprepared(r"DELETE FROM issue_genres WHERE genre ~* '^cvdb[0-9]+$'")
            .await?;
        conn.execute_unprepared(r"DELETE FROM series_genres WHERE genre ~* '^cvdb[0-9]+$'")
            .await?;

        // 3. Rebuild the derived `issues.genre` column for the rows that
        //    carried a marker (same aggregation as the CSV read-cache).
        conn.execute_unprepared(
            r"UPDATE issues
                 SET genre = (
                     SELECT NULLIF(
                                CASE WHEN bool_or(genre LIKE '%,%')
                                     THEN string_agg(genre, '; ' ORDER BY genre)
                                     ELSE string_agg(genre, ', ' ORDER BY genre)
                                END, '')
                       FROM issue_genres
                      WHERE issue_id = issues.id)
               WHERE genre ~* '(^|[,;][[:space:]]*)cvdb[0-9]+([[:space:]]*[,;]|$)'",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Data repair; a forced scan re-derives genres from the files
        // (which the parser now cleans) either way.
        Ok(())
    }
}
