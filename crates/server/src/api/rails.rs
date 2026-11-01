//! Home-page system rails: Continue Reading + On Deck + New Issues +
//! per-user dismissals.
//!
//! Endpoints:
//!   - `GET    /me/continue-reading`               — partially-read issues
//!   - `GET    /me/on-deck`                        — (M3) — next-up issues + CBLs
//!   - `GET    /me/recent-issues`                  — newest ingests, per-series-capped
//!   - `GET    /me/recent-history`                 — every issue with recent activity
//!   - `GET    /me/random-picks`                   — daily shuffle of unstarted series
//!   - `GET    /me/on-this-day`                    — issues released on today's date
//!   - `GET    /me/rail-dismissals`                — the caller's hidden items
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
    extract::{Path as AxPath, Query, State},
    http::{HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{Datelike, Utc};
use entity::{cbl_entry, issue, progress_record, rail_dismissal, series};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DbBackend, EntityTrait, FromQueryResult,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, QueryTrait, Set, Statement,
    sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use shared::pagination::{decode_cursor, encode_cursor};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::{error, not_found};
use crate::api::issue_card::IssueCardRow;
use crate::api::series::{IssueSummaryView, SeriesView, hydrate_series};
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
        .routes(routes!(recent_issues))
        .routes(routes!(recent_history))
        .routes(routes!(random_picks))
        .routes(routes!(on_this_day))
        .routes(routes!(create_dismissal, list_dismissals))
        .routes(routes!(delete_dismissal))
}

/// Cap on how many issues one series may contribute to the New Issues
/// rail. Pure reverse-chronological, a single bulk import (say a
/// 200-issue series) fills every rail slot with one series; the cap
/// keeps the rail a digest of *what* arrived rather than a wall of one
/// title. The uncapped list stays reachable — the rail's "View all"
/// page walks `GET /issues?sort=created_at&order=desc`.
const RECENT_ISSUES_PER_SERIES_CAP: i32 = 3;
/// Rail preview size; mirrors the web's `RAIL_PREVIEW_LIMIT`.
const RECENT_ISSUES_LIMIT: i32 = 30;
/// Random-picks rail size (filter-parity-and-builtin-views-1.0 M6).
const RANDOM_PICKS_LIMIT: i64 = 12;
/// On-this-day rail size.
const ON_THIS_DAY_LIMIT: i64 = 30;

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

/// Recent-history rail (filter-parity-and-builtin-views-1.0 M6): every
/// issue the caller touched recently, finished or not — the reading log
/// as a rail. Same card as Continue reading.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RecentHistoryView {
    pub items: Vec<ContinueReadingCard>,
}

/// Random-picks rail: a daily shuffle of series the caller hasn't
/// started. `seed_date` is the UTC day the order is stable for.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RandomPicksView {
    pub seed_date: String,
    pub items: Vec<SeriesView>,
}

/// On-this-day rail: issues released on this calendar day (any year),
/// oldest first.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OnThisDayView {
    pub month: u32,
    pub day: u32,
    pub items: Vec<IssueSummaryView>,
}

/// `?month=&day=` for the on-this-day rail — the client sends its local
/// calendar day; the server's UTC date is the fallback.
#[derive(Debug, Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
pub struct OnThisDayQuery {
    pub month: Option<u32>,
    pub day: Option<u32>,
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

/// Newest-ingests rail. Items are plain issue cards (`series_name`
/// populated so the card heading reads "Series #N" across a rail that
/// mixes many series).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RecentIssuesView {
    pub items: Vec<IssueSummaryView>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateDismissalReq {
    /// One of `'issue'`, `'series'`, `'cbl'`.
    pub target_kind: String,
    pub target_id: String,
}

/// Kind of a hidden rail target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RailDismissalKind {
    Issue,
    Series,
    Cbl,
}

impl RailDismissalKind {
    fn from_db(kind: &str) -> Option<Self> {
        match kind {
            DISMISS_KIND_ISSUE => Some(Self::Issue),
            DISMISS_KIND_SERIES => Some(Self::Series),
            DISMISS_KIND_CBL => Some(Self::Cbl),
            _ => None,
        }
    }

    /// The rails a dismissal of this kind hides its target from. The row
    /// does not record which rail the click came from, so this is the
    /// full set the rail queries consult for the kind.
    fn hidden_from(self) -> Vec<DismissalRail> {
        match self {
            Self::Issue => vec![
                DismissalRail::ContinueReading,
                DismissalRail::RecentHistory,
                DismissalRail::OpdsContinueReading,
            ],
            Self::Series => vec![
                DismissalRail::OnDeck,
                DismissalRail::RandomPicks,
                DismissalRail::SimilarSeries,
            ],
            Self::Cbl => vec![DismissalRail::OnDeck],
        }
    }
}

/// A surface that honours dismissals: the home rails, the series page's
/// Similar series rail, and the OPDS Continue reading feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DismissalRail {
    ContinueReading,
    RecentHistory,
    OnDeck,
    RandomPicks,
    SimilarSeries,
    OpdsContinueReading,
}

/// The hydrated target of a dismissal. Which link fields are set depends
/// on the kind: `slug` + `series_slug` for an issue, `slug` for a series,
/// `saved_view_id` for a CBL reading list (absent when no saved view
/// wraps the list).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RailDismissalTargetView {
    /// Display name: `"<series> #<number>"` for an issue, the series name,
    /// or the reading-list name.
    pub name: String,
    /// Secondary line: the issue title, or the series start year.
    pub subtitle: Option<String>,
    /// Issue slug (issue) or series slug (series).
    pub slug: Option<String>,
    /// Parent series slug (issue only).
    pub series_slug: Option<String>,
    /// Saved-view id (kind `cbl`) wrapping the list (cbl only).
    pub saved_view_id: Option<String>,
    /// Cover thumbnail URL; `None` for reading lists and coverless series.
    pub cover_url: Option<String>,
}

/// One hidden rail item.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RailDismissalView {
    /// Stable id of the dismissal, `"<kind>:<target_id>"`.
    pub id: String,
    pub target_kind: RailDismissalKind,
    pub target_id: String,
    /// The rails this dismissal hides the target from.
    pub hidden_from: Vec<DismissalRail>,
    /// RFC 3339 timestamp of the (latest) dismissal.
    pub dismissed_at: String,
    pub target: RailDismissalTargetView,
}

/// `GET /me/rail-dismissals` page. The `CursorPage` shape plus one
/// first-page count: dismissals whose target the caller can no longer
/// see (soft-removed, outside the library grant, over the age cap, or
/// deleted) are never listed, and `unavailable` says how many there are.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RailDismissalListView {
    pub items: Vec<RailDismissalView>,
    pub next_cursor: Option<String>,
    /// Listed dismissals in total; first page only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// Dismissals withheld because their target is not visible; first
    /// page only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<u64>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListDismissalsQuery {
    /// Opaque cursor from a previous page's `next_cursor`.
    pub cursor: Option<String>,
    /// Page size; default 50, max 200.
    pub limit: Option<u64>,
}

const DISMISSALS_DEFAULT_LIMIT: u64 = 50;
const DISMISSALS_MAX_LIMIT: u64 = 200;

/// Keyset position in the newest-first dismissal order.
#[derive(Debug, Serialize, Deserialize)]
struct DismissalCursor {
    at: chrono::DateTime<chrono::FixedOffset>,
    kind: String,
    id: String,
}

// ───── Handlers ─────

/// Library-ACL clause for a raw rail query. Pushes the allowed-library
/// list onto `params` and binds it as `$<params.len()>` — the same
/// convention `VisibleLibraries::raw_cap_clause` uses, so the two can be
/// chained on one parameter vector that already holds the fixed binds.
/// `None` when the caller can see no library at all (the rail is empty;
/// skip the query). Unrestricted callers get an empty clause.
/// Post-audit T3.5: the clause runs inside the query so the LIMIT applies
/// to rows the caller can actually see.
fn rail_acl_clause(
    acl: &access::VisibleLibraries,
    library_expr: &str,
    params: &mut Vec<sea_orm::Value>,
) -> Option<String> {
    if acl.unrestricted {
        return Some(String::new());
    }
    if acl.allowed.is_empty() {
        return None;
    }
    let allowed: Vec<Uuid> = acl.allowed.iter().copied().collect();
    params.push(allowed.into());
    Some(format!(" AND {library_expr} = ANY(${})", params.len()))
}

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
    match progress_cards(
        &app,
        &user,
        "AND p.finished = false AND p.last_page > 0",
        "continue-reading",
    )
    .await
    {
        Ok(items) => Json(ContinueReadingView { items }).into_response(),
        Err(resp) => resp,
    }
}

/// Everything the caller touched recently — in progress *or* finished —
/// most recent activity first. Bulk mark-read backfills are excluded
/// (`is_backfill`) so marking a 200-issue run read doesn't bury the
/// issues actually read. Dismissals follow the Continue-reading rule.
#[utoipa::path(
    operation_id = "rails_recent_history",    get,
    path = "/me/recent-history",
    responses((status = 200, body = RecentHistoryView))
)]
#[handler]
pub async fn recent_history(State(app): State<AppState>, user: CurrentUser) -> Response {
    match progress_cards(
        &app,
        &user,
        "AND (p.finished = true OR p.last_page > 0) AND p.is_backfill = false",
        "recent-history",
    )
    .await
    {
        Ok(items) => Json(RecentHistoryView { items }).into_response(),
        Err(resp) => resp,
    }
}

/// Shared body of the progress-backed rails: the caller's
/// `progress_records` joined to active, visible issues, minus current
/// dismissals, newest activity first, hydrated to `ContinueReadingCard`s.
/// `extra_where` narrows the progress rows (static SQL, no user input).
async fn progress_cards(
    app: &AppState,
    user: &CurrentUser,
    extra_where: &'static str,
    rail: &'static str,
) -> Result<Vec<ContinueReadingCard>, Response> {
    let acl = access::for_user(app, user).await;

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
    // progress overlay + the parent series slug in one query. Library
    // ACL + age cap run *inside* the query, before the LIMIT (post-audit
    // T3.5): filtering the 24 rows afterwards in Rust let a user with
    // many records in libraries they cannot see end up with an empty
    // rail while their visible in-progress issues sat past the cut.
    let mut params: Vec<sea_orm::Value> = vec![user.id.into()];
    let Some(acl_clause) = rail_acl_clause(&acl, "i.library_id", &mut params) else {
        return Ok(Vec::new());
    };
    let cap_clause = acl.raw_cap_clause(
        "i.library_id",
        "COALESCE(i.age_rating, s.age_rating)",
        &mut params,
    );
    let sql = format!(
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
              {extra_where}
              AND i.state     = 'active'
              AND i.removed_at IS NULL
              {acl_clause}{cap_clause}
              AND (d.dismissed_at IS NULL OR p.updated_at > d.dismissed_at)
            ORDER BY p.updated_at DESC
            LIMIT 24
        "#
    );
    let rows: Vec<Row> = match Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        params,
    ))
    .all(&app.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, rail, "rails: progress rail query failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    // Hydrate the slim card projection (WP-3.6) for the page. One batched
    // fetch keeps it O(1) round-trips even for the full 24.
    let issue_ids: Vec<String> = rows.iter().map(|r| r.issue_id.clone()).collect();
    let issue_rows = match issue::Entity::find()
        .filter(issue::Column::Id.is_in(issue_ids.clone()))
        .into_partial_model::<OnDeckIssue>()
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, rail, "rails: progress rail hydrate failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    // WP-2.7: drops issues rated above the caller's cap (one extra
    // series-rating query, only for capped callers).
    let issue_rows = access::filter_issues(app, &acl, issue_rows).await;
    let issue_by_id: std::collections::HashMap<String, OnDeckIssue> =
        issue_rows.into_iter().map(|i| (i.id.clone(), i)).collect();

    let items: Vec<ContinueReadingCard> = rows
        .into_iter()
        .filter_map(|row| {
            if !acl.contains(row.library_id) {
                return None;
            }
            let issue_model = issue_by_id.get(&row.issue_id)?.clone();
            Some(ContinueReadingCard {
                issue: issue_model
                    .into_summary_view(&row.series_slug)
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

    Ok(items)
}

/// A daily shuffle of series the caller hasn't started (no progress row
/// on any issue), with at least one active issue, that they can see and
/// haven't dismissed. The order is `md5(series.id || seed)` with a
/// per-user, per-UTC-day seed, so the rail is stable for a day and
/// different for every user. Dismissals (`series` kind) simply hide a
/// series — there is no activity to auto-restore on.
#[utoipa::path(
    operation_id = "rails_random_picks",    get,
    path = "/me/random-picks",
    responses((status = 200, body = RandomPicksView))
)]
#[handler]
pub async fn random_picks(State(app): State<AppState>, user: CurrentUser) -> Response {
    let acl = access::for_user(&app, &user).await;
    let seed_date = Utc::now().date_naive().to_string();
    let seed = format!("{}:{seed_date}", user.id);
    let mut params: Vec<sea_orm::Value> = vec![user.id.into(), seed.into()];
    let Some(acl_clause) = rail_acl_clause(&acl, "s.library_id", &mut params) else {
        return Json(RandomPicksView {
            seed_date,
            items: Vec::new(),
        })
        .into_response();
    };
    let cap_clause = acl.raw_cap_clause("s.library_id", "s.age_rating", &mut params);
    params.push(RANDOM_PICKS_LIMIT.into());
    let limit_param = params.len();
    let sql = format!(
        r#"
            SELECT s.*
              FROM series s
             WHERE s.removed_at IS NULL
               AND EXISTS (SELECT 1 FROM issues i
                            WHERE i.series_id = s.id
                              AND i.state = 'active' AND i.removed_at IS NULL)
               AND NOT EXISTS (SELECT 1 FROM progress_records p
                                 JOIN issues pi ON pi.id = p.issue_id
                                WHERE p.user_id = $1 AND pi.series_id = s.id)
               AND NOT EXISTS (SELECT 1 FROM rail_dismissals d
                                WHERE d.user_id = $1
                                  AND d.target_kind = 'series'
                                  AND d.target_id = s.id::text)
               {acl_clause}{cap_clause}
             ORDER BY md5(s.id::text || $2)
             LIMIT ${limit_param}
        "#
    );
    let rows = match series::Model::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        params,
    ))
    .all(&app.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "rails: random-picks query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let items = crate::api::series::hydrate_series(&app, rows, user.id).await;
    Json(RandomPicksView { seed_date, items }).into_response()
}

/// Issues released on this calendar day in any year (`issues.month` /
/// `issues.day`), oldest first. Only issues with a full release date
/// qualify, so libraries tagged with years alone leave the rail empty
/// (it hides itself). The client passes its local `month` / `day`;
/// absent, the server's UTC date applies.
#[utoipa::path(
    operation_id = "rails_on_this_day",    get,
    path = "/me/on-this-day",
    params(OnThisDayQuery),
    responses((status = 200, body = OnThisDayView), (status = 422, description = "bad date"))
)]
#[handler]
pub async fn on_this_day(
    State(app): State<AppState>,
    user: CurrentUser,
    axum::extract::Query(q): axum::extract::Query<OnThisDayQuery>,
) -> Response {
    let today = Utc::now().date_naive();
    let month = q.month.unwrap_or(today.month());
    let day = q.day.unwrap_or(today.day());
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "month must be 1..=12 and day 1..=31",
        );
    }
    let acl = access::for_user(&app, &user).await;

    #[derive(Debug, FromQueryResult)]
    struct Row {
        id: String,
        slug: String,
        series_id: Uuid,
        title: Option<String>,
        number_raw: Option<String>,
        sort_number: Option<f64>,
        year: Option<i32>,
        page_count: Option<i32>,
        special_type: Option<String>,
        created_at: chrono::DateTime<chrono::FixedOffset>,
        updated_at: chrono::DateTime<chrono::FixedOffset>,
        series_slug: String,
        series_name: String,
    }

    let mut params: Vec<sea_orm::Value> = vec![(month as i32).into(), (day as i32).into()];
    let Some(acl_clause) = rail_acl_clause(&acl, "i.library_id", &mut params) else {
        return Json(OnThisDayView {
            month,
            day,
            items: Vec::new(),
        })
        .into_response();
    };
    let cap_clause = acl.raw_cap_clause(
        "i.library_id",
        "COALESCE(i.age_rating, s.age_rating)",
        &mut params,
    );
    params.push(ON_THIS_DAY_LIMIT.into());
    let limit_param = params.len();
    let sql = format!(
        r#"
            SELECT i.id, i.slug, i.series_id, i.title, i.number_raw,
                   i.sort_number, i.year, i.page_count, i.special_type,
                   i.created_at, i.updated_at,
                   s.slug AS series_slug, s.name AS series_name
              FROM issues i
              JOIN series s ON s.id = i.series_id
             WHERE i.state = 'active'
               AND i.removed_at IS NULL
               AND i.month = $1 AND i.day = $2
               {acl_clause}{cap_clause}
             ORDER BY i.year ASC NULLS LAST, s.name ASC, i.sort_number ASC NULLS LAST, i.id ASC
             LIMIT ${limit_param}
        "#
    );
    let rows: Vec<Row> = match Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        params,
    ))
    .all(&app.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "rails: on-this-day query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let items: Vec<IssueSummaryView> = rows
        .into_iter()
        .map(|r| IssueSummaryView {
            cover_url: Some(format!("/issues/{}/pages/0/thumb", r.id)),
            id: r.id,
            slug: r.slug,
            series_id: r.series_id.to_string(),
            series_slug: r.series_slug,
            series_name: Some(r.series_name),
            title: r.title,
            number: r.number_raw,
            sort_number: r.sort_number,
            year: r.year,
            page_count: r.page_count,
            state: "active".to_owned(),
            special_type: r.special_type,
            created_at: r.created_at.to_rfc3339(),
            updated_at: r.updated_at.to_rfc3339(),
        })
        .collect();
    Json(OnThisDayView { month, day, items }).into_response()
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

/// Newest issues across every library the caller can see, ordered by
/// ingest time (`issues.created_at`) descending — the issue-level
/// complement to the series-level "Recently added series" filter rail.
/// At most [`RECENT_ISSUES_PER_SERIES_CAP`] issues per series survive
/// (scan-flood guard); the rail's detail page shows the uncapped list
/// via `GET /issues?sort=created_at&order=desc`.
///
/// The ACL lives in the SQL (unlike continue-reading's post-filter):
/// with a window this small, filtering after the LIMIT would let rows
/// from invisible libraries starve out the visible ones.
#[utoipa::path(
    operation_id = "rails_recent_issues",    get,
    path = "/me/recent-issues",
    responses((status = 200, body = RecentIssuesView))
)]
#[handler]
pub async fn recent_issues(State(app): State<AppState>, user: CurrentUser) -> Response {
    let acl = access::for_user(&app, &user).await;

    #[derive(Debug, FromQueryResult)]
    struct Row {
        id: String,
        slug: String,
        series_id: Uuid,
        title: Option<String>,
        number_raw: Option<String>,
        sort_number: Option<f64>,
        year: Option<i32>,
        page_count: Option<i32>,
        special_type: Option<String>,
        created_at: chrono::DateTime<chrono::FixedOffset>,
        updated_at: chrono::DateTime<chrono::FixedOffset>,
        series_slug: String,
        series_name: String,
    }

    // Newest `LIMIT` issues overall, at most `CAP` per series. Columns are
    // projected to what the card renders (see `OnDeckIssue` for the
    // rationale — full issue rows average ~1.9 KB).
    //
    // WP-3.6: this used to rank *every* active issue with
    // `ROW_NUMBER() OVER (PARTITION BY series_id ...)` and keep rank ≤ CAP —
    // a full scan + sort of the library on every home-page load. Walking
    // the active set newest-first down `issues_active_created_idx` in
    // keyset batches and counting per series in Rust yields exactly the
    // same rows (a series' issues appear in the global walk in the same
    // `created_at DESC, id DESC` order the window ranked them by), and
    // normally stops after the first batch. A scan-flood of one series
    // costs extra batches, bounded by the old full scan.
    let acl_clause = if acl.unrestricted {
        String::new()
    } else if acl.allowed.is_empty() {
        return Json(RecentIssuesView { items: Vec::new() }).into_response();
    } else {
        "AND i.library_id = ANY($1)".to_owned()
    };
    let mut base_values: Vec<sea_orm::Value> = Vec::new();
    if !acl.unrestricted {
        let allowed: Vec<Uuid> = acl.allowed.iter().copied().collect();
        base_values.push(allowed.into());
    }
    // WP-2.7: age-rating cap on the issue's own rating, series fallback.
    let cap_clause = acl.raw_cap_clause(
        "i.library_id",
        "COALESCE(i.age_rating, s.age_rating)",
        &mut base_values,
    );
    const BATCH: usize = 128;
    let cap = RECENT_ISSUES_PER_SERIES_CAP as usize;
    let limit = RECENT_ISSUES_LIMIT as usize;
    let mut per_series: std::collections::HashMap<Uuid, usize> = std::collections::HashMap::new();
    let mut rows: Vec<Row> = Vec::with_capacity(limit);
    let mut after: Option<(chrono::DateTime<chrono::FixedOffset>, String)> = None;
    'walk: loop {
        let mut values = base_values.clone();
        let keyset = match &after {
            Some((ts, id)) => {
                values.push((*ts).into());
                values.push(id.clone().into());
                format!(
                    "AND (i.created_at, i.id) < (${}, ${})",
                    values.len() - 1,
                    values.len()
                )
            }
            None => String::new(),
        };
        values.push((BATCH as i64).into());
        let limit_param = values.len();
        let sql = format!(
            r#"
            SELECT i.id, i.slug, i.series_id, i.title, i.number_raw,
                   i.sort_number, i.year, i.page_count, i.special_type,
                   i.created_at, i.updated_at,
                   s.slug AS series_slug, s.name AS series_name
            FROM issues i
            JOIN series s ON s.id = i.series_id
            WHERE i.state = 'active'
              AND i.removed_at IS NULL
              {acl_clause}{cap_clause}
              {keyset}
            ORDER BY i.created_at DESC, i.id DESC
            LIMIT ${limit_param}
            "#
        );
        let batch: Vec<Row> = match Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .all(&app.db)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "rails: recent-issues query failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
        let exhausted = batch.len() < BATCH;
        if let Some(last) = batch.last() {
            after = Some((last.created_at, last.id.clone()));
        }
        for row in batch {
            let seen = per_series.entry(row.series_id).or_insert(0);
            if *seen >= cap {
                continue;
            }
            *seen += 1;
            rows.push(row);
            if rows.len() >= limit {
                break 'walk;
            }
        }
        if exhausted {
            break;
        }
    }

    let items: Vec<IssueSummaryView> = rows
        .into_iter()
        .map(|r| {
            // `state` is pinned 'active' by the WHERE, so the cover URL is
            // unconditional (cf. `OnDeckIssue::into_summary_view`).
            let cover_url = Some(format!("/issues/{}/pages/0/thumb", r.id));
            IssueSummaryView {
                id: r.id,
                slug: r.slug,
                series_id: r.series_id.to_string(),
                series_slug: r.series_slug,
                series_name: Some(r.series_name),
                title: r.title,
                number: r.number_raw,
                sort_number: r.sort_number,
                year: r.year,
                page_count: r.page_count,
                state: "active".to_owned(),
                cover_url,
                special_type: r.special_type,
                created_at: r.created_at.to_rfc3339(),
                updated_at: r.updated_at.to_rfc3339(),
            }
        })
        .collect();

    Json(RecentIssuesView { items }).into_response()
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
        series_age_rating: Option<String>,
    }
    // ACL + age cap before the LIMIT (post-audit T3.5) — see
    // `continue_reading`. The `series_ok` re-check below stays as a
    // belt-and-braces guard; it no longer decides which rows survive.
    let mut series_params: Vec<sea_orm::Value> = vec![user_id.into()];
    let Some(acl_clause) = rail_acl_clause(acl, "s.library_id", &mut series_params) else {
        return Ok(Vec::new());
    };
    let cap_clause = acl.raw_cap_clause("s.library_id", "s.age_rating", &mut series_params);
    let series_sql = format!(
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
                       s.library_id AS library_id,
                       s.age_rating AS series_age_rating
                FROM started
                JOIN series s ON s.id = started.series_id
                LEFT JOIN rail_dismissals d
                  ON d.user_id = $1
                 AND d.target_kind = 'series'
                 AND d.target_id = s.id::text
                WHERE started.series_id NOT IN (SELECT series_id FROM in_progress)
                  {acl_clause}{cap_clause}
                  AND (d.dismissed_at IS NULL OR started.last_activity > d.dismissed_at)
                ORDER BY started.last_activity DESC
                LIMIT 40
            "#
    );
    let series_candidates: Vec<SeriesRow> = match SeriesRow::find_by_statement(
        Statement::from_sql_and_values(DbBackend::Postgres, series_sql, series_params),
    )
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
        .filter(|r| acl.series_ok(r.library_id, r.series_age_rating.as_deref()))
        .collect();
    let series_ids: Vec<Uuid> = visible_series.iter().map(|r| r.series_id).collect();
    let issues_by_series = match hydrate_series_issues(app, &series_ids, acl).await {
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
    // Deliberately NOT ACL-filtered here (post-audit T3.5 left this one
    // alone): a finished prefix entry in a library the user can no longer
    // see still counts toward candidacy and frontier activity — the ACL
    // applies to the *pick* (`hydrate_cbl_picks`), consistent with
    // `on_deck_cbl_acl_invisible_finished_prefix_still_counts`.
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
        .filter(saved_view::Column::Kind.eq(saved_view::SavedViewKind::Cbl))
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

/// Slim projection of `issue` the On Deck walk (`next_up::walk_*`) and the
/// rendered `IssueSummaryView` card read. On Deck hydrates *every* active
/// issue across up to 40 candidate series plus every matched CBL issue just
/// to pick the next-up issue per candidate, so projecting to the 14 card
/// columns (instead of all ~76, ~1.9 KB/row) drops transfer +
/// deserialization by ~20-40x. Shared with every other issue-card list path
/// since WP-3.6 — see [`crate::api::issue_card`].
pub(crate) type OnDeckIssue = crate::api::issue_card::IssueCardRow;

/// Bulk-fetch the active, non-removed issues for many series in one query,
/// grouped by series id and ordered the way each series reads
/// (`sort_number IS NULL` last, then `sort_number`, then `id`) so a caller
/// can feed each group straight into `next_up::walk_series_continue_pick`.
/// Collapses the old per-series `pick_next_in_series_continue` round-trip
/// in `compute_on_deck` to a single query.
async fn hydrate_series_issues(
    app: &AppState,
    series_ids: &[Uuid],
    acl: &access::VisibleLibraries,
) -> Result<std::collections::HashMap<Uuid, Vec<OnDeckIssue>>, Response> {
    use std::collections::HashMap;
    if series_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut sel = issue::Entity::find()
        .filter(issue::Column::SeriesId.is_in(series_ids.to_vec()))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null());
    // WP-2.7: an issue rated above the cap is never the "next" pick.
    if let Some(cap) = acl.issue_cap_condition() {
        sel = sel.filter(cap);
    }
    let rows = match sel
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

/// The caller's hidden rail items, newest dismissal first, each with its
/// target hydrated for display. Only the caller's own rows, and only those
/// whose target the caller can currently see (the same live + grant + cap
/// selects collections use); the rest are counted in `unavailable`.
#[utoipa::path(
    operation_id = "rails_list_dismissals",    get,
    path = "/me/rail-dismissals",
    params(ListDismissalsQuery),
    responses(
        (status = 200, body = RailDismissalListView),
        (status = 400, description = "invalid cursor"),
    )
)]
#[handler]
pub async fn list_dismissals(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListDismissalsQuery>,
) -> Response {
    let after = match q.cursor.as_deref() {
        Some(c) => match decode_cursor::<DismissalCursor>(c) {
            Ok(k) => Some(k),
            Err(_) => return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor"),
        },
        None => None,
    };
    let limit = q
        .limit
        .unwrap_or(DISMISSALS_DEFAULT_LIMIT)
        .clamp(1, DISMISSALS_MAX_LIMIT);
    let acl = access::for_user(&app, &user).await;
    let visible = visible_dismissal_condition(&acl, user.id);
    let internal = |e: sea_orm::DbErr| {
        tracing::error!(error = %e, "rails: dismissal list failed");
        error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
    };

    // Rows the rails still honour; an auto-restored row is neither listed
    // nor counted.
    let hiding = || {
        rail_dismissal::Entity::find()
            .filter(rail_dismissal::Column::UserId.eq(user.id))
            .filter(still_hiding_condition())
    };
    let (total, unavailable) = if after.is_none() {
        let (all, listed) = tokio::join!(
            hiding().count(&app.db),
            hiding().filter(visible.clone()).count(&app.db),
        );
        match (all, listed) {
            (Ok(all), Ok(listed)) => (Some(listed), Some(all.saturating_sub(listed))),
            (Err(e), _) | (_, Err(e)) => return internal(e),
        }
    } else {
        (None, None)
    };

    let mut select = hiding().filter(visible);
    if let Some(k) = &after {
        select = select.filter(Expr::cust_with_values(
            "(rail_dismissals.dismissed_at, rail_dismissals.target_kind, rail_dismissals.target_id) \
             < ($1, $2, $3)",
            [
                sea_orm::Value::from(k.at),
                sea_orm::Value::from(k.kind.clone()),
                sea_orm::Value::from(k.id.clone()),
            ],
        ));
    }
    let mut rows = match select
        .order_by_desc(rail_dismissal::Column::DismissedAt)
        .order_by_desc(rail_dismissal::Column::TargetKind)
        .order_by_desc(rail_dismissal::Column::TargetId)
        .limit(limit + 1)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    let has_more = rows.len() as u64 > limit;
    rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let next_cursor = if has_more {
        rows.last().and_then(|r| {
            encode_cursor(&DismissalCursor {
                at: r.dismissed_at,
                kind: r.target_kind.clone(),
                id: r.target_id.clone(),
            })
            .ok()
        })
    } else {
        None
    };

    let items = match hydrate_dismissals(&app, &acl, user.id, rows).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    Json(RailDismissalListView {
        items,
        next_cursor,
        total,
        unavailable,
    })
    .into_response()
}

/// `rail_dismissals` rows the rails still honour. A dismissal stops hiding
/// its target once the caller has newer progress on it — on the issue
/// (Continue reading / Recently read) or on any issue of the series (On
/// Deck, Similar series; Random picks only lists unstarted series) — so
/// such a row is not "hidden" any more. CBL rows stay: On Deck's restore
/// rule for them reads the list's reading frontier, which the walk computes
/// in Rust, and restoring one that already came back is harmless.
fn still_hiding_condition() -> Condition {
    Condition::all()
        .add(Expr::cust(
            r#"NOT ("rail_dismissals"."target_kind" = 'issue' AND EXISTS (
                SELECT 1 FROM progress_records p
                 WHERE p.user_id = "rail_dismissals"."user_id"
                   AND p.issue_id = "rail_dismissals"."target_id"
                   AND p.updated_at > "rail_dismissals"."dismissed_at"))"#,
        ))
        .add(Expr::cust(
            r#"NOT ("rail_dismissals"."target_kind" = 'series' AND EXISTS (
                SELECT 1 FROM progress_records p
                  JOIN issues i ON i.id = p.issue_id
                 WHERE p.user_id = "rail_dismissals"."user_id"
                   AND i.series_id = CASE WHEN "rail_dismissals"."target_kind" = 'series'
                                          THEN "rail_dismissals"."target_id"::uuid END
                   AND p.updated_at > "rail_dismissals"."dismissed_at"))"#,
        ))
}

/// `rail_dismissals` rows whose target the caller can currently see: an
/// active issue through `access::visible_issues_select` (not removed, live
/// parent series, inside the grant, under the cap), a series through
/// `access::visible_series_select`, or a CBL list the caller owns or that
/// is global (the rule `ensure_target_visible` applies on create). Each
/// leg is a correlated `EXISTS` keyed on the row's `target_id`; the uuid
/// legs cast `target_id` (create validates it as a UUID for those kinds)
/// rather than the key, so the primary-key index serves the probe, and the
/// `CASE` keeps the cast off an issue row's hex id.
fn visible_dismissal_condition(acl: &access::VisibleLibraries, user_id: Uuid) -> Condition {
    use entity::cbl_list;
    // Scoped: at module level `ExprTrait::max` shadows `Ord::max` on ints.
    use sea_orm::sea_query::ExprTrait;
    let issue_hit = access::visible_issues_select(acl)
        .select_only()
        .column(issue::Column::Id)
        .filter(issue::Column::State.eq("active"))
        .filter(
            Expr::col((issue::Entity, issue::Column::Id))
                .equals((rail_dismissal::Entity, rail_dismissal::Column::TargetId)),
        )
        .into_query();
    let series_hit = access::visible_series_select(acl)
        .select_only()
        .column(series::Column::Id)
        .filter(Expr::cust(
            r#""series"."id" = CASE WHEN "rail_dismissals"."target_kind" = 'series'
                                     THEN "rail_dismissals"."target_id"::uuid END"#,
        ))
        .into_query();
    let cbl_hit = cbl_list::Entity::find()
        .select_only()
        .column(cbl_list::Column::Id)
        .filter(
            Condition::any()
                .add(cbl_list::Column::OwnerUserId.is_null())
                .add(cbl_list::Column::OwnerUserId.eq(user_id)),
        )
        .filter(Expr::cust(
            r#""cbl_lists"."id" = CASE WHEN "rail_dismissals"."target_kind" = 'cbl'
                                        THEN "rail_dismissals"."target_id"::uuid END"#,
        ))
        .into_query();
    Condition::any()
        .add(
            Condition::all()
                .add(rail_dismissal::Column::TargetKind.eq(DISMISS_KIND_ISSUE))
                .add(Expr::exists(issue_hit)),
        )
        .add(
            Condition::all()
                .add(rail_dismissal::Column::TargetKind.eq(DISMISS_KIND_SERIES))
                .add(Expr::exists(series_hit)),
        )
        .add(
            Condition::all()
                .add(rail_dismissal::Column::TargetKind.eq(DISMISS_KIND_CBL))
                .add(Expr::exists(cbl_hit)),
        )
}

/// Hydrate one page of dismissal rows. Targets are re-read through the
/// same visibility selects, so a target that vanished between the page
/// query and this read is dropped rather than listed without a card.
async fn hydrate_dismissals(
    app: &AppState,
    acl: &access::VisibleLibraries,
    user_id: Uuid,
    rows: Vec<rail_dismissal::Model>,
) -> Result<Vec<RailDismissalView>, Response> {
    use entity::cbl_list;
    use std::collections::HashMap;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let internal = |e: sea_orm::DbErr| {
        tracing::error!(error = %e, "rails: dismissal hydrate failed");
        error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
    };
    let ids_of = |kind: &str| -> Vec<String> {
        rows.iter()
            .filter(|r| r.target_kind == kind)
            .map(|r| r.target_id.clone())
            .collect()
    };
    let uuids_of = |kind: &str| -> Vec<Uuid> {
        ids_of(kind)
            .iter()
            .filter_map(|id| Uuid::parse_str(id).ok())
            .collect()
    };

    // Issues: card projection + parent series slug/name.
    let issue_ids = ids_of(DISMISS_KIND_ISSUE);
    let issue_rows: Vec<IssueCardRow> = if issue_ids.is_empty() {
        Vec::new()
    } else {
        access::visible_issues_select(acl)
            .filter(issue::Column::State.eq("active"))
            .filter(issue::Column::Id.is_in(issue_ids))
            .into_partial_model::<IssueCardRow>()
            .all(&app.db)
            .await
            .map_err(internal)?
    };
    let parent_ids: Vec<Uuid> = issue_rows.iter().map(|i| i.series_id).collect();
    let parents: HashMap<Uuid, (String, String)> = if parent_ids.is_empty() {
        HashMap::new()
    } else {
        series::Entity::find()
            .select_only()
            .column(series::Column::Id)
            .column(series::Column::Slug)
            .column(series::Column::Name)
            .filter(series::Column::Id.is_in(parent_ids))
            .into_tuple::<(Uuid, String, String)>()
            .all(&app.db)
            .await
            .map_err(internal)?
            .into_iter()
            .map(|(id, slug, name)| (id, (slug, name)))
            .collect()
    };
    let issues: HashMap<String, IssueSummaryView> = issue_rows
        .into_iter()
        .filter_map(|row| {
            let (slug, name) = parents.get(&row.series_id)?;
            let view = row.into_summary_view(slug).with_series_name(name.clone());
            Some((view.id.clone(), view))
        })
        .collect();

    // Series: the shared hydrate gives the same cover the cards paint.
    let series_ids = uuids_of(DISMISS_KIND_SERIES);
    let series_models = if series_ids.is_empty() {
        Vec::new()
    } else {
        access::visible_series_select(acl)
            .filter(series::Column::Id.is_in(series_ids))
            .all(&app.db)
            .await
            .map_err(internal)?
    };
    let series_views: HashMap<String, SeriesView> = hydrate_series(app, series_models, user_id)
        .await
        .into_iter()
        .map(|v| (v.id.clone(), v))
        .collect();

    // CBL lists: own or global, plus the saved view that links them.
    let cbl_ids = uuids_of(DISMISS_KIND_CBL);
    let cbl_names: HashMap<Uuid, String> = if cbl_ids.is_empty() {
        HashMap::new()
    } else {
        cbl_list::Entity::find()
            .select_only()
            .column(cbl_list::Column::Id)
            .column(cbl_list::Column::ParsedName)
            .filter(cbl_list::Column::Id.is_in(cbl_ids.clone()))
            .filter(
                Condition::any()
                    .add(cbl_list::Column::OwnerUserId.is_null())
                    .add(cbl_list::Column::OwnerUserId.eq(user_id)),
            )
            .into_tuple::<(Uuid, String)>()
            .all(&app.db)
            .await
            .map_err(internal)?
            .into_iter()
            .collect()
    };
    let cbl_views = cbl_saved_view_ids_for_candidates(app, user_id, &cbl_ids).await?;

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let kind = RailDismissalKind::from_db(&r.target_kind)?;
            let target = match kind {
                RailDismissalKind::Issue => {
                    let i = issues.get(&r.target_id)?;
                    let series_name = i.series_name.clone().unwrap_or_default();
                    RailDismissalTargetView {
                        name: match &i.number {
                            Some(n) => format!("{series_name} #{n}"),
                            None => series_name,
                        },
                        subtitle: i.title.clone(),
                        slug: Some(i.slug.clone()),
                        series_slug: Some(i.series_slug.clone()),
                        saved_view_id: None,
                        cover_url: i.cover_url.clone(),
                    }
                }
                RailDismissalKind::Series => {
                    let s = series_views.get(&r.target_id)?;
                    RailDismissalTargetView {
                        name: s.name.clone(),
                        subtitle: s.year.map(|y| y.to_string()),
                        slug: Some(s.slug.clone()),
                        series_slug: None,
                        saved_view_id: None,
                        cover_url: s.cover_url.clone(),
                    }
                }
                RailDismissalKind::Cbl => {
                    let id = Uuid::parse_str(&r.target_id).ok()?;
                    RailDismissalTargetView {
                        name: cbl_names.get(&id)?.clone(),
                        subtitle: None,
                        slug: None,
                        series_slug: None,
                        saved_view_id: cbl_views.get(&id).map(Uuid::to_string),
                        cover_url: None,
                    }
                }
            };
            Some(RailDismissalView {
                id: format!("{}:{}", r.target_kind, r.target_id),
                target_kind: kind,
                hidden_from: kind.hidden_from(),
                dismissed_at: r.dismissed_at.to_rfc3339(),
                target_id: r.target_id,
                target,
            })
        })
        .collect())
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
            if !access::issue_allowed(app, &acl, &issue).await {
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
            if !acl.series_ok(series.library_id, series.age_rating.as_deref()) {
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
