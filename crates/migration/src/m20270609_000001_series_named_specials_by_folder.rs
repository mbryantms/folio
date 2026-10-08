//! Series that *are* their specials: clear `issues.special_type` where the
//! series' own *folder* name carries the marker.
//!
//! Supersedes `m20270608_000001_series_named_specials` (shipped in
//! v0.48.1), which keyed the same repair on `series.name`. That one is
//! now a no-op; this id re-runs the repair on every instance, including
//! those that already applied the old one.
//!
//! `The Amazing Spider-Man Annual (1965)` holds 33 files that all say
//! "Annual", so the scanner tagged every one `special_type = 'Annual'`:
//! the series showed as 0 of 33 with "+33 specials", the coverage analysis
//! excluded all of them, and the Issues tab filed the whole run under
//! Specials. `detect_special_type` now drops a tag the series folder name
//! itself carries (annual / special / one-shot — never TPB); this is the
//! one-off repair for rows scanned before that rule. Rescans don't
//! revisit unchanged files, so without this the fix would wait for a
//! forced scan of each series.
//!
//! Keyed on the same identity the scanner compares against: the series
//! *folder* name (`series.folder_path` basename). `series.name` comes
//! from the first-scanned file's `<Series>`, which an annual under
//! `Batman (2016)/Annuals/` legitimately sets to "Batman Annual" — keying
//! on it would strip real annuals the scanner keeps tagged. Rows from
//! before the folder-path fast path have no `folder_path`; only those
//! fall back to `name`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        // Whole-word match mirroring `series_name_carries_marker`: tokens
        // are split on any non-alphanumeric (space, hyphen, underscore,
        // colon …), so "Semiannual" / "Specialists" don't match but
        // "Annuals", "Batman_Annual", "One Shot", "One-Shots" and
        // "Oneshot" do. Postgres `\m` / `\M` can't be used here because
        // they treat `_` as a word character. `~*` is case-insensitive.
        conn.execute_unprepared(
            r"UPDATE issues i SET special_type = NULL
                FROM (
                    SELECT id,
                           COALESCE(
                               NULLIF(regexp_replace(rtrim(folder_path, '/'), '^.*/', ''), ''),
                               name
                           ) AS ident
                      FROM series
                ) s
               WHERE s.id = i.series_id
                 AND i.special_type IS NOT NULL
                 AND (
                      (i.special_type = 'Annual'  AND s.ident ~* '(^|[^a-z0-9])annuals?($|[^a-z0-9])')
                   OR (i.special_type = 'Special' AND s.ident ~* '(^|[^a-z0-9])specials?($|[^a-z0-9])')
                   OR (i.special_type = 'OneShot' AND s.ident ~* '(^|[^a-z0-9])one[^a-z0-9]*shots?($|[^a-z0-9])')
                 )",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Data repair; the next forced scan re-derives `special_type`
        // from the files either way.
        Ok(())
    }
}
