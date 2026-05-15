//! `/issues/{issue_id}/next-up` — single-issue "what should I read next?"
//! resolver. Used by the reader to power its persistent "Up Next" pill
//! and end-of-issue card.
//!
//! Resolution order:
//!   1. If the caller passes `?cbl=<saved_view_id>` and the saved view
//!      resolves to a CBL the user can see + currently has the calling
//!      issue in it, return the next-unfinished entry after the current
//!      one in that list. Fall through on any CBL miss (deleted view,
//!      stale param, current issue not in the list).
//!   2. Otherwise (or after fallthrough), walk the current issue's
//!      series in sort order and return the first ACL-visible
//!      not-finished issue *strictly after* the current one.
//!   3. If both branches yield nothing, the response is `source: "none"`
//!      with an optional `fallback_suggestion` (the top On Deck card).
//!      The field is reserved in M1 and always populated `None`; M4/M5
//!      wires it up when the end-of-issue card needs it.
//!
//! The CBL > series ordering is deliberate: when the user opened the
//! reader from a CBL context, the list is their stated reading thread
//! and "next in series" is incidental. The series fallback only fires
//! when there's no CBL context or the CBL has nothing to say.
//!
//! This module also owns the two shared `pick_next_in_*` helpers that
//! `rails::on_deck` re-uses, so the "first unfinished" algorithm has a
//! single source of truth.

use axum::{
    Json, Router,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use entity::{cbl_entry, cbl_list, issue, progress_record, saved_view, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, sea_query::Expr};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::rails::OnDeckCard;
use crate::api::saved_views::KIND_CBL;
use crate::api::series::IssueSummaryView;
use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/issues/{issue_id}/next-up", get(next_up))
        .route("/issues/{issue_id}/prev-up", get(prev_up))
}

// ───── Response shapes ─────

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NextUpSource {
    /// The next-unfinished entry after the current issue in the CBL the
    /// caller passed via `?cbl=`.
    Cbl,
    /// The next-unfinished issue in the current issue's series, in sort
    /// order. Used when no CBL context was given, or the CBL branch had
    /// nothing to return.
    Series,
    /// Neither branch yielded a target — the user is at the end of both
    /// their series and (when applicable) their CBL.
    None,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct NextUpView {
    pub source: NextUpSource,
    /// The next-up issue, denormalized with its parent series slug + name
    /// so the reader can render the pill / end-card without a follow-up
    /// fetch. `None` iff `source == "none"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<IssueSummaryView>,
    /// Set when `source == "cbl"`. The id of the CBL list (NOT the
    /// saved-view id the caller sent) — useful for downstream UI that
    /// wants to deep-link into the CBL detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cbl_list_id: Option<String>,
    /// Set when `source == "cbl"`. The CBL list's human-readable name,
    /// for the chrome pill's "in {list}" sub-label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cbl_list_name: Option<String>,
    /// Set when `source == "cbl"`. 1-based position of the *target*
    /// entry within the CBL (so the reader can show "Issue 4 of 24").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cbl_position: Option<i32>,
    /// Set when `source == "cbl"`. Total matched entries in the CBL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cbl_total: Option<i32>,
    /// Top On Deck card to show as a peek when `source == "none"`. M1
    /// always returns `None`; populating it remains deferred work since
    /// the on_deck composition isn't factored as a reusable helper yet.
    /// The web layer falls back to a "Browse the library" CTA when this
    /// field is absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_suggestion: Option<OnDeckCard>,
    /// `true` when the caller passed `?cbl=<id>` but the current issue
    /// wasn't in that CBL (entry deleted, stale share-link, etc.). The
    /// web layer uses this as a signal to strip the dead param from the
    /// URL via `router.replace` so a refresh / shared link no longer
    /// carries the stale reference.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    #[serde(default)]
    pub cbl_param_was_stale: bool,
}

#[derive(Debug, Deserialize)]
pub struct NextUpQuery {
    /// Saved-view id of a CBL the caller is reading through. Optional;
    /// when omitted or invalid, the resolver falls back to series-next.
    pub cbl: Option<Uuid>,
}

// ───── Handler ─────

/// Resolve the next issue the user should read after the given one. Picks
/// CBL > series; returns `source: "none"` when neither branch has a
/// viable target.
#[utoipa::path(
    get,
    path = "/issues/{issue_id}/next-up",
    params(
        ("issue_id" = String, Path, description = "Current issue id"),
        ("cbl" = Option<String>, Query, description = "Saved-view id (kind='cbl') the caller is reading through")
    ),
    responses(
        (status = 200, body = NextUpView),
        (status = 404, description = "current issue not found or invisible to caller")
    )
)]
pub async fn next_up(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(issue_id): AxPath<String>,
    Query(q): Query<NextUpQuery>,
) -> Response {
    // Drop-on-exit timer — records every return path (404, error, ok)
    // without restructuring the early-return chains below. Default
    // Prometheus buckets cover the range we care about
    // (5ms → 10s); the series-walk worst case lives in the upper end.
    let _latency = LatencyTimer::new("comic_reader_next_up_latency_seconds");

    let acl = access::for_user(&app, &user).await;

    // Resolve the current issue + ACL-gate. Returning 404 (not 403) for
    // the no-ACL case matches every other read-path in the app.
    let current = match issue::Entity::find_by_id(issue_id.clone())
        .one(&app.db)
        .await
    {
        Ok(Some(m)) => m,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "issue not found"),
        Err(e) => {
            tracing::error!(error = %e, "next_up: current issue lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if !acl.contains(current.library_id) {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    // ───── CBL branch ─────
    let mut cbl_param_was_stale = false;
    if let Some(saved_view_id) = q.cbl {
        match resolve_cbl_next(&app, user.id, saved_view_id, &current, &acl).await {
            Ok(CblBranchOutcome::Resolved(mut view)) => {
                view.cbl_param_was_stale = false;
                record_resolved("cbl");
                return Json(view).into_response();
            }
            Ok(CblBranchOutcome::StaleParam) => {
                // Current issue is not in the referenced CBL. Resolver
                // still falls through to series, but flags the result
                // so the web layer can scrub `?cbl=` from the URL.
                tracing::debug!(
                    user_id = %user.id, issue_id = %current.id, saved_view_id = %saved_view_id,
                    "next_up: ?cbl= param was stale, falling back to series"
                );
                cbl_param_was_stale = true;
            }
            Ok(CblBranchOutcome::NoMatch) => {
                // View doesn't resolve (deleted / wrong kind / not
                // owned by caller) OR every later CBL entry is
                // finished. Same fall-through behavior as the stale
                // case but no URL-scrub hint — the param may still be
                // valid context for a future visit (e.g., user adds an
                // unfinished entry later).
                tracing::debug!(
                    user_id = %user.id, issue_id = %current.id, saved_view_id = %saved_view_id,
                    "next_up: CBL branch yielded nothing, falling back to series"
                );
            }
            Err(resp) => return resp,
        }
    }

    // ───── Series branch ─────
    match pick_next_in_series_after(&app, user.id, &current, &acl).await {
        Ok(Some(next)) => {
            let series_row = match series::Entity::find_by_id(current.series_id)
                .one(&app.db)
                .await
            {
                Ok(Some(s)) => s,
                _ => {
                    // Exceptional path — series row missing despite the
                    // issue resolving. Don't bother fetching a fallback
                    // suggestion; the next-up resolver should fail
                    // simple here.
                    record_resolved("none");
                    return Json(none_view(cbl_param_was_stale)).into_response();
                }
            };
            let target = IssueSummaryView::from_model(next, &series_row.slug)
                .with_series_name(series_row.name);
            record_resolved("series");
            Json(NextUpView {
                source: NextUpSource::Series,
                target: Some(target),
                cbl_list_id: None,
                cbl_list_name: None,
                cbl_position: None,
                cbl_total: None,
                fallback_suggestion: None,
                cbl_param_was_stale,
            })
            .into_response()
        }
        Ok(None) => {
            // Clean caught-up — populate `fallback_suggestion` with the
            // top On Deck card (D-6) so the end-of-issue card has
            // something to render in its caught-up state.
            record_resolved("none");
            let view =
                none_view_with_fallback(&app, user.id, &acl, &current.id, cbl_param_was_stale)
                    .await;
            Json(view).into_response()
        }
        Err(resp) => resp,
    }
}

/// Bumps the `comic_reader_next_up_resolved_total{source=…}` counter.
/// Sampled label cardinality is bounded to {cbl, series, none}.
fn record_resolved(source: &'static str) {
    metrics::counter!("comic_reader_next_up_resolved_total", "source" => source).increment(1);
}

/// Records a named histogram on drop. Owned by handler entries as
/// `let _latency = LatencyTimer::new("…")` so every return path
/// (including early `?` propagation and 404s) is sampled without
/// restructuring the handler body. Parameterized over the metric name
/// so `next_up` and `prev_up` can share the implementation.
struct LatencyTimer {
    start: std::time::Instant,
    metric: &'static str,
}

impl LatencyTimer {
    fn new(metric: &'static str) -> Self {
        Self {
            start: std::time::Instant::now(),
            metric,
        }
    }
}

impl Drop for LatencyTimer {
    fn drop(&mut self) {
        metrics::histogram!(self.metric).record(self.start.elapsed().as_secs_f64());
    }
}

fn none_view(cbl_param_was_stale: bool) -> NextUpView {
    NextUpView {
        source: NextUpSource::None,
        target: None,
        cbl_list_id: None,
        cbl_list_name: None,
        cbl_position: None,
        cbl_total: None,
        fallback_suggestion: None,
        cbl_param_was_stale,
    }
}

/// Same as [`none_view`] but populates `fallback_suggestion` with the
/// top On Deck card for the user (excluding the issue they just
/// finished). Failures fetching the fallback degrade silently — the
/// caught-up response stays useful even if the On Deck composition
/// hits a DB error.
async fn none_view_with_fallback(
    app: &AppState,
    user_id: Uuid,
    acl: &access::VisibleLibraries,
    current_issue_id: &str,
    cbl_param_was_stale: bool,
) -> NextUpView {
    let fallback = crate::api::rails::top_on_deck_card(app, user_id, acl, Some(current_issue_id))
        .await
        .ok()
        .flatten();
    NextUpView {
        source: NextUpSource::None,
        target: None,
        cbl_list_id: None,
        cbl_list_name: None,
        cbl_position: None,
        cbl_total: None,
        fallback_suggestion: fallback,
        cbl_param_was_stale,
    }
}

/// Outcomes for the CBL branch. Letting the caller distinguish the
/// "current issue not in this list" case from other misses is what
/// powers the self-healing `cbl_param_was_stale` hint on the response.
///
/// `Resolved` carries the ~700-byte `NextUpView`; the other variants
/// are unit. clippy flags the size delta but it's a transient
/// stack-only value returned from one function — boxing would just add
/// an allocation for no benefit.
#[allow(clippy::large_enum_variant)]
enum CblBranchOutcome {
    /// Saved view resolved, current issue is in the list, and at least
    /// one later entry is unfinished.
    Resolved(NextUpView),
    /// Saved view resolved + valid, but the current issue is NOT in
    /// the list. Triggers `cbl_param_was_stale: true` so the web can
    /// drop `?cbl=` from the URL.
    StaleParam,
    /// Any other miss (view doesn't exist, wrong kind, not owned by
    /// caller, every later entry finished). Same fall-through behavior
    /// as `StaleParam` but no URL-scrub hint.
    NoMatch,
}

/// CBL branch of the resolver. The caller distinguishes `StaleParam`
/// (current issue not in this list — drives the self-healing URL
/// scrub) from `NoMatch` (any other miss) via [`CblBranchOutcome`].
/// `Err` is reserved for genuine DB failures.
async fn resolve_cbl_next(
    app: &AppState,
    user_id: Uuid,
    saved_view_id: Uuid,
    current: &issue::Model,
    acl: &access::VisibleLibraries,
) -> Result<CblBranchOutcome, Response> {
    let sv = match saved_view::Entity::find_by_id(saved_view_id)
        .one(&app.db)
        .await
    {
        Ok(opt) => opt,
        Err(e) => {
            tracing::error!(error = %e, "next_up: saved view lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    let Some(sv) = sv else {
        return Ok(CblBranchOutcome::NoMatch);
    };
    // CBL saved views are either system-owned (NULL user_id) or owned by
    // the calling user. Anything else is "someone else's view" and gets
    // dropped silently.
    if let Some(owner) = sv.user_id
        && owner != user_id
    {
        return Ok(CblBranchOutcome::NoMatch);
    }
    if sv.kind != KIND_CBL {
        return Ok(CblBranchOutcome::NoMatch);
    }
    let Some(cbl_list_id) = sv.cbl_list_id else {
        return Ok(CblBranchOutcome::NoMatch);
    };

    // Find the entry that points at the current issue. If the issue
    // isn't in this list at all, this is the *stale param* case — the
    // referenced CBL exists and is valid, but the current issue isn't
    // a member of it. Flag it so the web can scrub `?cbl=` from the
    // URL.
    let current_entry = match cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(cbl_list_id))
        .filter(cbl_entry::Column::MatchedIssueId.eq(current.id.clone()))
        .one(&app.db)
        .await
    {
        Ok(opt) => opt,
        Err(e) => {
            tracing::error!(error = %e, "next_up: current cbl_entry lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    let Some(current_entry) = current_entry else {
        return Ok(CblBranchOutcome::StaleParam);
    };

    let next = match pick_next_in_cbl(app, user_id, cbl_list_id, acl, Some(current_entry.position))
        .await
    {
        Ok(opt) => opt,
        Err(resp) => return Err(resp),
    };
    let Some((issue_model, series_slug, series_name, position)) = next else {
        return Ok(CblBranchOutcome::NoMatch);
    };

    // Fetch the list once for its display name + total matched count.
    let list_row = match cbl_list::Entity::find_by_id(cbl_list_id).one(&app.db).await {
        Ok(Some(l)) => l,
        _ => return Ok(CblBranchOutcome::NoMatch),
    };
    let total = match cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(cbl_list_id))
        .filter(cbl_entry::Column::MatchedIssueId.is_not_null())
        .select_only()
        .column_as(Expr::col(cbl_entry::Column::Id).count(), "count")
        .into_tuple::<i64>()
        .one(&app.db)
        .await
    {
        Ok(Some(n)) => n as i32,
        _ => 0,
    };

    let target =
        IssueSummaryView::from_model(issue_model, &series_slug).with_series_name(series_name);
    Ok(CblBranchOutcome::Resolved(NextUpView {
        source: NextUpSource::Cbl,
        target: Some(target),
        cbl_list_id: Some(cbl_list_id.to_string()),
        cbl_list_name: Some(list_row.parsed_name),
        cbl_position: Some(position),
        cbl_total: Some(total),
        fallback_suggestion: None,
        cbl_param_was_stale: false,
    }))
}

// ───── Shared helpers (also used by rails::on_deck) ─────

/// Server-side port of the client's `pickNextIssue` algorithm
/// ([web/lib/reading-state.ts]) — applied to a single series. Returns the
/// next-unread `issue::Model` or `None` if every active issue is already
/// finished / there are no active issues at all.
///
/// Called from the On Deck handler only after we've already filtered out
/// series with an in-progress issue, so step 1 of the original algorithm
/// (continue resumable in-progress) is a no-op here. We still apply step
/// 2 (first unfinished) and skip step 3 (all-finished restart) because
/// "Read again" doesn't belong in an On Deck queue.
pub(crate) async fn pick_next_in_series(
    app: &AppState,
    user_id: Uuid,
    series_id: Uuid,
) -> Result<Option<issue::Model>, Response> {
    let issues: Vec<issue::Model> = match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(series_id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .order_by_asc(Expr::cust("sort_number IS NULL"))
        .order_by_asc(issue::Column::SortNumber)
        .order_by_asc(issue::Column::Id)
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "rails: pick_next_in_series issues lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    if issues.is_empty() {
        return Ok(None);
    }
    let issue_ids: Vec<String> = issues.iter().map(|i| i.id.clone()).collect();
    let progress_rows = match progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user_id))
        .filter(progress_record::Column::IssueId.is_in(issue_ids))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "rails: pick_next_in_series progress lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    let progress_by_id: std::collections::HashMap<String, progress_record::Model> = progress_rows
        .into_iter()
        .map(|p| (p.issue_id.clone(), p))
        .collect();

    for iss in issues {
        let progress = progress_by_id.get(&iss.id);
        let finished = progress.map(|p| p.finished).unwrap_or(false);
        if !finished {
            return Ok(Some(iss));
        }
    }
    Ok(None)
}

/// "Next unread issue *strictly after* the given one" within the same
/// series. Differs from [`pick_next_in_series`]: that helper returns the
/// first unread issue anywhere in the series (used by On Deck, where the
/// user hasn't told us where they were); this one respects the user's
/// stated position so finishing issue 5 with an unfinished issue 3 lying
/// around still surfaces issue 6 next, not issue 3.
async fn pick_next_in_series_after(
    app: &AppState,
    user_id: Uuid,
    current: &issue::Model,
    acl: &access::VisibleLibraries,
) -> Result<Option<issue::Model>, Response> {
    let issues: Vec<issue::Model> = match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(current.series_id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .order_by_asc(Expr::cust("sort_number IS NULL"))
        .order_by_asc(issue::Column::SortNumber)
        .order_by_asc(issue::Column::Id)
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "next_up: pick_next_in_series_after issues lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    if issues.is_empty() {
        return Ok(None);
    }
    let issue_ids: Vec<String> = issues.iter().map(|i| i.id.clone()).collect();
    let progress_rows = match progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user_id))
        .filter(progress_record::Column::IssueId.is_in(issue_ids))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "next_up: pick_next_in_series_after progress lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    let progress_by_id: std::collections::HashMap<String, progress_record::Model> = progress_rows
        .into_iter()
        .map(|p| (p.issue_id.clone(), p))
        .collect();

    // Walk past the current issue in sort order, then return the first
    // unfinished issue. ACL is the series's library — series can't span
    // libraries today, but check the parent in case that changes.
    if !acl.contains(current.library_id) {
        return Ok(None);
    }
    let mut seen_current = false;
    for iss in issues {
        if !seen_current {
            if iss.id == current.id {
                seen_current = true;
            }
            continue;
        }
        let finished = progress_by_id
            .get(&iss.id)
            .map(|p| p.finished)
            .unwrap_or(false);
        if !finished {
            return Ok(Some(iss));
        }
    }
    Ok(None)
}

/// For a CBL list, find the lowest-position matched entry whose issue is
/// not yet finished + is visible to the user (library ACL). When
/// `start_after` is `Some(p)`, only entries with `position > p` are
/// considered — used by the next-up resolver to step past the entry the
/// caller is currently on. `None` keeps the legacy "lowest unfinished"
/// behavior the On Deck rail relies on.
pub(crate) async fn pick_next_in_cbl(
    app: &AppState,
    user_id: Uuid,
    cbl_list_id: Uuid,
    acl: &access::VisibleLibraries,
    start_after: Option<i32>,
) -> Result<Option<(issue::Model, String, String, i32)>, Response> {
    let mut select = cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(cbl_list_id))
        .filter(cbl_entry::Column::MatchedIssueId.is_not_null());
    if let Some(after) = start_after {
        select = select.filter(cbl_entry::Column::Position.gt(after));
    }
    let entries = match select
        .order_by_asc(cbl_entry::Column::Position)
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "next_up: pick_next_in_cbl entries lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    if entries.is_empty() {
        return Ok(None);
    }

    let matched_ids: Vec<String> = entries
        .iter()
        .filter_map(|e| e.matched_issue_id.clone())
        .collect();
    let progress_rows = match progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user_id))
        .filter(progress_record::Column::IssueId.is_in(matched_ids))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "next_up: pick_next_in_cbl progress lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    let progress_by_issue: std::collections::HashMap<String, progress_record::Model> =
        progress_rows
            .into_iter()
            .map(|p| (p.issue_id.clone(), p))
            .collect();

    for entry in entries {
        let Some(issue_id) = entry.matched_issue_id.clone() else {
            continue;
        };
        let finished = progress_by_issue
            .get(&issue_id)
            .map(|p| p.finished)
            .unwrap_or(false);
        if finished {
            continue;
        }
        let Ok(Some(issue_model)) = issue::Entity::find_by_id(issue_id).one(&app.db).await else {
            continue;
        };
        if !acl.contains(issue_model.library_id) {
            continue;
        }
        let (series_slug, series_name) = match series::Entity::find_by_id(issue_model.series_id)
            .one(&app.db)
            .await
        {
            Ok(Some(s)) => (s.slug, s.name),
            _ => continue,
        };
        return Ok(Some((
            issue_model,
            series_slug,
            series_name,
            entry.position + 1,
        )));
    }
    Ok(None)
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

// ───────────────────────────────────────────────────────────────────
// `/issues/{id}/prev-up` — navigate backwards in series / CBL.
// ───────────────────────────────────────────────────────────────────
//
// Symmetric with `next_up` but with two semantic differences:
//
//   - "Prev" is pure sequential navigation, not a reading queue: the
//     `pick_prev_*` helpers do NOT filter by `finished` state. If the
//     user is on issue 5 and issues 3-4 are already finished, prev
//     still returns issue 4 — the user is asking to back up one step,
//     not to find the most recent unread thing.
//   - `fallback_suggestion` is intentionally never populated for prev.
//     "You're already at the start, here's an unrelated suggestion"
//     doesn't make sense.
//
// Reuses `NextUpView` as the response shape (same JSON contract); the
// matching web hook `usePrevUp` consumes it identically to `useNextUp`.

/// Resolve the previous issue the user should navigate to. Picks
/// CBL > series; returns `source: "none"` when neither branch has a
/// viable target (e.g., user is on the first issue of the series and
/// no CBL context).
#[utoipa::path(
    get,
    path = "/issues/{issue_id}/prev-up",
    params(
        ("issue_id" = String, Path, description = "Current issue id"),
        ("cbl" = Option<String>, Query, description = "Saved-view id (kind='cbl') the caller is reading through")
    ),
    responses(
        (status = 200, body = NextUpView),
        (status = 404, description = "current issue not found or invisible to caller")
    )
)]
pub async fn prev_up(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(issue_id): AxPath<String>,
    Query(q): Query<NextUpQuery>,
) -> Response {
    let _latency = LatencyTimer::new("comic_reader_prev_up_latency_seconds");

    let acl = access::for_user(&app, &user).await;

    let current = match issue::Entity::find_by_id(issue_id.clone())
        .one(&app.db)
        .await
    {
        Ok(Some(m)) => m,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "issue not found"),
        Err(e) => {
            tracing::error!(error = %e, "prev_up: current issue lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if !acl.contains(current.library_id) {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    // ───── CBL branch ─────
    let mut cbl_param_was_stale = false;
    if let Some(saved_view_id) = q.cbl {
        match resolve_cbl_prev(&app, user.id, saved_view_id, &current, &acl).await {
            Ok(CblBranchOutcome::Resolved(mut view)) => {
                view.cbl_param_was_stale = false;
                record_resolved_prev("cbl");
                return Json(view).into_response();
            }
            Ok(CblBranchOutcome::StaleParam) => {
                tracing::debug!(
                    user_id = %user.id, issue_id = %current.id, saved_view_id = %saved_view_id,
                    "prev_up: ?cbl= param was stale, falling back to series"
                );
                cbl_param_was_stale = true;
            }
            Ok(CblBranchOutcome::NoMatch) => {
                tracing::debug!(
                    user_id = %user.id, issue_id = %current.id, saved_view_id = %saved_view_id,
                    "prev_up: CBL branch yielded nothing, falling back to series"
                );
            }
            Err(resp) => return resp,
        }
    }

    // ───── Series branch ─────
    match pick_prev_in_series_before(&app, &current, &acl).await {
        Ok(Some(prev)) => {
            let series_row = match series::Entity::find_by_id(current.series_id)
                .one(&app.db)
                .await
            {
                Ok(Some(s)) => s,
                _ => {
                    record_resolved_prev("none");
                    return Json(none_view(cbl_param_was_stale)).into_response();
                }
            };
            let target = IssueSummaryView::from_model(prev, &series_row.slug)
                .with_series_name(series_row.name);
            record_resolved_prev("series");
            Json(NextUpView {
                source: NextUpSource::Series,
                target: Some(target),
                cbl_list_id: None,
                cbl_list_name: None,
                cbl_position: None,
                cbl_total: None,
                fallback_suggestion: None,
                cbl_param_was_stale,
            })
            .into_response()
        }
        Ok(None) => {
            // No prev anywhere — user's already at the start. Don't
            // populate fallback_suggestion (per the design note at the
            // top of this section); just return a clean none.
            record_resolved_prev("none");
            Json(none_view(cbl_param_was_stale)).into_response()
        }
        Err(resp) => resp,
    }
}

fn record_resolved_prev(source: &'static str) {
    metrics::counter!("comic_reader_prev_up_resolved_total", "source" => source).increment(1);
}

/// CBL branch of the prev-up resolver. Same outcome enum as the
/// next-up CBL branch; the stale-param self-healing flag works
/// identically (current issue not in the referenced list).
async fn resolve_cbl_prev(
    app: &AppState,
    user_id: Uuid,
    saved_view_id: Uuid,
    current: &issue::Model,
    acl: &access::VisibleLibraries,
) -> Result<CblBranchOutcome, Response> {
    let sv = match saved_view::Entity::find_by_id(saved_view_id)
        .one(&app.db)
        .await
    {
        Ok(opt) => opt,
        Err(e) => {
            tracing::error!(error = %e, "prev_up: saved view lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    let Some(sv) = sv else {
        return Ok(CblBranchOutcome::NoMatch);
    };
    if let Some(owner) = sv.user_id
        && owner != user_id
    {
        return Ok(CblBranchOutcome::NoMatch);
    }
    if sv.kind != KIND_CBL {
        return Ok(CblBranchOutcome::NoMatch);
    }
    let Some(cbl_list_id) = sv.cbl_list_id else {
        return Ok(CblBranchOutcome::NoMatch);
    };

    let current_entry = match cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(cbl_list_id))
        .filter(cbl_entry::Column::MatchedIssueId.eq(current.id.clone()))
        .one(&app.db)
        .await
    {
        Ok(opt) => opt,
        Err(e) => {
            tracing::error!(error = %e, "prev_up: current cbl_entry lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    let Some(current_entry) = current_entry else {
        return Ok(CblBranchOutcome::StaleParam);
    };

    let prev = match pick_prev_in_cbl(app, cbl_list_id, current_entry.position, acl).await {
        Ok(opt) => opt,
        Err(resp) => return Err(resp),
    };
    let Some((issue_model, series_slug, series_name, position)) = prev else {
        return Ok(CblBranchOutcome::NoMatch);
    };

    let list_row = match cbl_list::Entity::find_by_id(cbl_list_id).one(&app.db).await {
        Ok(Some(l)) => l,
        _ => return Ok(CblBranchOutcome::NoMatch),
    };
    let total = match cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(cbl_list_id))
        .filter(cbl_entry::Column::MatchedIssueId.is_not_null())
        .select_only()
        .column_as(Expr::col(cbl_entry::Column::Id).count(), "count")
        .into_tuple::<i64>()
        .one(&app.db)
        .await
    {
        Ok(Some(n)) => n as i32,
        _ => 0,
    };

    let target =
        IssueSummaryView::from_model(issue_model, &series_slug).with_series_name(series_name);
    Ok(CblBranchOutcome::Resolved(NextUpView {
        source: NextUpSource::Cbl,
        target: Some(target),
        cbl_list_id: Some(cbl_list_id.to_string()),
        cbl_list_name: Some(list_row.parsed_name),
        cbl_position: Some(position),
        cbl_total: Some(total),
        fallback_suggestion: None,
        cbl_param_was_stale: false,
    }))
}

/// "Previous issue strictly before the given one" within the same
/// series. Pure sequential nav — no progress lookup, no finished
/// filter. The user is asking to back up one step in sort order.
async fn pick_prev_in_series_before(
    app: &AppState,
    current: &issue::Model,
    acl: &access::VisibleLibraries,
) -> Result<Option<issue::Model>, Response> {
    if !acl.contains(current.library_id) {
        return Ok(None);
    }
    let issues: Vec<issue::Model> = match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(current.series_id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .order_by_asc(Expr::cust("sort_number IS NULL"))
        .order_by_asc(issue::Column::SortNumber)
        .order_by_asc(issue::Column::Id)
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "prev_up: pick_prev_in_series_before issues lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };

    // Walk forward, track the latest issue seen before `current`. When
    // we hit current, return the tracked candidate. Simpler than a
    // reverse iteration + the ordering is already done by SQL.
    let mut candidate: Option<issue::Model> = None;
    for iss in issues {
        if iss.id == current.id {
            return Ok(candidate);
        }
        candidate = Some(iss);
    }
    Ok(None)
}

/// CBL prev: lowest-position matched entry strictly before
/// `before_position` that is ACL-visible. Pure sequential nav — no
/// finished filter; user is asking to back up one step in the list.
async fn pick_prev_in_cbl(
    app: &AppState,
    cbl_list_id: Uuid,
    before_position: i32,
    acl: &access::VisibleLibraries,
) -> Result<Option<(issue::Model, String, String, i32)>, Response> {
    let entries = match cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(cbl_list_id))
        .filter(cbl_entry::Column::MatchedIssueId.is_not_null())
        .filter(cbl_entry::Column::Position.lt(before_position))
        // DESC so the first ACL-visible entry we find is the closest
        // one before the current position.
        .order_by_desc(cbl_entry::Column::Position)
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "prev_up: pick_prev_in_cbl entries lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };

    for entry in entries {
        let Some(issue_id) = entry.matched_issue_id.clone() else {
            continue;
        };
        let Ok(Some(issue_model)) = issue::Entity::find_by_id(issue_id).one(&app.db).await else {
            continue;
        };
        if !acl.contains(issue_model.library_id) {
            continue;
        }
        let (series_slug, series_name) = match series::Entity::find_by_id(issue_model.series_id)
            .one(&app.db)
            .await
        {
            Ok(Some(s)) => (s.slug, s.name),
            _ => continue,
        };
        return Ok(Some((
            issue_model,
            series_slug,
            series_name,
            entry.position + 1,
        )));
    }
    Ok(None)
}
