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
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::{issue, rail_dismissal, series};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DbBackend, EntityTrait, FromQueryResult, QueryFilter, Set,
    Statement, sea_query::Expr,
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
        /// Saved-view id (kind=`cbl`) wrapping this CBL list, when the
        /// caller can see one. Web threads it onto the reader URL as
        /// `?cbl=<id>` so the next-up resolver keeps picking from the
        /// list across page turns. `None` when no saved view points at
        /// this `cbl_list_id` for the caller — the reader still works,
        /// just without persistent CBL context.
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
    let acl = access::for_user(&app, &user).await;
    let mut items = match compute_on_deck(&app, user.id, &acl).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    items.truncate(24);
    Json(OnDeckView { items }).into_response()
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

    for row in &series_candidates {
        if !acl.contains(row.library_id) {
            continue;
        }
        // `pick_next_in_series_continue` anchors on the user's latest
        // finished issue in the series, so finishing #20 surfaces #21
        // as on-deck instead of #1 (the bare "first unread anywhere"
        // pick used pre-v0.5.6). Falls back to the earliest-unread
        // pick when no finished issue exists, but the `started` CTE
        // above guarantees we always have at least one finished
        // issue here, so the fallback only matters for the helper's
        // other call sites.
        let next =
            match crate::api::next_up::pick_next_in_series_continue(app, user_id, row.series_id)
                .await
            {
                Ok(opt) => opt,
                Err(resp) => return Err(resp),
            };
        let Some(issue_model) = next else { continue };
        series_buf.push((
            row.last_activity,
            OnDeckCard::SeriesNext {
                issue: IssueSummaryView::from_model(issue_model, &row.series_slug)
                    .with_series_name(row.series_name.clone()),
                series_name: row.series_name.clone(),
                last_activity: row.last_activity.to_rfc3339(),
            },
        ));
    }

    // ───── cbl_next candidates ─────
    //
    // CBLs where the user has any matched-issue progress + at least one
    // matched entry that isn't yet finished. We pull the (cbl, last_activity,
    // next_entry_issue_id) tuple in one query per list for clarity; the
    // total candidate count is bounded by the user's actual CBL usage so
    // the round-trip count is small in practice.
    //
    // Pre-fetch the (cbl_list_id → saved_view_id) lookup for every
    // saved view of kind='cbl' the caller can see. Lets each CblNext
    // card carry the saved-view id so the web can thread `?cbl=` onto
    // the reader URL. Tiebreak: user-owned saved view wins over
    // system-owned (NULL user_id); within a tier, lowest id wins.
    let cbl_saved_view_by_list_id: std::collections::HashMap<Uuid, Uuid> = {
        use entity::saved_view;
        use sea_orm::{Condition, QueryOrder};
        let rows = match saved_view::Entity::find()
            .filter(saved_view::Column::Kind.eq("cbl"))
            .filter(saved_view::Column::CblListId.is_not_null())
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
        map
    };

    #[derive(Debug, FromQueryResult)]
    struct CblCandidate {
        cbl_list_id: Uuid,
        cbl_list_name: String,
        last_activity: chrono::DateTime<chrono::FixedOffset>,
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
            r#"
                SELECT cl.id AS cbl_list_id, cl.parsed_name AS cbl_list_name,
                       MAX(p.updated_at) AS last_activity
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

    // Series-wide ownership per CBL — pre-fetched in one round-trip
    // across every candidate (audit-remediation M5.2). Over-fetching
    // for CBLs that turn out to have no next is cheap compared to the
    // per-iteration query the old `series_ids_in_cbl(one)` helper
    // produced. See comment on `cbl_owned_series_ids` above for why we
    // shadow every series in the CBL, not just the currently-pointed
    // issue.
    let cbl_series_ownership: std::collections::HashMap<Uuid, Vec<Uuid>> = {
        let ids: Vec<Uuid> = cbl_candidates.iter().map(|c| c.cbl_list_id).collect();
        match series_ids_in_cbls(app, &ids).await {
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

    for cand in &cbl_candidates {
        let next =
            match crate::api::next_up::pick_next_in_cbl(app, user_id, cand.cbl_list_id, acl, None)
                .await
            {
                Ok(opt) => opt,
                Err(resp) => return Err(resp),
            };
        let Some((issue_model, series_slug, series_name, position)) = next else {
            continue;
        };
        if let Some(series_ids) = cbl_series_ownership.get(&cand.cbl_list_id) {
            cbl_owned_series_ids.extend(series_ids.iter().copied());
        }
        let cbl_saved_view_id = cbl_saved_view_by_list_id
            .get(&cand.cbl_list_id)
            .map(|id| id.to_string());
        items.push((
            cand.last_activity,
            OnDeckCard::CblNext {
                issue: IssueSummaryView::from_model(issue_model, &series_slug)
                    .with_series_name(series_name),
                cbl_list_id: cand.cbl_list_id.to_string(),
                cbl_list_name: cand.cbl_list_name.clone(),
                cbl_saved_view_id,
                position,
                last_activity: cand.last_activity.to_rfc3339(),
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

    // Merge by most-recent activity desc; caller caps to per-surface
    // limit (the rail truncates to 24, the next-up resolver takes 1).
    items.sort_by(|a, b| b.0.cmp(&a.0));
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
