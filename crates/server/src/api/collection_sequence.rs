//! Reading order of a user collection (saved view `kind = 'collection'`).
//!
//! A collection is a user-curated list of mixed series + issue entries,
//! ordered by `collection_entries.position`. When the reader is opened
//! from a collection (`?collection=<saved_view_id>`), "next" must follow
//! that order rather than the parent series — the same way a CBL reading
//! list drives the reader via `?cbl=`. This module turns the entries into
//! one flat issue sequence so the next-up / prev-up resolvers and the
//! collection page's "Continue" button all walk the same list:
//!
//!   - an **issue entry** contributes that one issue;
//!   - a **series entry** expands to the series' active issues in the
//!     series-page order — the main run first (`sort_number` ASC, NULLs
//!     last, then `id`), then the specials (annuals / one-shots / TPBs)
//!     in the same order — so reading "all of Invincible" inside a
//!     collection follows the run and finishes with the extras;
//!   - an issue that appears twice (directly and via its series) is kept
//!     at its first position only.
//!
//! Collections are strictly user-owned (no system collections except
//! Want to Read, which is still per-user), so a view belonging to
//! another user resolves to `None` and the caller falls through to the
//! series branch — never a 403, the context param is advisory.
//!
//! ACL: the sequence keeps every entry so positions are stable; the
//! walks skip issues the caller cannot open (`VisibleLibraries::issue_ok`,
//! issue rating with the series rating as fallback) exactly as the CBL
//! walk does.

use std::collections::HashMap;

use axum::{http::StatusCode, response::Response};
use entity::{collection_entry, issue, progress_record, saved_view, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, sea_query::Expr};
use uuid::Uuid;

use super::error;
use crate::api::issue_card::IssueCardRow;
use crate::api::saved_views::KIND_COLLECTION;
use crate::library::access;
use crate::state::AppState;

/// A collection flattened to its issues in reading order.
pub(crate) struct CollectionSequence {
    pub view: saved_view::Model,
    /// Flat reading order (see the module docs). May be empty.
    pub items: Vec<IssueCardRow>,
    /// Parent series of every item, for the slug / name on the target
    /// view and the series-rating fallback in the ACL check.
    pub series_by_id: HashMap<Uuid, series::Model>,
}

impl CollectionSequence {
    /// Index of `issue_id` in the sequence, if the collection contains
    /// it (directly or through its series).
    pub(crate) fn index_of(&self, issue_id: &str) -> Option<usize> {
        self.items.iter().position(|i| i.id == issue_id)
    }

    /// Number of issues in the sequence, for the "N of M" badge.
    pub(crate) fn total(&self) -> i32 {
        i32::try_from(self.items.len()).unwrap_or(i32::MAX)
    }

    /// Can the caller open the item at `idx`? Membership + age cap, the
    /// issue's own rating first and the series rating as fallback.
    pub(crate) fn visible(&self, acl: &access::VisibleLibraries, idx: usize) -> bool {
        let item = &self.items[idx];
        let series_rating = self
            .series_by_id
            .get(&item.series_id)
            .and_then(|s| s.age_rating.as_deref());
        acl.issue_ok(item.library_id, item.age_rating.as_deref(), series_rating)
    }

    /// Parent series of the item at `idx`. `None` only if the series row
    /// vanished between the two reads — callers skip such an item.
    pub(crate) fn series_of(&self, idx: usize) -> Option<&series::Model> {
        self.series_by_id.get(&self.items[idx].series_id)
    }

    /// The first item strictly after `after` (or from the top when
    /// `None`) that the caller can open and has not finished. This is
    /// the reading-queue rule shared with the CBL branch: finished
    /// entries are stepped over, so "continue the collection" lands on
    /// the next thing left to read.
    pub(crate) fn first_unfinished_after(
        &self,
        acl: &access::VisibleLibraries,
        progress: &HashMap<String, progress_record::Model>,
        after: Option<usize>,
    ) -> Option<usize> {
        let start = after.map(|i| i + 1).unwrap_or(0);
        (start..self.items.len()).find(|&idx| {
            let finished = progress
                .get(&self.items[idx].id)
                .map(|p| p.finished)
                .unwrap_or(false);
            !finished && self.series_of(idx).is_some() && self.visible(acl, idx)
        })
    }

    /// The closest item strictly before `before` that the caller can
    /// open. Pure sequential navigation — no finished filter, the user
    /// is backing up one step.
    pub(crate) fn prev_visible_before(
        &self,
        acl: &access::VisibleLibraries,
        before: usize,
    ) -> Option<usize> {
        (0..before)
            .rev()
            .find(|&idx| self.series_of(idx).is_some() && self.visible(acl, idx))
    }
}

/// Resolve `saved_view_id` as a collection owned by `user_id` and flatten
/// it. `Ok(None)` when the view is missing, not a collection, or someone
/// else's; `Err` only for DB failures.
pub(crate) async fn load(
    app: &AppState,
    user_id: Uuid,
    saved_view_id: Uuid,
) -> Result<Option<CollectionSequence>, Response> {
    let view = saved_view::Entity::find_by_id(saved_view_id)
        .one(&app.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "collection_sequence: saved view lookup failed");
            internal()
        })?;
    let Some(view) = view else {
        return Ok(None);
    };
    if view.kind != KIND_COLLECTION || view.user_id != Some(user_id) {
        return Ok(None);
    }
    load_for_view(app, view).await.map(Some)
}

/// Flatten an already-authorised collection view.
pub(crate) async fn load_for_view(
    app: &AppState,
    view: saved_view::Model,
) -> Result<CollectionSequence, Response> {
    let entries = collection_entry::Entity::find()
        .filter(collection_entry::Column::SavedViewId.eq(view.id))
        .order_by_asc(collection_entry::Column::Position)
        .all(&app.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "collection_sequence: entries lookup failed");
            internal()
        })?;

    let series_ids: Vec<Uuid> = entries.iter().filter_map(|e| e.series_id).collect();
    let issue_ids: Vec<String> = entries.iter().filter_map(|e| e.issue_id.clone()).collect();

    // Series entries: every active issue of every referenced series in
    // one query, ordered main-run-first then series-page order, then
    // grouped per series (the ORDER BY keeps each group's order).
    let mut issues_by_series: HashMap<Uuid, Vec<IssueCardRow>> = HashMap::new();
    if !series_ids.is_empty() {
        let rows = issue::Entity::find()
            .filter(issue::Column::SeriesId.is_in(series_ids.clone()))
            .filter(issue::Column::State.eq(issue::IssueState::Active))
            .filter(issue::Column::RemovedAt.is_null())
            .order_by_asc(Expr::cust("special_type IS NOT NULL"))
            .order_by_asc(Expr::cust("sort_number IS NULL"))
            .order_by_asc(issue::Column::SortNumber)
            .order_by_asc(issue::Column::Id)
            .into_partial_model::<IssueCardRow>()
            .all(&app.db)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "collection_sequence: series issues lookup failed");
                internal()
            })?;
        for row in rows {
            issues_by_series.entry(row.series_id).or_default().push(row);
        }
    }

    // Issue entries: the referenced rows, active only (a removed or
    // malformed issue cannot be read, so it leaves the sequence).
    let mut issue_by_id: HashMap<String, IssueCardRow> = HashMap::new();
    if !issue_ids.is_empty() {
        let rows = issue::Entity::find()
            .filter(issue::Column::Id.is_in(issue_ids.clone()))
            .filter(issue::Column::State.eq(issue::IssueState::Active))
            .filter(issue::Column::RemovedAt.is_null())
            .into_partial_model::<IssueCardRow>()
            .all(&app.db)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "collection_sequence: issue entries lookup failed");
                internal()
            })?;
        for row in rows {
            issue_by_id.insert(row.id.clone(), row);
        }
    }

    // Flatten in entry order; an issue reachable twice keeps its first
    // slot so the position badge never counts it twice.
    let mut items: Vec<IssueCardRow> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in &entries {
        if let Some(sid) = entry.series_id {
            if let Some(rows) = issues_by_series.get(&sid) {
                for row in rows {
                    if seen.insert(row.id.clone()) {
                        items.push(row.clone());
                    }
                }
            }
        } else if let Some(iid) = entry.issue_id.as_deref()
            && let Some(row) = issue_by_id.get(iid)
            && seen.insert(row.id.clone())
        {
            items.push(row.clone());
        }
    }

    let mut parent_ids: Vec<Uuid> = items.iter().map(|i| i.series_id).collect();
    parent_ids.sort();
    parent_ids.dedup();
    let series_by_id: HashMap<Uuid, series::Model> = if parent_ids.is_empty() {
        HashMap::new()
    } else {
        series::Entity::find()
            .filter(series::Column::Id.is_in(parent_ids))
            .all(&app.db)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "collection_sequence: series hydrate failed");
                internal()
            })?
            .into_iter()
            .map(|s| (s.id, s))
            .collect()
    };

    Ok(CollectionSequence {
        view,
        items,
        series_by_id,
    })
}

/// The caller's progress rows for every item of the sequence, keyed by
/// issue id.
pub(crate) async fn progress_map(
    app: &AppState,
    user_id: Uuid,
    seq: &CollectionSequence,
) -> Result<HashMap<String, progress_record::Model>, Response> {
    if seq.items.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<String> = seq.items.iter().map(|i| i.id.clone()).collect();
    let rows = progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user_id))
        .filter(progress_record::Column::IssueId.is_in(ids))
        .all(&app.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "collection_sequence: progress lookup failed");
            internal()
        })?;
    Ok(rows.into_iter().map(|p| (p.issue_id.clone(), p)).collect())
}

fn internal() -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}
