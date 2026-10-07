//! Series that *are* their specials: clear `issues.special_type` where the
//! series' own name carries the marker.
//!
//! `The Amazing Spider-Man Annual (1965)` holds 33 files that all say
//! "Annual", so the scanner tagged every one `special_type = 'Annual'`:
//! the series showed as 0 of 33 with "+33 specials", the coverage analysis
//! excluded all of them, and the Issues tab filed the whole run under
//! Specials. `detect_special_type` now drops a tag the series folder name
//! itself carries (annual / special / one-shot — never TPB); this is the
//! one-off repair for rows scanned before that rule, keyed on
//! `series.name` since the folder isn't in the database. Rescans don't
//! revisit unchanged files, so without this the fix would wait for a
//! forced scan of each series.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        // `\m` / `\M` are Postgres word boundaries (so "Semiannual" and
        // "Specialists" don't match); `~*` is case-insensitive.
        conn.execute_unprepared(
            r"UPDATE issues i SET special_type = NULL
                FROM series s
               WHERE s.id = i.series_id
                 AND i.special_type IS NOT NULL
                 AND (
                      (i.special_type = 'Annual'  AND s.name ~* '\mannuals?\M')
                   OR (i.special_type = 'Special' AND s.name ~* '\mspecials?\M')
                   OR (i.special_type = 'OneShot' AND s.name ~* '(\moneshots?\M|\mone[ -]shots?\M)')
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
