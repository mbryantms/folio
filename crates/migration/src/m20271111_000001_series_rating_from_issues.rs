//! A series' age rating is derived from its issues, and a library grant
//! can hide unrated content.
//!
//! 1. `library_user_access.hide_unrated BOOLEAN NOT NULL DEFAULT FALSE`:
//!    when on, a series with no rating on the ladder is hidden from that
//!    member as if it ranked above the grant's cap (and its issues with
//!    it). Existing grants keep today's behaviour (unrated is shown).
//! 2. `series.declared_age_rating TEXT NULL`: the rating the series
//!    folder's `series.json` declares. The scanner fills it on every scan
//!    that reads the folder; this migration cannot (the sidecar is on
//!    disk), so it starts NULL.
//! 3. `series.age_rating` used to be copied once from whichever file the
//!    scanner read first. At runtime it is now the highest-ranked rating
//!    among the series' active, not-removed issues and its declared
//!    rating (`server::library::series_rating`), unless a person or a
//!    metadata provider set it (a `field_provenance` row for the field
//!    `age_rating` whose `set_by` is not a file-tier code).
//!
//!    **This migration never lowers a rating.** For every series not so
//!    set it stores the highest-ranked of (the stored rating, every active
//!    issue's rating), in canonical ladder spelling, and only when that
//!    maximum exists: a stored rating is raised to its highest issue, a
//!    NULL or off-ladder rating is filled from its issues, and a series
//!    where nothing ranks is left as it is.
//!
//!    A stored rating that ranks above every active issue can only have
//!    come from `series.json` or from a file that was since removed or
//!    re-rated; the migration cannot tell which. It records that rating
//!    as a provisional `declared_age_rating`, so a recompute before the
//!    folder is next scanned (an issue edit, a removal) keeps it too. The
//!    next library scan that reads the folder, unchanged folders
//!    included, replaces the provisional value with what `series.json`
//!    actually declares, or clears it, and re-derives.
//!
//! [`AGE_RATING_LADDER`] and [`AGE_RATING_ALIASES`] are frozen copies of
//! `server::library::age_rating::{LADDER, ALIASES}`, and the trim set is
//! that module's `RATING_TRIM`; the server's test suite checks they agree
//! on every spelling.
//!
//! `down` drops both columns. The recomputed ratings are not restored: the
//! old values depended on scan order and were never recorded, so the data
//! half of `down` is a deliberate no-op (the next scan keeps the derived
//! values either way).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

/// The ComicInfo `AgeRating` ladder, youngest audience first (index =
/// rank). Frozen here so this migration keeps meaning what it meant.
pub const AGE_RATING_LADDER: [&str; 13] = [
    "Early Childhood",
    "Everyone",
    "G",
    "Everyone 10+",
    "PG",
    "Kids to Adults",
    "Teen",
    "MA15+",
    "Mature 17+",
    "M",
    "R18+",
    "Adults Only 18+",
    "X18+",
];

/// Off-ladder words (Metron's vocabulary) and the rank each counts as.
pub const AGE_RATING_ALIASES: [(&str, u8); 4] = [
    ("Teen Plus", 7),
    ("Mature", 8),
    ("Adult", 11),
    ("Explicit", 12),
];

/// `field_provenance.set_by` codes of the file tier (the weakest
/// attribution: user > provider > file). Any other code protects the
/// series rating.
const FILE_TIER_SET_BY: [&str; 5] = [
    "comicinfo",
    "metroninfo",
    "series_json",
    "scanner_inference",
    "scanner_folder_tag",
];

/// `lower(btrim(<expr>, <trim set>))`, the runtime's rating normalisation.
fn norm(expr: &str) -> String {
    format!("lower(btrim({expr}, E' \\t\\n\\r\\f\\013'))")
}

/// `NOT EXISTS (...)`: no person- or provider-set rating on series `s`.
fn unprotected() -> String {
    let file_tier: Vec<String> = FILE_TIER_SET_BY.iter().map(|c| format!("'{c}'")).collect();
    format!(
        "NOT EXISTS ( \
           SELECT 1 FROM field_provenance fp \
            WHERE fp.entity_type = 'series' AND fp.entity_id = s.id::text \
              AND fp.field = 'age_rating' AND fp.set_by NOT IN ({}))",
        file_tier.join(", ")
    )
}

/// The statement recording a stored rating that ranks above every active
/// issue as the provisional declared floor (see the module docs). Runs
/// before [`recompute_all_series_age_ratings_sql`].
pub fn provisional_declared_sql() -> String {
    format!(
        "WITH ladder(norm, rank, canon) AS {ladder} \
         UPDATE series s SET declared_age_rating = s.age_rating \
          WHERE {unprotected} \
            AND (SELECT l.rank FROM ladder l WHERE l.norm = {stored}) > \
                COALESCE((SELECT max(l.rank) FROM issues i \
                            JOIN ladder l ON l.norm = {issue} \
                           WHERE i.series_id = s.id AND i.state = 'active' \
                             AND i.removed_at IS NULL), -1)",
        ladder = ladder_values(),
        unprotected = unprotected(),
        stored = norm("s.age_rating"),
        issue = norm("i.age_rating"),
    )
}

/// `(VALUES ('early childhood', 0, 'Early Childhood'), ...)`: every rung,
/// then every alias with its rung's canonical spelling.
fn ladder_values() -> String {
    let rows = AGE_RATING_LADDER
        .iter()
        .enumerate()
        .map(|(rank, rung)| (rung.to_lowercase(), rank))
        .chain(
            AGE_RATING_ALIASES
                .iter()
                .map(|(w, r)| (w.to_lowercase(), *r as usize)),
        );
    let ladder: Vec<String> = rows
        .map(|(norm, rank)| format!("('{norm}', {rank}, '{}')", AGE_RATING_LADDER[rank]))
        .collect();
    format!("(VALUES {})", ladder.join(", "))
}

/// The upgrade statement over every series: raise or fill, never lower
/// (see the module docs).
pub fn recompute_all_series_age_ratings_sql() -> String {
    format!(
        "WITH ladder(norm, rank, canon) AS {ladder}, \
         derived AS ( \
           SELECT s.id, \
                  (SELECT l.canon FROM ( \
                       SELECT i.age_rating AS r FROM issues i \
                        WHERE i.series_id = s.id AND i.state = 'active' \
                          AND i.removed_at IS NULL \
                       UNION ALL SELECT s.age_rating \
                     ) c \
                     JOIN ladder l ON l.norm = {norm} \
                    ORDER BY l.rank DESC LIMIT 1) AS rating \
             FROM series s \
            WHERE {unprotected} \
         ) \
         UPDATE series s SET age_rating = d.rating \
           FROM derived d \
          WHERE s.id = d.id AND d.rating IS NOT NULL \
            AND s.age_rating IS DISTINCT FROM d.rating",
        ladder = ladder_values(),
        norm = norm("c.r"),
        unprotected = unprotected(),
    )
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared(
            "ALTER TABLE library_user_access \
             ADD COLUMN IF NOT EXISTS hide_unrated BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE series ADD COLUMN IF NOT EXISTS declared_age_rating TEXT",
        )
        .await?;
        conn.execute_unprepared(&provisional_declared_sql()).await?;
        conn.execute_unprepared(&recompute_all_series_age_ratings_sql())
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Data: no-op by design (see the module docs).
        let conn = manager.get_connection();
        conn.execute_unprepared("ALTER TABLE series DROP COLUMN IF EXISTS declared_age_rating")
            .await?;
        conn.execute_unprepared(
            "ALTER TABLE library_user_access DROP COLUMN IF EXISTS hide_unrated",
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recompute_sql_carries_every_rung_in_order() {
        let sql = recompute_all_series_age_ratings_sql();
        let mut at = 0;
        for (rank, rung) in AGE_RATING_LADDER.iter().enumerate() {
            let row = format!("('{}', {rank}, '{rung}')", rung.to_lowercase());
            let pos = sql[at..]
                .find(&row)
                .unwrap_or_else(|| panic!("{row} in {sql}"));
            at += pos + row.len();
        }
        assert!(sql.contains("('teen plus', 7, 'MA15+')"), "{sql}");
        assert!(sql.contains("('explicit', 12, 'X18+')"), "{sql}");
        assert!(sql.contains("fp.set_by NOT IN ('comicinfo', 'metroninfo'"));
        // Never lowers: the stored rating competes, and NULL never wins.
        assert!(sql.contains("UNION ALL SELECT s.age_rating"), "{sql}");
        assert!(sql.contains("d.rating IS NOT NULL"), "{sql}");
        let sql = provisional_declared_sql();
        assert!(
            sql.contains("SET declared_age_rating = s.age_rating"),
            "{sql}"
        );
        assert!(sql.contains("('adult', 11, 'Adults Only 18+')"), "{sql}");
    }
}
