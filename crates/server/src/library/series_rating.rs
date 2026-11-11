//! A series' age rating is derived from its issues.
//!
//! `series.age_rating` is the **highest-ranked** rating among the series'
//! active, not-removed issues **and** the rating its folder's
//! `series.json` declares (`series.declared_age_rating`, a floor), on the
//! ladder in [`crate::library::age_rating`] (aliases included) and after
//! the same [`age_rating::sql_norm`] normalisation the cap uses. A rating
//! with no rank (NULL, empty, `Unknown`, `Rating Pending`, off-ladder
//! text) is ignored, the way the cap treats it as unrated; a series with
//! nothing that ranks is unrated (NULL). The derived value is stored in
//! the ladder's canonical spelling (an alias such as `Mature` is stored
//! as `Mature 17+`).
//!
//! `declared_age_rating` is written by the scanner
//! ([`apply_declared_age_rating`]) from every scan that reads the
//! folder's `series.json`; a sidecar that is gone or carries no rating
//! clears it. The upgrade migration seeds a provisional value (a stored
//! rating ranking above every issue) that the first scan replaces.
//!
//! The derived value is file tier. A series whose `age_rating` field has
//! a `field_provenance` row set by a person (`user`) or a metadata
//! provider keeps that value: [`crate::metadata::writers::is_file_tier_set_by`]
//! decides, over the same `FILE_SOURCED_SET_BY` list (user > provider >
//! file). No metadata provider writes `series.age_rating` today, so in
//! practice the holder is a person (series PATCH); clearing the rating on
//! a series PATCH removes that pin and re-derives.
//!
//! Two recomputes of one series racing each other can each read the
//! issue set as of its own statement start; the last writer wins and the
//! next rollup of the series corrects any stale result (no row lock: a
//! lock taken inside the statement would not refresh its snapshot).
//!
//! [`recompute_series_age_ratings`] is one set-based statement per chunk
//! of series ids. It runs as a step of
//! `metadata_rollup::rollup_series_metadata` (every scan that ingests a
//! file of the series, the single-issue scan, the post-rewrite rescan of a
//! writeback library) and directly from every other path that changes an
//! issue's rating or a series' active issue set (issue PATCH, bulk
//! metadata, the DB-direct provider apply, soft-removal and restore). The
//! upgrade migration `m20271111_000001_series_rating_from_issues` runs a
//! never-lowering form once over every series (max of the stored rating
//! and the issues), with its own frozen copy of the ladder;
//! `tests/age_rating_cap.rs` checks the two ladders agree on every
//! spelling.

use crate::library::age_rating::{self, LADDER};
use crate::metadata::writers::FILE_SOURCED_SET_BY;
use sea_orm::{ConnectionTrait, DbErr, Statement, Value};
use uuid::Uuid;

/// Series ids per statement (well under Postgres' bind limits; the ids
/// travel as one array parameter anyway).
const CHUNK: usize = 1000;

/// `(VALUES ('early childhood', 0, 'Early Childhood'), ...)` built from
/// [`age_rating::spellings`]: normalised text, rank, canonical ladder
/// spelling. Aliases follow the rungs (`('mature', 8, 'Mature 17+')`).
pub fn ladder_values_sql() -> String {
    let rows: Vec<String> = age_rating::spellings()
        .map(|(norm, rank)| {
            format!(
                "('{}', {rank}, '{}')",
                norm.replace('\'', "''"),
                LADDER[rank as usize].replace('\'', "''")
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
                  (SELECT l.canon FROM ( \
                       SELECT i.age_rating AS r FROM issues i \
                        WHERE i.series_id = s.id AND i.state = 'active' \
                          AND i.removed_at IS NULL \
                       UNION ALL SELECT s.declared_age_rating \
                     ) c \
                     JOIN ladder l ON l.norm = {norm} \
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
        norm = age_rating::sql_norm("c.r"),
    )
}

/// Store the rating a series folder's `series.json` declares
/// (`declared` = `None` when the sidecar is gone or has no rating) and,
/// when it changed, re-derive the series rating. Blank text is stored as
/// NULL. Returns `true` when `series.age_rating` changed.
pub async fn apply_declared_age_rating<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
    declared: Option<&str>,
) -> Result<bool, DbErr> {
    let declared = declared
        .map(|d| d.trim_matches(age_rating::RATING_TRIM))
        .filter(|d| !d.is_empty())
        .map(str::to_owned);
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            db.get_database_backend(),
            "UPDATE series SET declared_age_rating = $2 \
              WHERE id = $1 AND declared_age_rating IS DISTINCT FROM $2",
            [Value::from(series_id), Value::from(declared)],
        ))
        .await?;
    if res.rows_affected() == 0 {
        return Ok(false);
    }
    recompute_series_age_rating(db, series_id).await
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
        assert!(sql.contains("('x18+', 12, 'X18+'), "));
        assert!(sql.contains("('teen plus', 7, 'MA15+')"));
        assert!(sql.ends_with("('explicit', 12, 'X18+'))"));
        assert_eq!(
            sql.matches("), (").count(),
            LADDER.len() + age_rating::ALIASES.len() - 1
        );
    }
}
