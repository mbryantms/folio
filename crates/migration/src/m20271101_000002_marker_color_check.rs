//! `markers.color` gets the same bound in the schema as at the API
//! (audit AR-8).
//!
//! The marker handlers validate `color` with garde (`color_problem` in
//! `crates/server/src/api/markers.rs`: a palette name or `#RRGGBB` /
//! `#RRGGBBAA`, stored trimmed, blank stored as NULL), but the column was
//! unbounded `text`, so any other writer (a restore, a hand-run SQL fix, a
//! future handler that forgets the rule) could store anything.
//!
//! `up` first normalises what is already there, then adds the CHECK:
//!
//! 1. trims surrounding whitespace (tabs and newlines too, as the
//!    handlers' `str::trim` does); blank becomes NULL ("no colour", how the
//!    handlers store an empty value);
//! 2. lower-cases a palette name written in another case (`Yellow`);
//! 3. sets every value that still breaks the rule to NULL. `color` is
//!    nullable and NULL means "no colour" (the client falls back to its
//!    default swatch), so this drops only the unusable value, never the
//!    marker.
//!
//! How many rows each step rewrote is reported with `RAISE NOTICE`, which
//! sqlx surfaces in the server / migration CLI log.
//!
//! The pattern is `MARKER_COLOR_PATTERN` from the markers API; the
//! `char_length` bound restates the same rule as a plain length cap (the
//! longest legal value, `#RRGGBBAA`, is 9 characters). `down` drops the
//! constraint; the normalisation is not reverted (the old values were
//! unusable).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            r"DO $$
            DECLARE
                trimmed INT;
                cased   INT;
                nulled  INT;
            BEGIN
                UPDATE markers
                   SET color = NULLIF(regexp_replace(color, '^[[:space:]]+|[[:space:]]+$', '', 'g'), '')
                 WHERE color IS DISTINCT FROM
                       NULLIF(regexp_replace(color, '^[[:space:]]+|[[:space:]]+$', '', 'g'), '');
                GET DIAGNOSTICS trimmed = ROW_COUNT;

                UPDATE markers
                   SET color = lower(color)
                 WHERE color <> lower(color)
                   AND lower(color) IN ('yellow', 'green', 'blue', 'red', 'violet');
                GET DIAGNOSTICS cased = ROW_COUNT;

                UPDATE markers
                   SET color = NULL
                 WHERE color IS NOT NULL
                   AND (char_length(color) > 32
                        OR color !~ '^(yellow|green|blue|red|violet|#[0-9A-Fa-f]{6}|#[0-9A-Fa-f]{8})$');
                GET DIAGNOSTICS nulled = ROW_COUNT;

                RAISE NOTICE 'markers.color normalised: % trimmed, % lower-cased, % invalid set to NULL',
                    trimmed, cased, nulled;
            END
            $$",
        )
        .await?;
        db.execute_unprepared(
            r"ALTER TABLE markers
                ADD CONSTRAINT markers_color_chk CHECK (
                    color IS NULL OR (
                        char_length(color) <= 32
                        AND color ~ '^(yellow|green|blue|red|violet|#[0-9A-Fa-f]{6}|#[0-9A-Fa-f]{8})$'
                    )
                )",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE markers DROP CONSTRAINT IF EXISTS markers_color_chk")
            .await?;
        Ok(())
    }
}
