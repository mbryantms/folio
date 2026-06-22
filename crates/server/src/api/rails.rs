//! Home-page system rails: Continue Reading + On Deck + per-user dismissals.
//!
//! Endpoints:
//!   - `GET    /me/continue-reading`               — partially-read issues
//!   - `GET    /me/on-deck`                        — (M3) — next-up issues + CBLs
//!   - `POST   /me/rail-dismissals`                — hide a target from a rail
//!   - `DELETE /me/rail-dismissals/{kind}/{id}`    — undo a dismissal
//!
//! Dismissals auto-restore: a row stays in the table once written, but
//! gets filtered out of the rail query as soon as the target has new
//! activity past `dismissed_at`. That way the user always sees an
//! up-to-date "where was I?" list without needing a separate restore
//! click for every paused issue they re-opened.
//!
//! See plan: `Continue Reading + On Deck home rails` for the full data
//! model, dismiss semantics, and rail composition rules.
//!
//! M1 ships the Continue Reading endpoint + dismiss endpoints. M3 adds
//! On Deck. Both rails share the same dismissal table, so M3 only needs
//! to extend the query side.

use axum::{
    Extension, Json,
    extract::{Path as AxPath, State},
    http::{HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::{cbl_entry, issue, progress_record, rail_dismissal, series};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DbBackend, DerivePartialModel, EntityTrait, FromQueryResult,
    QueryFilter, QueryOrder, Set, Statement, prelude::DateTimeWithTimeZone, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::{error, not_found};
use crate::api::series::IssueSummaryView;
use crate::auth::CurrentUser;
use crate::library::access;
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

const DISMISS_KIND_ISSUE: &str = "issue";
const DISMISS_KIND_SERIES: &str = "series";
const DISMISS_KIND_CBL: &str = "cbl";

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(continue_reading))
        .routes(routes!(on_deck))
        .routes(routes!(create_dismissal))
        .routes(routes!(delete_dismissal))
}

// ───── Response shapes ─────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProgressInfo {
    /// Last viewed page index (0-based).
    pub last_page: i32,
    /// 0.0–1.0 fraction read; computed from `last_page / page_count`.
    pub percent: f64,
    /// RFC 3339 timestamp of the most recent progress write.
    pub updated_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContinueReadingCard {
    pub issue: IssueSummaryView,
    pub series_name: String,
    pub progress: ProgressInfo,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContinueReadingView {
    pub items: Vec<ContinueReadingCard>,
}

/// Discriminated union for the On Deck rail. Each card represents one
/// "what's next" suggestion — either the next-unread issue in a series the
/// user has finished at least one issue of, or the next-unread entry in a
/// CBL list the user is working through.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OnDeckCard {
    SeriesNext {
        issue: IssueSummaryView,
        series_name: String,
        last_activity: String,
    },
    CblNext {
        issue: IssueSummaryView,
        cbl_list_id: String,
        cbl_list_name: String,
        /// Saved-view id (kind=`cbl`) wrapping this CBL list. Web
        /// threads it onto the reader URL as `?cbl=<id>` so the next-up
        /// resolver keeps picking from the list across page turns.
        /// Wrapper-less lists are no longer candidates (ghost guard),
        /// so cards always carry `Some` in practice; the field stays
        /// `Option` for wire-compat.
        ///
        /// Tiebreak when multiple saved views match: user-owned wins
        /// over system-owned (NULL `user_id`); within a tier, lowest
        /// `id` wins. Stable + cheap; the picker UI can pick a
        /// different one later if needed.
        #[serde(skip_serializing_if = "Option::is_none")]
        cbl_saved_view_id: Option<String>,
        /// 1-based position of the entry within its CBL list (matches the
        /// "#N" badge the CBL detail UI shows).
        position: i32,
        last_activity: String,
    },
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OnDeckView {
    pub items: Vec<OnDeckCard>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateDismissalReq {
    /// One of `'issue'`, `'series'`, `'cbl'`.
    pub target_kind: String,
    pub target_id: String,
}

// ───── Handlers ─────

/// In-progress issues for the calling user, ordered by most recent
/// activity. Excludes finished issues, issues that haven't actually been
/// opened (`last_page = 0`), removed/encrypted issues, issues the user
/// can't see (library ACL), and issues whose dismissal is still current
/// (no newer progress write past the dismissal timestamp).
///
/// Implementation note: dismissals require comparing `progress_records.
/// updated_at` against `rail_dismissals.dismissed_at`, so we let Postgres
/// do that in one SQL pass rather than re-filtering rows in Rust.
#[utoipa::path(
    operation_id = "rails_continue_reading",    get,
    path = "/me/continue-reading",
    responses((status = 200, body = ContinueReadingView))
)]
#[handler]
pub async fn continue_reading(State(app): State<AppState>, user: CurrentUser) -> Response {
    let acl = access::for_user(&app, &user).await;

    #[derive(Debug, FromQueryResult)]
    struct Row {
        issue_id: String,
        last_page: i32,
        percent: f64,
        progress_updated_at: chrono::DateTime<chrono::FixedOffset>,
        library_id: Uuid,
        series_slug: String,
        series_name: String,
    }

    // We pull the columns we need to render `IssueSummaryView` + the
    // progress overlay + the parent series slug in one query. The
    // `library_id` round-trips so we can apply the ACL filter in Rust
    // (the same pattern the rest of the API uses for non-admin users).
    let rows: Vec<Row> = match Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                p.issue_id            AS issue_id,
                p.last_page           AS last_page,
                p.percent             AS percent,
                p.updated_at          AS progress_updated_at,
                i.library_id          AS library_id,
                s.slug                AS series_slug,
                s.name                AS series_name
            FROM progress_records p
            JOIN issues  i ON i.id = p.issue_id
            JOIN series  s ON s.id = i.series_id
            LEFT JOIN rail_dismissals d
              ON d.user_id = p.user_id
             AND d.target_kind = 'issue'
             AND d.target_id   = p.issue_id
            WHERE p.user_id   = $1
              AND p.finished  = false
              AND p.last_page > 0
              AND i.state     = 'active'
              AND i.removed_at IS NULL
              AND (d.dismissed_at IS NULL OR p.updated_at > d.dismissed_at)
            ORDER BY p.updated_at DESC
            LIMIT 24
        "#,
        [user.id.into()],
    ))
    .all(&app.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "rails: continue-reading query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    if rows.is_empty() {
        return Json(ContinueReadingView { items: Vec::new() }).into_response();
    }

    // Hydrate full issue::Model rows for `IssueSummaryView::from_model`.
    // One batched fetch keeps it O(1) round-trips even for the full 24.
    let issue_ids: Vec<String> = rows.iter().map(|r| r.issue_id.clone()).collect();
    let issue_rows = match issue::Entity::find()
        .filter(issue::Column::Id.is_in(issue_ids.clone()))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "rails: continue-reading hydrate failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let issue_by_id: std::collections::HashMap<String, issue::Model> =
        issue_rows.into_iter().map(|i| (i.id.clone(), i)).collect();

    let items: Vec<ContinueReadingCard> = rows
        .into_iter()
        .filter_map(|row| {
            if !acl.contains(row.library_id) {
                return None;
            }
            let issue_model = issue_by_id.get(&row.issue_id)?.clone();
            Some(ContinueReadingCard {
                issue: IssueSummaryView::from_model(issue_model, &row.series_slug)
                    .with_series_name(row.series_name.clone()),
                series_name: row.series_name,
                progress: ProgressInfo {
                    last_page: row.last_page,
                    percent: row.percent,
                    updated_at: row.progress_updated_at.to_rfc3339(),
                },
            })
        })
        .collect();

    Json(ContinueReadingView { items }).into_response()
}

/// "What's next" suggestions for the home page. Returns a mix of
/// `series_next` cards (next-unread issue in a series the user has read
/// at least one finished issue of, with no in-progress issue blocking the
/// queue) and `cbl_next` cards (lowest-position unfinished matched entry
/// in a CBL the user has any progress in). Series with an active in-
/// progress issue are skipped — they already surface in Continue Reading.
#[utoipa::path(
    operation_id = "rails_on_deck",    get,
    path = "/me/on-deck",
    responses((status = 200, body = OnDeckView))
)]
#[handler]
pub async fn on_deck(State(app): State<AppState>, user: CurrentUser) -> Response {
    // Time the whole handler (ACL + composition) so the cost shows up as a
    // `Server-Timing` entry in the browser's Network → Timing panel — the
    // same place the slow TTFB was first observed. Lets an operator confirm
    // the rail's server cost directly without scraping `/metrics`.
    let started = std::time::Instant::now();
    let acl = access::for_user(&app, &user).await;
    let mut items = match compute_on_deck(&app, user.id, &acl).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    items.truncate(24);
    let mut resp = Json(OnDeckView { items }).into_response();
    let dur_ms = started.elapsed().as_secs_f64() * 1000.0;
    if let Ok(value) = HeaderValue::from_str(&format!("compute;dur={dur_ms:.1}")) {
        resp.headers_mut()
            .insert(HeaderName::from_static("server-timing"), value);
    }
    resp
}

/// Composes the On Deck rail's cards (series_next + cbl_next mixed),
/// sorted by most-recent activity desc. Same logic the `/me/on-deck`
/// handler ships — extracted so the next-up resolver can ask for a
/// single "top" card to render as the caught-up state's fallback
/// suggestion. Returns the FULL sorted list; callers cap as they need.
///
/// Pre-D-6 this was inline inside `on_deck`; the extraction is purely
/// mechanical (same queries, same dedup, same ordering).
pub(crate) async fn compute_on_deck(
    app: &AppState,
    user_id: Uuid,
    acl: &access::VisibleLibraries,
) -> Result<Vec<OnDeckCard>, Response> {
    let mut items: Vec<(chrono::DateTime<chrono::FixedOffset>, OnDeckCard)> = Vec::new();
    // SeriesNext cards are deferred into this buffer and filtered against
    // the CBL set after both queries run. CBL framing wins on overlap and
    // the dedup is series-wide, not issue-exact: if a series has *any*
    // issue inside a CBL that's currently surfacing in On Deck, the user
    // has signalled "I want to read this body of work in the CBL's order"
    // and the SeriesNext card just adds noise (especially when its
    // first-unread pick disagrees with the CBL's curated position — e.g.,
    // CBL points at #20 while the bare series points at #1 for a user
    // who has 1..50 on disk but only #20 in the CBL). The SeriesNext
    // resurfaces naturally once every CBL covering that series leaves
    // On Deck.
    let mut series_buf: Vec<(chrono::DateTime<chrono::FixedOffset>, OnDeckCard)> = Vec::new();
    let mut cbl_owned_series_ids: std::collections::HashSet<Uuid> =
        std::collections::HashSet::new();

    // ───── series_next candidates ─────
    //
    // Series the user has *meaningful* progress in, MAX(updated_at) per
    // series, but excluding series with a still-in-progress issue (those
    // land in Continue Reading instead). "Meaningful" = finished OR read
    // past page 0; "mark all as unread" writes (last_page=0, finished=
    // false) rows rather than deleting, so without this filter a fully-
    // reset series would keep surfacing the first issue as on-deck.
    // Dismissals are honored with auto-restore (the dismissal expires
    // once new progress lands past `dismissed_at`).
    // `series_slug` joins through the same `JOIN series s` already in
    // the CTE — pulling it inline lets the per-row loop skip a second
    // `series::Entity::find_by_id` round-trip (audit-remediation M5.1).
    #[derive(Debug, FromQueryResult)]
    struct SeriesRow {
        series_id: Uuid,
        series_name: String,
        series_slug: String,
        last_activity: chrono::DateTime<chrono::FixedOffset>,
        library_id: Uuid,
    }
    let series_candidates: Vec<SeriesRow> =
        match SeriesRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                WITH started AS (
                    SELECT i.series_id, MAX(p.updated_at) AS last_activity
                    FROM progress_records p
                    JOIN issues i ON i.id = p.issue_id
                    WHERE p.user_id = $1
                      AND i.state = 'active'
                      AND i.removed_at IS NULL
                      AND (p.finished = true OR p.last_page > 0)
                    GROUP BY i.series_id
                ),
                in_progress AS (
                    SELECT DISTINCT i.series_id
                    FROM progress_records p
                    JOIN issues i ON i.id = p.issue_id
                    WHERE p.user_id = $1
                      AND p.finished = false
                      AND p.last_page > 0
                      AND i.state = 'active'
                      AND i.removed_at IS NULL
                )
                SELECT s.id AS series_id, s.name AS series_name,
                       s.slug AS series_slug,
                       started.last_activity AS last_activity,
                       s.library_id AS library_id
                FROM started
                JOIN series s ON s.id = started.series_id
                LEFT JOIN rail_dismissals d
                  ON d.user_id = $1
                 AND d.target_kind = 'series'
                 AND d.target_id = s.id::text
                WHERE started.series_id NOT IN (SELECT series_id FROM in_progress)
                  AND (d.dismissed_at IS NULL OR started.last_activity > d.dismissed_at)
                ORDER BY started.last_activity DESC
                LIMIT 40
            "#,
            [user_id.into()],
        ))
        .all(&app.db)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "rails: on-deck series query failed");
                return Err(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal",
                ));
            }
        };

    // Resolve every visible series candidate's next-up pick in a constant
    // number of round-trips: bulk-hydrate all candidates' issues + the
    // user's progress once, then run the SAME in-memory walk the reader's
    // next-up resolver uses (`walk_series_continue_pick`) per series. This
    // anchors on the user's latest finished issue (finishing #20 surfaces
    // #21, not #1) and falls back to earliest-unread when no finished
    // issue exists — identical to the per-candidate path it replaces,
    // which ran 2 queries per series and made On Deck scale O(series).
    let visible_series: Vec<&SeriesRow> = series_candidates
        .iter()
        .filter(|r| acl.contains(r.library_id))
        .collect();
    let series_ids: Vec<Uuid> = visible_series.iter().map(|r| r.series_id).collect();
    let issues_by_series = match hydrate_series_issues(app, &series_ids).await {
        Ok(m) => m,
        Err(resp) => return Err(resp),
    };
    let series_issue_ids: Vec<String> = issues_by_series
        .values()
        .flat_map(|v| v.iter().map(|i| i.id.clone()))
        .collect();
    let series_progress = match fetch_progress_for(app, user_id, &series_issue_ids).await {
        Ok(m) => m,
        Err(resp) => return Err(resp),
    };
    for row in &visible_series {
        let Some(issues) = issues_by_series.get(&row.series_id) else {
            continue;
        };
        let Some(issue_model) =
            crate::api::next_up::walk_series_continue_pick(issues, &series_progress)
        else {
            continue;
        };
        series_buf.push((
            row.last_activity,
            OnDeckCard::SeriesNext {
                issue: issue_model
                    .into_summary_view(&row.series_slug)
                    .with_series_name(row.series_name.clone()),
                series_name: row.series_name.clone(),
                last_activity: row.last_activity.to_rfc3339(),
            },
        ));
    }

    // ───── cbl_next candidates ─────
    //
    // CBLs the user is *actively* reading. The SQL below is a coarse
    // superset pre-filter (any matched-issue progress + a saved-view
    // wrapper visible to the caller); the authoritative candidacy checks
    // live in the per-candidate loop:
    //   - a saved view must wrap the list (ghost guard — wrapper-less
    //     lists from partial imports or wrapper-only deletes must not
    //     surface cards the user can't navigate to);
    //   - the finished prefix must be non-empty (the user actually
    //     started the list — without this, reading an issue that happens
    //     to sit deep inside a master reading order, via a series or a
    //     different CBL, would surface that list's entry 1 as "up next");
    //   - ranking + dismissal auto-restore key on the *frontier*
    //     activity (MAX(updated_at) over the finished prefix), so deep
    //     cross-reads neither bump a stale list to the top nor
    //     un-dismiss it.
    // We pull the per-list pick in one scan per candidate; the total
    // candidate count is bounded by the user's actual CBL usage so the
    // round-trip count is small in practice.
    //
    #[derive(Debug, FromQueryResult)]
    struct CblCandidate {
        cbl_list_id: Uuid,
        cbl_list_name: String,
        dismissed_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    }
    let cbl_candidates: Vec<CblCandidate> =
        match CblCandidate::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            // `(p.finished = true OR p.last_page > 0)` mirrors the
            // series-side meaningful-progress filter. Without it,
            // "mark all unread" leaves zeroed progress_records rows
            // on the formerly-read CBL entries and the CBL keeps
            // surfacing in On Deck (asymmetric with the series-side
            // reset behaviour). With it, a fully-reset CBL drops off
            // the rail the same way a fully-reset series does.
            //
            // The saved-view EXISTS lives in SQL (not just the loop's
            // map check) because LIMIT 40 runs here: a library with many
            // wrapper-less lists would otherwise fill every slot with
            // ghosts and starve the rail.
            //
            // The HAVING dismissal clause is a *conservative* pre-filter
            // only: frontier activity ⊆ all activity, so MAX(all) ≤
            // dismissed_at implies frontier ≤ dismissed_at and nothing
            // the loop would keep is dropped here. The authoritative
            // restore decision (frontier > dismissed_at) runs in the
            // loop. MAX(p.updated_at) is likewise retained only as the
            // LIMIT-ordering key; emitted cards rank by frontier
            // activity. Residual cost: contaminated-but-started lists
            // can still occupy pre-LIMIT slots — bounded by real usage;
            // raise the LIMIT if it ever bites.
            r#"
                SELECT cl.id AS cbl_list_id, cl.parsed_name AS cbl_list_name,
                       d.dismissed_at AS dismissed_at
                FROM progress_records p
                JOIN cbl_entries e ON e.matched_issue_id = p.issue_id
                JOIN cbl_lists  cl ON cl.id = e.cbl_list_id
                LEFT JOIN rail_dismissals d
                  ON d.user_id = $1
                 AND d.target_kind = 'cbl'
                 AND d.target_id = cl.id::text
                WHERE p.user_id = $1
                  AND (p.finished = true OR p.last_page > 0)
                  AND (cl.owner_user_id IS NULL OR cl.owner_user_id = $1)
                  AND EXISTS (
                      SELECT 1 FROM saved_views sv
                      WHERE sv.kind = 'cbl'
                        AND sv.cbl_list_id = cl.id
                        AND (sv.user_id IS NULL OR sv.user_id = $1)
                  )
                GROUP BY cl.id, cl.parsed_name, d.dismissed_at
                HAVING d.dismissed_at IS NULL OR MAX(p.updated_at) > d.dismissed_at
                ORDER BY MAX(p.updated_at) DESC
                LIMIT 40
            "#,
            [user_id.into()],
        ))
        .all(&app.db)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "rails: on-deck cbl query failed");
                return Err(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal",
                ));
            }
        };

    let cbl_candidate_ids: Vec<Uuid> = cbl_candidates.iter().map(|c| c.cbl_list_id).collect();

    // Pre-fetch the (cbl_list_id → saved_view_id) lookup for only the
    // bounded candidate set. The CBL candidate SQL already enforces the
    // saved-view EXISTS predicate, but this race-window guard keeps emitted
    // cards navigable and supplies the `?cbl=` saved-view id. Tiebreak:
    // user-owned saved view wins over system-owned (NULL user_id); within
    // a tier, lowest id wins.
    let cbl_saved_view_by_list_id =
        cbl_saved_view_ids_for_candidates(app, user_id, &cbl_candidate_ids).await?;

    // Series-wide ownership per CBL — pre-fetched in one round-trip
    // across every candidate (audit-remediation M5.2). Over-fetching
    // for CBLs that turn out to have no next is cheap compared to the
    // per-iteration query the old `series_ids_in_cbl(one)` helper
    // produced. See comment on `cbl_owned_series_ids` above for why we
    // shadow every series in the CBL, not just the currently-pointed
    // issue.
    let cbl_series_ownership: std::collections::HashMap<Uuid, Vec<Uuid>> = {
        match series_ids_in_cbls(app, &cbl_candidate_ids).await {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "rails: on-deck cbl series-ownership lookup failed");
                return Err(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal",
                ));
            }
        }
    };

    // Bulk-hydrate every candidate list's matched entries + their issues,
    // parent series, and the user's progress in a constant number of
    // round-trips, then run the SAME in-memory walk the resolver uses
    // (`walk_cbl_pick`) per list. Replaces the per-candidate
    // `pick_next_in_cbl_frontier` call, which ran several queries per list
    // and made On Deck scale O(active CBLs).
    let (cbl_entries_by_list, cbl_issue_by_id, cbl_series_by_id, cbl_progress) =
        match hydrate_cbl_picks(app, user_id, &cbl_candidate_ids).await {
            Ok(t) => t,
            Err(resp) => return Err(resp),
        };

    for cand in &cbl_candidates {
        // Ghost guard. The SQL EXISTS already filtered; this re-check
        // keeps the map and the candidate query coherent across their
        // race window. Emitted cards therefore always carry a saved-view
        // id the caller can navigate to.
        let Some(sv_id) = cbl_saved_view_by_list_id.get(&cand.cbl_list_id) else {
            continue;
        };
        let entries = cbl_entries_by_list
            .get(&cand.cbl_list_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let Some(pick) = crate::api::next_up::walk_cbl_pick(
            entries,
            &cbl_progress,
            &cbl_issue_by_id,
            &cbl_series_by_id,
            acl,
        ) else {
            continue;
        };
        // Frontier candidacy: a list with an empty finished prefix was
        // never started by the user — any progress intersecting it came
        // from reading the same issues in another context, and a card
        // pointing at its entry 1 is pure noise. (Seam for a future
        // explicit "track this list" opt-in: bypass this guard for
        // tracked lists and fall back to the inclusion's created_at as
        // the sort timestamp.)
        let Some(frontier_ts) = pick.prefix_last_activity else {
            continue;
        };
        // Dismissal auto-restore keys on frontier activity only — a deep
        // cross-read must not un-dismiss the list. Strict '>' matches
        // the SQL HAVING's semantics.
        if let Some(dismissed_at) = cand.dismissed_at
            && frontier_ts <= dismissed_at
        {
            continue;
        }
        if let Some(series_ids) = cbl_series_ownership.get(&cand.cbl_list_id) {
            cbl_owned_series_ids.extend(series_ids.iter().copied());
        }
        items.push((
            frontier_ts,
            OnDeckCard::CblNext {
                issue: pick
                    .issue
                    .into_summary_view(&pick.series_slug)
                    .with_series_name(pick.series_name),
                cbl_list_id: cand.cbl_list_id.to_string(),
                cbl_list_name: cand.cbl_list_name.clone(),
                cbl_saved_view_id: Some(sv_id.to_string()),
                position: pick.position,
                last_activity: frontier_ts.to_rfc3339(),
            },
        ));
    }

    // Drain the deferred SeriesNext buffer, skipping any whose series
    // is already owned by a CBL card on this rail.
    for (ts, card) in series_buf {
        let OnDeckCard::SeriesNext { issue, .. } = &card else {
            items.push((ts, card));
            continue;
        };
        // `issue.series_id` is a UUID string on the view; parse to
        // match the CBL ownership set's key type.
        let Ok(sid) = Uuid::parse_str(&issue.series_id) else {
            items.push((ts, card));
            continue;
        };
        if cbl_owned_series_ids.contains(&sid) {
            continue;
        }
        items.push((ts, card));
    }

    // ───── Continue Reading dedup ─────
    //
    // Any issue the user has in-progress (read past page 0, not finished)
    // already surfaces in the Continue Reading rail, so it must never also
    // appear here. SeriesNext cards can't trip this — their series is
    // excluded upstream by the `in_progress` CTE — but a CblNext card points
    // at the next *unfinished* CBL entry, and "unfinished" includes
    // "in-progress", so a half-read issue can leak in via the CBL path.
    // Mirror Continue Reading's filter (finished = false AND last_page > 0)
    // and drop any matching card.
    #[derive(Debug, FromQueryResult)]
    struct InProgressRow {
        issue_id: String,
    }
    let in_progress: std::collections::HashSet<String> =
        match InProgressRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT DISTINCT p.issue_id AS issue_id
                FROM progress_records p
                JOIN issues i ON i.id = p.issue_id
                WHERE p.user_id = $1
                  AND p.finished = false
                  AND p.last_page > 0
                  AND i.state = 'active'
                  AND i.removed_at IS NULL
            "#,
            [user_id.into()],
        ))
        .all(&app.db)
        .await
        {
            Ok(rows) => rows.into_iter().map(|r| r.issue_id).collect(),
            Err(e) => {
                tracing::error!(error = %e, "rails: on-deck in-progress dedup query failed");
                return Err(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal",
                ));
            }
        };
    if !in_progress.is_empty() {
        items.retain(|(_, card)| {
            let id = match card {
                OnDeckCard::SeriesNext { issue, .. } | OnDeckCard::CblNext { issue, .. } => {
                    issue.id.as_str()
                }
            };
            !in_progress.contains(id)
        });
    }

    // Merge by most-recent activity desc; caller caps to per-surface
    // limit (the rail truncates to 24, the next-up resolver takes 1).
    items.sort_by_key(|item| std::cmp::Reverse(item.0));

    // ───── Final dedup by issue id ─────
    //
    // The composition can emit the same issue from more than one source.
    // The series-wide CBL>Series dedup above only suppresses SeriesNext
    // cards whose series a CBL owns; it never dedups one CblNext against
    // another. So an issue that is the next-up pick of two different CBL
    // lists (the series belongs to both) produces two identical CblNext
    // cards. Collapse to one card per issue, keeping the first occurrence
    // — after the sort that's the card with the most recent activity.
    let mut seen_issue_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    items.retain(|(_, card)| {
        let id = match card {
            OnDeckCard::SeriesNext { issue, .. } | OnDeckCard::CblNext { issue, .. } => {
                issue.id.clone()
            }
        };
        seen_issue_ids.insert(id)
    });

    Ok(items.into_iter().map(|(_, c)| c).collect())
}

/// Top On Deck card for the user, excluding cards that target the
/// given issue (so the next-up resolver doesn't suggest the issue the
/// reader is already on). Drives the `fallback_suggestion` field on
/// `NextUpView` when `source == "none"`. Returns `Ok(None)` when no
/// applicable card exists (new user, fully caught-up, etc.).
pub(crate) async fn top_on_deck_card(
    app: &AppState,
    user_id: Uuid,
    acl: &access::VisibleLibraries,
    exclude_issue_id: Option<&str>,
) -> Result<Option<OnDeckCard>, Response> {
    let cards = compute_on_deck(app, user_id, acl).await?;
    Ok(cards.into_iter().find(|c| {
        let id = match c {
            OnDeckCard::SeriesNext { issue, .. } | OnDeckCard::CblNext { issue, .. } => {
                issue.id.as_str()
            }
        };
        exclude_issue_id != Some(id)
    }))
}

// `pick_next_in_series` and `pick_next_in_cbl` live in `crate::api::next_up`
// so the new `/issues/{id}/next-up` resolver and this rail share one
// definition. See [`next_up::pick_next_in_series`] / [`next_up::pick_next_in_cbl`].

/// Return every distinct series id that any matched entry in this CBL
/// references. Used by [`compute_on_deck`] to suppress SeriesNext cards
/// for series that a surviving CBL card already represents. Joins
/// through `issues` rather than reading `cbl_entries.matched_series_id`
/// directly so the dedup works regardless of whether the matcher
/// populated the denormalised column (which has historically been
/// sometimes-set, sometimes-NULL depending on the match path).
/// Batched companion that pulls series-ids for *every* CBL in
/// `list_ids` in a single SQL round-trip. Returns
/// `HashMap<cbl_list_id, Vec<series_id>>`; CBLs with no matched
/// entries are absent from the map (callers should treat that as
/// "no owned series"). Avoids the per-iteration N+1 the old
/// `series_ids_in_cbl(one_id)` helper produced when called inside
/// `compute_on_deck`'s CBL loop (audit-remediation M5.2).
async fn series_ids_in_cbls(
    app: &AppState,
    list_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<Uuid>>, sea_orm::DbErr> {
    if list_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    #[derive(FromQueryResult)]
    struct Row {
        cbl_list_id: Uuid,
        series_id: Uuid,
    }
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT DISTINCT e.cbl_list_id AS cbl_list_id, i.series_id AS series_id \
         FROM cbl_entries e \
         JOIN issues i ON i.id = e.matched_issue_id \
         WHERE e.cbl_list_id = ANY($1)",
        [list_ids.to_vec().into()],
    ))
    .all(&app.db)
    .await?;
    let mut map: std::collections::HashMap<Uuid, Vec<Uuid>> =
        std::collections::HashMap::with_capacity(list_ids.len());
    for r in rows {
        map.entry(r.cbl_list_id).or_default().push(r.series_id);
    }
    Ok(map)
}

async fn cbl_saved_view_ids_for_candidates(
    app: &AppState,
    user_id: Uuid,
    list_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Uuid>, Response> {
    if list_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    use entity::saved_view;
    use sea_orm::Condition;

    let rows = match saved_view::Entity::find()
        .filter(saved_view::Column::Kind.eq("cbl"))
        .filter(saved_view::Column::CblListId.is_in(list_ids.to_vec()))
        .filter(
            Condition::any()
                .add(saved_view::Column::UserId.is_null())
                .add(saved_view::Column::UserId.eq(user_id)),
        )
        // user-owned rows (UserId IS NOT NULL) first; within tier, lowest id wins.
        .order_by_desc(Expr::cust("user_id IS NOT NULL"))
        .order_by_asc(saved_view::Column::Id)
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "rails: on-deck saved-view lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };

    let mut map = std::collections::HashMap::new();
    for sv in rows {
        if let Some(list_id) = sv.cbl_list_id {
            // First insert wins thanks to the ORDER BY tiebreak.
            map.entry(list_id).or_insert(sv.id);
        }
    }
    Ok(map)
}

/// Slim projection of `issue` carrying only the columns the On Deck walk
/// (`next_up::walk_*`) and the rendered `IssueSummaryView` card read —
/// 13 narrow columns instead of all 76. The full row averages ~1.9 KB
/// (the `pages` page-map JSON, `summary`, `characters`, `notes`,
/// `search_doc`, …), and On Deck hydrates *every* active issue across up
/// to 40 candidate series plus every matched CBL issue just to pick the
/// next-up issue per candidate and keep 24 cards. Projecting to the
/// fields actually used drops that transfer + deserialization by ~20-40x.
/// The card output is byte-for-byte identical to `IssueSummaryView::
/// from_model` (see `into_summary_view`).
#[derive(Clone, Debug, FromQueryResult, DerivePartialModel)]
#[sea_orm(entity = "issue::Entity")]
pub(crate) struct OnDeckIssue {
    pub id: String,
    pub slug: String,
    pub series_id: Uuid,
    pub library_id: Uuid,
    pub title: Option<String>,
    pub number_raw: Option<String>,
    pub sort_number: Option<f64>,
    pub year: Option<i32>,
    pub page_count: Option<i32>,
    pub state: String,
    pub special_type: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

impl crate::api::next_up::WalkIssue for OnDeckIssue {
    fn walk_id(&self) -> &str {
        &self.id
    }
    fn walk_series_id(&self) -> Uuid {
        self.series_id
    }
    fn walk_library_id(&self) -> Uuid {
        self.library_id
    }
}

impl OnDeckIssue {
    /// Build the card view from the slim row. Mirrors
    /// [`IssueSummaryView::from_model`] field-for-field (including the
    /// `cover_url` derivation) so the projection is invisible on the wire.
    fn into_summary_view(self, series_slug: &str) -> IssueSummaryView {
        let cover_url =
            (self.state == "active").then(|| format!("/issues/{}/pages/0/thumb", self.id));
        IssueSummaryView {
            id: self.id,
            slug: self.slug,
            series_id: self.series_id.to_string(),
            series_slug: series_slug.to_owned(),
            series_name: None,
            title: self.title,
            number: self.number_raw,
            sort_number: self.sort_number,
            year: self.year,
            page_count: self.page_count,
            state: self.state,
            cover_url,
            special_type: self.special_type,
            created_at: self.created_at.to_rfc3339(),
            updated_at: self.updated_at.to_rfc3339(),
        }
    }
}

/// Bulk-fetch the active, non-removed issues for many series in one query,
/// grouped by series id and ordered the way each series reads
/// (`sort_number IS NULL` last, then `sort_number`, then `id`) so a caller
/// can feed each group straight into `next_up::walk_series_continue_pick`.
/// Collapses the old per-series `pick_next_in_series_continue` round-trip
/// in `compute_on_deck` to a single query.
async fn hydrate_series_issues(
    app: &AppState,
    series_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<OnDeckIssue>>, Response> {
    use std::collections::HashMap;
    if series_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = match issue::Entity::find()
        .filter(issue::Column::SeriesId.is_in(series_ids.to_vec()))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .order_by_asc(issue::Column::SeriesId)
        .order_by_asc(Expr::cust("sort_number IS NULL"))
        .order_by_asc(issue::Column::SortNumber)
        .order_by_asc(issue::Column::Id)
        .into_partial_model::<OnDeckIssue>()
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "rails: on-deck series-issue hydrate failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    let mut map: HashMap<Uuid, Vec<OnDeckIssue>> = HashMap::new();
    for r in rows {
        map.entry(r.series_id).or_default().push(r);
    }
    Ok(map)
}

/// Bulk-fetch the caller's progress rows for a set of issue ids, keyed by
/// issue id. Shared by the On Deck series + CBL hydration paths.
async fn fetch_progress_for(
    app: &AppState,
    user_id: Uuid,
    issue_ids: &[String],
) -> Result<std::collections::HashMap<String, progress_record::Model>, Response> {
    use std::collections::HashMap;
    if issue_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = match progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user_id))
        .filter(progress_record::Column::IssueId.is_in(issue_ids.to_vec()))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "rails: on-deck progress hydrate failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    Ok(rows.into_iter().map(|p| (p.issue_id.clone(), p)).collect())
}

/// Maps returned by [`hydrate_cbl_picks`]: matched entries grouped by
/// list (ordered by position), all matched issues, their parent series,
/// and the caller's progress — everything `next_up::walk_cbl_pick` needs.
type CblPickHydration = (
    std::collections::HashMap<Uuid, Vec<cbl_entry::Model>>,
    std::collections::HashMap<String, OnDeckIssue>,
    std::collections::HashMap<Uuid, series::Model>,
    std::collections::HashMap<String, progress_record::Model>,
);

/// Bulk-hydrate everything needed to resolve the next-up pick for many
/// CBL lists at once: matched entries (ordered by position, grouped by
/// list), their issues + parent series, and the caller's progress.
/// Collapses the per-candidate `pick_next_in_cbl_frontier` round-trips in
/// `compute_on_deck` to a constant number of queries.
async fn hydrate_cbl_picks(
    app: &AppState,
    user_id: Uuid,
    list_ids: &[Uuid],
) -> Result<CblPickHydration, Response> {
    use std::collections::HashMap;
    if list_ids.is_empty() {
        return Ok((
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        ));
    }
    let entries = match cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.is_in(list_ids.to_vec()))
        .filter(cbl_entry::Column::MatchedIssueId.is_not_null())
        .order_by_asc(cbl_entry::Column::CblListId)
        .order_by_asc(cbl_entry::Column::Position)
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "rails: on-deck cbl-entry hydrate failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    let matched_ids: Vec<String> = entries
        .iter()
        .filter_map(|e| e.matched_issue_id.clone())
        .collect();
    let progress = fetch_progress_for(app, user_id, &matched_ids).await?;
    let issue_rows = match issue::Entity::find()
        .filter(issue::Column::Id.is_in(matched_ids))
        .into_partial_model::<OnDeckIssue>()
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "rails: on-deck cbl-issue hydrate failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    let issue_by_id: HashMap<String, OnDeckIssue> = issue_rows
        .iter()
        .map(|i| (i.id.clone(), i.clone()))
        .collect();
    let mut series_ids: Vec<Uuid> = issue_rows.iter().map(|i| i.series_id).collect();
    series_ids.sort();
    series_ids.dedup();
    let series_by_id: HashMap<Uuid, series::Model> = if series_ids.is_empty() {
        HashMap::new()
    } else {
        match series::Entity::find()
            .filter(series::Column::Id.is_in(series_ids))
            .all(&app.db)
            .await
        {
            Ok(v) => v.into_iter().map(|s| (s.id, s)).collect(),
            Err(e) => {
                tracing::error!(error = %e, "rails: on-deck cbl-series hydrate failed");
                return Err(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal",
                ));
            }
        }
    };
    let mut entries_by_list: HashMap<Uuid, Vec<cbl_entry::Model>> = HashMap::new();
    for e in entries {
        entries_by_list.entry(e.cbl_list_id).or_default().push(e);
    }
    Ok((entries_by_list, issue_by_id, series_by_id, progress))
}

#[utoipa::path(
    operation_id = "rails_create_dismissal",    post,
    path = "/me/rail-dismissals",
    request_body = CreateDismissalReq,
    responses((status = 204))
)]
#[handler]
pub async fn create_dismissal(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<CreateDismissalReq>,
) -> Response {
    let kind = req.target_kind.trim();
    if !is_valid_kind(kind) {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "target_kind must be one of: issue, series, cbl",
        );
    }
    let target_id = req.target_id.trim();
    if target_id.is_empty() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "target_id is required",
        );
    }
    // Validate the target actually exists and is visible to the user.
    // Bail with 404 (not 403) so we don't disclose existence of items
    // the user can't see.
    if let Err(resp) = ensure_target_visible(&app, &user, kind, target_id).await {
        return resp;
    }

    let now = Utc::now().fixed_offset();
    let am = rail_dismissal::ActiveModel {
        user_id: Set(user.id),
        target_kind: Set(kind.to_owned()),
        target_id: Set(target_id.to_owned()),
        dismissed_at: Set(now),
    };
    // Re-dismissing an already-dismissed target updates the timestamp
    // so the rail's "auto-restore on newer progress" comparison resets.
    let conn = &app.db;
    let already =
        rail_dismissal::Entity::find_by_id((user.id, kind.to_owned(), target_id.to_owned()))
            .one(conn)
            .await;
    match already {
        Ok(Some(_)) => {
            if let Err(e) = am.update(conn).await {
                tracing::error!(error = %e, "rails: dismissal update failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
        Ok(None) => {
            if let Err(e) = am.insert(conn).await {
                tracing::error!(error = %e, "rails: dismissal insert failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "rails: dismissal lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    }

    crate::audit::record(
        &app.db,
        crate::audit::AuditEntry {
            actor_id: user.id,
            action: "user.rail.dismiss",
            target_type: Some("rail_dismissal"),
            target_id: Some(format!("{kind}:{target_id}")),
            payload: serde_json::json!({ "target_kind": kind, "target_id": target_id }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    operation_id = "rails_delete_dismissal",    delete,
    path = "/me/rail-dismissals/{kind}/{target_id}",
    params(
        ("kind"      = String, Path,),
        ("target_id" = String, Path,),
    ),
    responses((status = 204))
)]
#[handler]
pub async fn delete_dismissal(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    AxPath((kind, target_id)): AxPath<(String, String)>,
) -> Response {
    if !is_valid_kind(&kind) {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "kind must be one of: issue, series, cbl",
        );
    }
    let res = rail_dismissal::Entity::delete_by_id((user.id, kind.clone(), target_id.clone()))
        .exec(&app.db)
        .await;
    match res {
        Ok(r) if r.rows_affected == 0 => {
            return error(
                StatusCode::NOT_FOUND,
                "not_found",
                "no dismissal for that target",
            );
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!(error = %e, "rails: dismissal delete failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    }
    crate::audit::record(
        &app.db,
        crate::audit::AuditEntry {
            actor_id: user.id,
            action: "user.rail.restore",
            target_type: Some("rail_dismissal"),
            target_id: Some(format!("{kind}:{target_id}")),
            payload: serde_json::json!({ "target_kind": kind, "target_id": target_id }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    StatusCode::NO_CONTENT.into_response()
}

// ───── Helpers ─────

fn is_valid_kind(kind: &str) -> bool {
    matches!(
        kind,
        DISMISS_KIND_ISSUE | DISMISS_KIND_SERIES | DISMISS_KIND_CBL
    )
}

/// 404 if the target doesn't exist or the user can't see it (library
/// ACL / list ownership), so we don't leak existence.
async fn ensure_target_visible(
    app: &AppState,
    user: &CurrentUser,
    kind: &str,
    target_id: &str,
) -> Result<(), Response> {
    let acl = access::for_user(app, user).await;
    match kind {
        DISMISS_KIND_ISSUE => {
            let row = issue::Entity::find_by_id(target_id.to_owned())
                .one(&app.db)
                .await
                .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"))?;
            let issue = row.ok_or_else(not_found)?;
            if !acl.contains(issue.library_id) {
                return Err(not_found());
            }
            Ok(())
        }
        DISMISS_KIND_SERIES => {
            let id = Uuid::parse_str(target_id).map_err(|_| {
                error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation",
                    "series target_id must be UUID",
                )
            })?;
            let row = series::Entity::find_by_id(id)
                .one(&app.db)
                .await
                .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"))?;
            let series = row.ok_or_else(not_found)?;
            if !acl.contains(series.library_id) {
                return Err(not_found());
            }
            Ok(())
        }
        DISMISS_KIND_CBL => {
            let id = Uuid::parse_str(target_id).map_err(|_| {
                error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation",
                    "cbl target_id must be UUID",
                )
            })?;
            let row = entity::cbl_list::Entity::find_by_id(id)
                .one(&app.db)
                .await
                .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"))?;
            let list = row.ok_or_else(not_found)?;
            // CBL lists are either user-owned or admin/global. Treat
            // visibility the same way `/me/cbl-lists` does — the user
            // can see their own + any admin-owned list.
            if let Some(owner) = list.owner_user_id
                && owner != user.id
            {
                return Err(not_found());
            }
            Ok(())
        }
        _ => Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "invalid kind",
        )),
    }
}
