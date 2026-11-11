//! A series' age rating is derived from its issues.
//!
//! `series.age_rating` is the **highest-ranked** rating among the series'
//! active, not-removed issues, on the ladder in
//! [`crate::library::age_rating`] and after the same `lower(btrim(...))`
//! normalisation the cap uses. A rating with no rank (NULL, empty,
//! `Unknown`, `Rating Pending`, off-ladder text) is ignored, the way the
//! cap treats it as unrated; a series none of whose issues ranks is
//! unrated (NULL). The derived value is stored in the ladder's canonical
//! spelling.
//!
//! The derived value is file tier. A series whose `age_rating` field has
//! a `field_provenance` row set by a person (`user`) or a metadata
//! provider keeps that value: [`crate::metadata::writers::is_file_tier_set_by`]
//! decides, over the same `FILE_SOURCED_SET_BY` list (user > provider >
//! file). The scanner's writeback exception (a provider value the archive
//! already carries) does not arise here: no sidecar carries a series-level
//! rating, so a provider-set series rating always holds.
//!
//! [`recompute_series_age_ratings`] is one set-based statement per chunk
//! of series ids. It runs as a step of
//! `metadata_rollup::rollup_series_metadata` (every scan that ingests a
//! file of the series, the single-issue scan, the post-rewrite rescan of a
//! writeback library) and directly from every other path that changes an
//! issue's rating or a series' active issue set (issue PATCH, bulk
//! metadata, the DB-direct provider apply, soft-removal and restore). The
//! upgrade migration `m20271111_000001_series_rating_from_issues` runs the
//! same derivation once over every series, with its own frozen copy of
//! the ladder; `tests/age_rating_cap.rs` checks the two agree on every
//! rung.

use crate::library::age_rating::LADDER;
use crate::metadata::writers::FILE_SOURCED_SET_BY;
use sea_orm::{ConnectionTrait, DbErr, Statement, Value};
use uuid::Uuid;

/// Series ids per statement (well under Postgres' bind limits; the ids
/// travel as one array parameter anyway).
const CHUNK: usize = 1000;

/// `(VALUES ('early childhood', 0, 'Early Childhood'), ...)` built from
/// [`LADDER`]: normalised text, rank, canonical spelling.
pub fn ladder_values_sql() -> String {
    let rows: Vec<String> = LADDER
        .iter()
        .enumerate()
        .map(|(rank, rung)| {
            format!(
                "('{}', {rank}, '{}')",
                rung.to_ascii_lowercase().replace('\'', "''"),
                rung.replace('\'', "''")
            )
        })
        .collect();
    format!("(VALUES {})", rows.join(", "))
}

/// The derivation statement for the series ids bound as `$1` (a uuid
/// array); `$2` is the file-tier `set_by` list.
fn recompute_sql() -> String {
    format!(
        "WITH ladder(norm, rank, canon) AS {ladder}, \
         derived AS ( \
           SELECT s.id, \
                  (SELECT l.canon FROM issues i \
                     JOIN ladder l ON l.norm = lower(btrim(i.age_rating)) \
                    WHERE i.series_id = s.id AND i.state = 'active' \
                      AND i.removed_at IS NULL \
                    ORDER BY l.rank DESC LIMIT 1) AS rating \
             FROM series s \
            WHERE s.id = ANY($1) \
              AND NOT EXISTS ( \
                  SELECT 1 FROM field_provenance fp \
                   WHERE fp.entity_type = 'series' AND fp.entity_id = s.id::text \
                     AND fp.field = 'age_rating' AND fp.set_by <> ALL($2)) \
         ) \
         UPDATE series s SET age_rating = d.rating \
           FROM derived d \
          WHERE s.id = d.id AND s.age_rating IS DISTINCT FROM d.rating",
        ladder = ladder_values_sql(),
    )
}

/// Re-derive `series.age_rating` for one series. Returns `true` when the
/// stored value changed.
pub async fn recompute_series_age_rating<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Result<bool, DbErr> {
    Ok(recompute_series_age_ratings(db, &[series_id]).await? > 0)
}

/// Re-derive `series.age_rating` for each series in `series_ids` (a
/// person- or provider-set rating is left alone). Returns how many rows
/// changed, so a caller holding the `AppState` can drop the
/// similar-series cache, which carries each neighbour's rating.
pub async fn recompute_series_age_ratings<C: ConnectionTrait>(
    db: &C,
    series_ids: &[Uuid],
) -> Result<u64, DbErr> {
    if series_ids.is_empty() {
        return Ok(0);
    }
    let sql = recompute_sql();
    let file_tier: Vec<String> = FILE_SOURCED_SET_BY
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let mut changed = 0;
    for chunk in series_ids.chunks(CHUNK) {
        let res = db
            .execute_raw(Statement::from_sql_and_values(
                db.get_database_backend(),
                sql.as_str(),
                [Value::from(chunk.to_vec()), Value::from(file_tier.clone())],
            ))
            .await?;
        changed += res.rows_affected();
    }
    Ok(changed)
}

/// [`recompute_series_age_ratings`] for paths that must not fail on it
/// (scan reconcile, soft-removal and restore handlers): logs and swallows
/// the error, returns the changed count (0 on error). The next rollup of
/// the series retries.
pub async fn recompute_best_effort<C: ConnectionTrait>(db: &C, series_ids: &[Uuid]) -> u64 {
    match recompute_series_age_ratings(db, series_ids).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, series = series_ids.len(), "series age rating recompute failed");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_values_follow_the_ladder() {
        let sql = ladder_values_sql();
        assert!(sql.starts_with("(VALUES ('early childhood', 0, 'Early Childhood'), "));
        assert!(sql.ends_with("('x18+', 12, 'X18+'))"));
        assert_eq!(sql.matches("), (").count(), LADDER.len() - 1);
    }
}
