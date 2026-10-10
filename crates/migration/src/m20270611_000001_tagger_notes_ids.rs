//! Provider ids the files already carried, recovered without a request.
//!
//! 1. **ComicTagger's notes trace.** `Tagged with ComicTagger … using info
//!    from Metron on … [Issue ID 28577]` names the provider and the issue
//!    id; the ComicVine and Grand Comics Database variants do the same.
//!    `parsers::comicinfo` now reads them when neither an explicit tag nor
//!    the `<Web>` URL supplied an id (Metron's `<Web>` is a slug, so the
//!    notes were the only numeric id in the file). This is the one-off
//!    repair for rows scanned before that rule — a rescan skips unchanged
//!    files, so without it ~1,900 Metron ids would wait for a forced scan.
//! 2. **ComicRack-era `[CVDB n]` notes** (`Scraped metadata from ComicVine
//!    [CVDB121185].`): the same marker Mylar writes into `<Genre>`, in the
//!    notes instead. Issue-level ComicVine id when none exists.
//! 3. **Series ids from cached issue records.** A provider's issue detail
//!    (`metadata_cache`, entity `issue`) names its series; a series whose
//!    issues were fetched but which never got a series-level id of that
//!    source takes the most common one. The apply path now does this as
//!    it goes; this covers the rows applied before.
//!
//! Every insert is file-tier (`set_by = 'comicinfo'`) or provider-tier
//! (`set_by = '<source>'`), never over an existing row of that source,
//! and `ON CONFLICT DO NOTHING` on the `(source, external_id, entity_type)`
//! key, so an id another issue already owns is left alone.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

/// `[Issue ID n]` from a ComicTagger note that names `source_phrase`.
fn issue_id_from_notes(
    source: &str,
    source_phrase: &str,
    url_prefix: &str,
    url_suffix: &str,
) -> String {
    format!(
        r"INSERT INTO external_ids
                 (entity_type, entity_id, source, external_id, external_url,
                  set_by, first_set_at, last_synced_at)
          SELECT 'issue', i.id, '{source}', m[1],
                 '{url_prefix}' || m[1] || '{url_suffix}',
                 'comicinfo', now(), now()
            FROM issues i,
                 regexp_match(COALESCE(i.comic_info_raw->>'notes', ''), '\[Issue ID ([0-9]+)\]') AS m
           WHERE i.removed_at IS NULL
             AND m[1] IS NOT NULL
             AND COALESCE(i.comic_info_raw->>'notes', '') ~* 'ComicTagger.*using info from {source_phrase}'
             AND NOT EXISTS (
                 SELECT 1 FROM external_ids e
                  WHERE e.entity_type = 'issue'
                    AND e.entity_id = i.id
                    AND e.source = '{source}')
          ON CONFLICT DO NOTHING"
    )
}

/// The most common series id among a series' cached issue records of
/// `source`, for series that have none of that source yet.
fn series_id_from_cache(source: &str, url_prefix: &str, url_suffix: &str) -> String {
    format!(
        r"INSERT INTO external_ids
                 (entity_type, entity_id, source, external_id, external_url,
                  set_by, first_set_at, last_synced_at)
          SELECT 'series', s.series_id::text, '{source}', s.sid,
                 '{url_prefix}' || s.sid || '{url_suffix}',
                 '{source}', now(), now()
            FROM (SELECT DISTINCT ON (series_id) series_id, sid
                    FROM (SELECT i.series_id,
                                 c.payload->>'series_external_id' AS sid,
                                 count(*) AS n
                            FROM issues i
                            JOIN external_ids e
                              ON e.entity_type = 'issue' AND e.entity_id = i.id
                             AND e.source = '{source}'
                            JOIN metadata_cache c
                              ON c.provider = '{source}' AND c.entity = 'issue'
                             AND c.external_id = e.external_id
                           WHERE i.removed_at IS NULL
                             AND COALESCE(c.payload->>'series_external_id', '') <> ''
                           GROUP BY 1, 2) t
                   ORDER BY series_id, n DESC, sid ASC) s
           WHERE NOT EXISTS (
                 SELECT 1 FROM external_ids x
                  WHERE x.entity_type = 'series'
                    AND x.entity_id = s.series_id::text
                    AND x.source = '{source}')
          ON CONFLICT DO NOTHING"
    )
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        // 1. ComicTagger notes → issue ids.
        conn.execute_unprepared(&issue_id_from_notes(
            "metron",
            "Metron",
            "https://metron.cloud/issue/",
            "/",
        ))
        .await?;
        conn.execute_unprepared(&issue_id_from_notes(
            "comicvine",
            "Comic ?Vine",
            "https://comicvine.gamespot.com/issue/4000-",
            "/",
        ))
        .await?;
        conn.execute_unprepared(&issue_id_from_notes(
            "gcd",
            "Grand Comics",
            "https://www.comics.org/issue/",
            "/",
        ))
        .await?;

        // 2. `[CVDB n]` in the notes → ComicVine issue id.
        conn.execute_unprepared(
            r"INSERT INTO external_ids
                     (entity_type, entity_id, source, external_id, external_url,
                      set_by, first_set_at, last_synced_at)
              SELECT 'issue', i.id, 'comicvine', m[1],
                     'https://comicvine.gamespot.com/issue/4000-' || m[1] || '/',
                     'comicinfo', now(), now()
                FROM issues i,
                     regexp_match(COALESCE(i.comic_info_raw->>'notes', ''), '\[CVDB([0-9]+)\]', 'i') AS m
               WHERE i.removed_at IS NULL
                 AND m[1] IS NOT NULL
                 AND NOT EXISTS (
                     SELECT 1 FROM external_ids e
                      WHERE e.entity_type = 'issue'
                        AND e.entity_id = i.id
                        AND e.source = 'comicvine')
              ON CONFLICT DO NOTHING",
        )
        .await?;

        // 3. Series ids from cached issue records.
        conn.execute_unprepared(&series_id_from_cache(
            "metron",
            "https://metron.cloud/series/",
            "/",
        ))
        .await?;
        conn.execute_unprepared(&series_id_from_cache(
            "gcd",
            "https://www.comics.org/series/",
            "/",
        ))
        .await?;
        conn.execute_unprepared(&series_id_from_cache(
            "comicvine",
            "https://comicvine.gamespot.com/volume/4050-",
            "/",
        ))
        .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Data repair: the rows are what a rescan (parser) and the apply
        // path now write anyway.
        Ok(())
    }
}
