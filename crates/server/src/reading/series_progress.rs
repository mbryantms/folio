//! Helper API around the `user_series_progress` SQL view.
//!
//! Two access paths:
//!
//!   - [`subquery_for`] — returns a `sea_query::SelectStatement` filtered
//!     by `user_id`. M3's filter compiler plugs this into a `LEFT JOIN`
//!     on `series.id` so reading-state predicates compose with the rest
//!     of the saved-view query. Centralizing the column list here means
//!     callers don't have to know the view's column shape.
//!   - [`fetch_for_series`] / [`fetch_for_series_batch`] — direct entity
//!     reads for handlers that just want a per-user summary (e.g.,
//!     `GET /series/{slug}` or the home rail counters).
//!
//! Coverage caveat: the view only has rows for `(user, series)` pairs the
//! user has actually started. Filter queries `LEFT JOIN` and use
//! `COALESCE(percent, 0)` to make unstarted series compare as 0%. Direct
//! lookups return `None`.

use entity::user_series_progress;
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    sea_query::{Alias, Expr, Query, SelectStatement},
};
use std::collections::HashMap;
use uuid::Uuid;

/// Build a `SELECT user_id, series_id, finished_count, total_count,
/// percent, last_read_at FROM user_series_progress WHERE user_id = $1`
/// statement intended for use as a derived table inside a `LEFT JOIN`.
///
/// Sample usage from a future filter compiler:
///
/// ```ignore
/// let mut q = Query::select();
/// q.from(series::Entity)
///  .left_join_lateral(
///      series_progress::subquery_for(user_id),
///      Alias::new("usp"),
///      Expr::col((Alias::new("usp"), Alias::new("series_id")))
///         .equals(series::Column::Id),
///  )
///  .and_where(
///      Expr::cust("COALESCE(usp.percent, 0)").gte(50),
///  );
/// ```
pub fn subquery_for(user_id: Uuid) -> SelectStatement {
    Query::select()
        .columns([
            user_series_progress::Column::UserId,
            user_series_progress::Column::SeriesId,
            user_series_progress::Column::FinishedCount,
            user_series_progress::Column::TotalCount,
            user_series_progress::Column::Percent,
            user_series_progress::Column::LastReadAt,
        ])
        .from(user_series_progress::Entity)
        .and_where(Expr::col(user_series_progress::Column::UserId).eq(user_id))
        .to_owned()
}

/// Conventional alias for the subquery in M3's join chain. Centralized so
/// the compiler doesn't sprinkle a string literal across every reading-
/// state predicate.
pub fn subquery_alias() -> Alias {
    Alias::new("usp")
}

/// One-row lookup. Returns `None` when the user has no progress records
/// in the series — caller decides what 0% / null-last-read should mean.
pub async fn fetch_for_series<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    series_id: Uuid,
) -> Result<Option<user_series_progress::Model>, sea_orm::DbErr> {
    user_series_progress::Entity::find()
        .filter(user_series_progress::Column::UserId.eq(user_id))
        .filter(user_series_progress::Column::SeriesId.eq(series_id))
        .one(db)
        .await
}

/// Bulk lookup keyed by `series_id`. Series the user hasn't started are
/// absent from the returned map (callers `unwrap_or_default` to a zeroed
/// summary). Single round-trip — used by list endpoints that want to
/// hydrate per-user state across many series.
pub async fn fetch_for_series_batch<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    series_ids: &[Uuid],
) -> Result<HashMap<Uuid, user_series_progress::Model>, sea_orm::DbErr> {
    if series_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = user_series_progress::Entity::find()
        .filter(user_series_progress::Column::UserId.eq(user_id))
        .filter(user_series_progress::Column::SeriesId.is_in(series_ids.to_vec()))
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| (r.series_id, r)).collect())
}
