//! A series' age rating is derived from its issues, and a library grant
//! can hide unrated content.
//!
//! 1. `library_user_access.hide_unrated BOOLEAN NOT NULL DEFAULT FALSE`:
//!    when on, a series with no rating on the ladder is hidden from that
//!    member as if it ranked above the grant's cap (and its issues with
//!    it). Existing grants keep today's behaviour (unrated is shown).
//! 2. `series.age_rating` used to be copied once from whichever file the
//!    scanner read first. It is now the highest-ranked rating among the
//!    series' active, not-removed issues (`server::library::series_rating`),
//!    unless a person or a metadata provider set it (a `field_provenance`
//!    row for the field `age_rating` whose `set_by` is not a file-tier
//!    code). This migration recomputes every series that is not so set,
//!    in one statement. A rating that is not on the ladder ranks as
//!    unrated and never raises the result; a series with no ranked issue
//!    becomes unrated (NULL).
//!
//! [`AGE_RATING_LADDER`] is a frozen copy of
//! `server::library::age_rating::LADDER`; the server's test suite checks
//! the two agree on every rung.
//!
//! `down` drops the column. The recomputed ratings are not restored: the
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

/// The recompute statement over every series.
pub fn recompute_all_series_age_ratings_sql() -> String {
    let ladder: Vec<String> = AGE_RATING_LADDER
        .iter()
        .enumerate()
        .map(|(rank, rung)| format!("('{}', {rank}, '{}')", rung.to_ascii_lowercase(), rung))
        .collect();
    let file_tier: Vec<String> = FILE_TIER_SET_BY.iter().map(|c| format!("'{c}'")).collect();
    format!(
        "WITH ladder(norm, rank, canon) AS (VALUES {ladder}), \
         derived AS ( \
           SELECT s.id, \
                  (SELECT l.canon FROM issues i \
                     JOIN ladder l ON l.norm = lower(btrim(i.age_rating)) \
                    WHERE i.series_id = s.id AND i.state = 'active' \
                      AND i.removed_at IS NULL \
                    ORDER BY l.rank DESC LIMIT 1) AS rating \
             FROM series s \
            WHERE NOT EXISTS ( \
                  SELECT 1 FROM field_provenance fp \
                   WHERE fp.entity_type = 'series' AND fp.entity_id = s.id::text \
                     AND fp.field = 'age_rating' AND fp.set_by NOT IN ({file_tier})) \
         ) \
         UPDATE series s SET age_rating = d.rating \
           FROM derived d \
          WHERE s.id = d.id AND s.age_rating IS DISTINCT FROM d.rating",
        ladder = ladder.join(", "),
        file_tier = file_tier.join(", "),
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
        conn.execute_unprepared(&recompute_all_series_age_ratings_sql())
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Data: no-op by design (see the module docs).
        manager
            .get_connection()
            .execute_unprepared(
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
            let row = format!("('{}', {rank}, '{rung}')", rung.to_ascii_lowercase());
            let pos = sql[at..]
                .find(&row)
                .unwrap_or_else(|| panic!("{row} in {sql}"));
            at += pos + row.len();
        }
        assert!(sql.contains("fp.set_by NOT IN ('comicinfo', 'metroninfo'"));
    }
}
