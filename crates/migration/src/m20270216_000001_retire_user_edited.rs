//! Retire `issues.user_edited` (roadmap WP-3.7, audit AR-6 / DI-8).
//!
//! `user_edited` was the pre-`field_provenance` pin store: a JSON array
//! of issue column names the user had edited, which the scanner skipped
//! on rescan. `field_provenance` (`set_by='user'`) is now the only pin
//! store — the issue PATCH writes one row per column key plus one per
//! `MetadataField` key the column rolls up into
//! (`writers::ISSUE_COLUMN_PIN_KEYS` / `issue_column_pin_field`).
//!
//! `up`:
//!   1. Backfill every string entry of `user_edited` as a `set_by='user'`
//!      row under its column key, and again under its rolled-up
//!      `MetadataField` key (`genre` → `genres`, `writer` → `credits`,
//!      `year` → `cover_date`, …). The M0 migration already copied the
//!      column keys once; this catches every edit recorded since.
//!      A file-tier row (`comicinfo`, …) at the same key is upgraded —
//!      the list outranked the file — but a provider row is kept: an
//!      `override_user_edits` apply deliberately replaced the pin.
//!   2. Drop the column.
//!
//! `down` re-adds the column (`jsonb NOT NULL DEFAULT '[]'`) and
//! repopulates it, best-effort, from the column-key user pins.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

/// The issue column keys the old list could hold (frozen mirror of
/// `server::metadata::writers::ISSUE_COLUMN_PIN_KEYS` at the time of
/// this migration).
const COLUMN_KEYS: &[&str] = &[
    "title",
    "number_raw",
    "sort_number",
    "volume",
    "year",
    "month",
    "day",
    "summary",
    "notes",
    "publisher",
    "imprint",
    "writer",
    "penciller",
    "inker",
    "colorist",
    "letterer",
    "cover_artist",
    "editor",
    "translator",
    "characters",
    "teams",
    "locations",
    "alternate_series",
    "story_arc",
    "story_arc_number",
    "genre",
    "tags",
    "language_code",
    "age_rating",
    "format",
    "manga",
    "black_and_white",
    "web_url",
    "gtin",
    "comicvine_id",
    "metron_id",
];

/// Column key → `MetadataField::key()` for keys whose rolled-up field key
/// differs from the column key (frozen mirror of
/// `writers::issue_column_pin_field`).
const ROLLUP: &[(&str, &str)] = &[
    ("number_raw", "number"),
    ("writer", "credits"),
    ("penciller", "credits"),
    ("inker", "credits"),
    ("colorist", "credits"),
    ("letterer", "credits"),
    ("cover_artist", "credits"),
    ("editor", "credits"),
    ("translator", "credits"),
    ("story_arc", "story_arcs"),
    ("story_arc_number", "story_arcs"),
    ("genre", "genres"),
    ("year", "cover_date"),
    ("month", "cover_date"),
    ("day", "cover_date"),
    ("gtin", "external_id.gtin"),
    ("comicvine_id", "external_id.comicvine"),
    ("metron_id", "external_id.metron"),
];

/// Every string entry of the list, one row per (issue, key).
const ENTRIES: &str = "SELECT i.id AS entity_id, e.value #>> '{}' AS field, \
            i.updated_at AS set_at \
     FROM issues i, jsonb_array_elements(i.user_edited) e \
     WHERE jsonb_typeof(i.user_edited) = 'array' \
       AND jsonb_typeof(e.value) = 'string'";

/// Upsert guard: a user pin upgrades a file-tier row, never a provider one.
const UPSERT_TAIL: &str = "ON CONFLICT (entity_type, entity_id, field) DO UPDATE \
     SET set_by = 'user', set_at = EXCLUDED.set_at, source_external_id = NULL \
     WHERE field_provenance.set_by IN \
       ('comicinfo', 'metroninfo', 'series_json', 'scanner_inference', 'scanner_folder_tag')";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_column("issues", "user_edited").await? {
            return Ok(());
        }
        let db = manager.get_connection();

        // 1a. Column keys, verbatim.
        db.execute_unprepared(&format!(
            "INSERT INTO field_provenance (entity_type, entity_id, field, set_by, set_at) \
             SELECT DISTINCT ON (entity_id, field) 'issue', entity_id, field, 'user', set_at \
             FROM ({ENTRIES}) src \
             {UPSERT_TAIL}"
        ))
        .await?;

        // 1b. Rolled-up MetadataField keys.
        let case = ROLLUP
            .iter()
            .map(|(col, mf)| format!("WHEN '{col}' THEN '{mf}'"))
            .collect::<Vec<_>>()
            .join(" ");
        db.execute_unprepared(&format!(
            "INSERT INTO field_provenance (entity_type, entity_id, field, set_by, set_at) \
             SELECT DISTINCT ON (entity_id, mf) 'issue', entity_id, mf, 'user', set_at \
             FROM (SELECT entity_id, CASE field {case} END AS mf, set_at \
                   FROM ({ENTRIES}) e) src \
             WHERE mf IS NOT NULL \
             {UPSERT_TAIL}"
        ))
        .await?;

        // 2. Drop the column.
        db.execute_unprepared("ALTER TABLE issues DROP COLUMN IF EXISTS user_edited")
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE issues ADD COLUMN IF NOT EXISTS user_edited jsonb NOT NULL \
             DEFAULT '[]'::jsonb",
        )
        .await?;
        let keys = COLUMN_KEYS
            .iter()
            .map(|k| format!("'{k}'"))
            .collect::<Vec<_>>()
            .join(", ");
        db.execute_unprepared(&format!(
            "UPDATE issues SET user_edited = pins.arr \
             FROM (SELECT entity_id, jsonb_agg(field ORDER BY field) AS arr \
                   FROM field_provenance \
                   WHERE entity_type = 'issue' AND set_by = 'user' AND field IN ({keys}) \
                   GROUP BY entity_id) pins \
             WHERE issues.id = pins.entity_id"
        ))
        .await?;
        Ok(())
    }
}
