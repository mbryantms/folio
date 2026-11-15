//! Upcoming releases: what readers see (upcoming-releases 1.0, part E).
//!
//! | method | path | |
//! |---|---|---|
//! | `GET` | `/upcoming` | weeks of reconciled releases, cursor = week |
//! | `GET` | `/upcoming/summary` | counts for the nav badge, each source's "as of", publishers |
//! | `GET` | `/series/{slug}/next-release` | the next releases of one series |
//!
//! All three take `CurrentUser` and are read-only. They answer from
//! Postgres alone: no request here reaches Metron or Mylar.
//!
//! **Access.** A release is public information; what is the viewer's own
//! is whether a series is in a library they can see, and whether Mylar
//! tracks it. `upcoming::view` loads Mylar data only for a viewer who can
//! see the whole library. A viewer with an age-rating cap is held to "my
//! series": they are not shown every publisher's titles.
//!
//! Every filter is a server query parameter. The client never narrows a
//! page it was sent.

use axum::{
    Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use shared::error::ApiErrorCode;
use shared::pagination::{CursorPage, decode_cursor, encode_cursor};
use std::collections::BTreeMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::respond;
use super::series::find_by_slug;
use crate::auth::CurrentUser;
use crate::library::access;
use crate::metadata::identifier::{Source, canonical_url};
use crate::state::AppState;
use crate::upcoming::mylar::Freshness;
use crate::upcoming::reconcile::{Entry, EntrySource, MylarStatus, Stage};
use crate::upcoming::{calendar, view, week_start};
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(summary))
        .routes(routes!(series_next_release))
}

const DEFAULT_WEEKS: u64 = 3;
const MAX_WEEKS: u64 = 12;
/// Releases `/series/{slug}/next-release` returns at most.
const NEXT_RELEASES: usize = 2;
/// The only host a calendar cover is served from (and the CSP allows).
const COVER_HOST_PREFIX: &str = "https://static.metron.cloud/";

/// Which releases a list shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpcomingScope {
    /// Series in a library the caller can see, or on Mylar's watchlist.
    #[default]
    Mine,
    /// Every release the calendar holds.
    All,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct UpcomingQuery {
    /// Opaque continuation token from a previous page's `next_cursor`.
    pub cursor: Option<String>,
    /// Weeks per page, 1–12 (default 3).
    pub limit: Option<u64>,
    /// The caller's local date, `YYYY-MM-DD`. Used to place "this week"
    /// when it is within a day of the server's own date.
    #[param(value_type = Option<String>, format = Date)]
    pub today: Option<NaiveDate>,
    /// `mine` (default) or `all`.
    #[param(inline)]
    pub scope: Option<UpcomingScope>,
    /// Only this publisher (exact name, case-insensitive).
    pub publisher: Option<String>,
    /// Only series on Mylar's watchlist.
    pub tracked: Option<bool>,
    /// Only series in a library the caller can see.
    pub in_library: Option<bool>,
    /// Only #1 issues.
    pub first_issues: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct TodayQuery {
    /// The caller's local date, `YYYY-MM-DD`.
    #[param(value_type = Option<String>, format = Date)]
    pub today: Option<NaiveDate>,
}

/// The Folio series a release belongs to.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpcomingSeriesLinkView {
    pub slug: String,
    pub name: String,
}

/// One upcoming (or just released) issue.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpcomingEntryView {
    /// Stable within one calendar + snapshot state; for list keys only.
    pub key: String,
    pub source: EntrySource,
    pub series_name: String,
    pub volume: Option<i32>,
    pub year_began: Option<i32>,
    pub publisher: Option<String>,
    pub number: String,
    /// In-store date.
    #[schema(value_type = String, format = Date)]
    pub release_date: NaiveDate,
    #[schema(value_type = Option<String>, format = Date)]
    pub cover_date: Option<NaiveDate>,
    /// Cover on Metron's CDN, when Metron lists the release.
    pub cover_url: Option<String>,
    /// The release's page on Metron.
    pub metron_url: Option<String>,
    /// A #1.
    pub first_issue: bool,
    /// Set when the series is in a library the caller can see.
    pub library_series: Option<UpcomingSeriesLinkView>,
    /// Folio already has this issue.
    pub owned: bool,
    /// The series is on Mylar's watchlist. Always `false` for a caller
    /// who may not see Mylar data.
    pub tracked: bool,
    /// What Mylar reported for the issue. Always absent for a caller who
    /// may not see Mylar data.
    pub mylar_status: Option<MylarStatus>,
    pub stage: Stage,
}

/// One Monday–Sunday week of releases.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpcomingWeekView {
    #[schema(value_type = String, format = Date)]
    pub week_start: NaiveDate,
    #[schema(value_type = String, format = Date)]
    pub week_end: NaiveDate,
    pub entries: Vec<UpcomingEntryView>,
}

/// Mylar's snapshot, for a caller allowed to see it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpcomingMylarView {
    #[schema(value_type = String, format = DateTime)]
    pub received_at: DateTime<FixedOffset>,
    pub freshness: Freshness,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpcomingSummaryView {
    /// `upcoming.enabled`. When `false` every other field is empty.
    pub enabled: bool,
    /// "My series" releases this week and next.
    pub this_week: u32,
    pub next_week: u32,
    /// When the calendar last finished a full Metron pass.
    #[schema(value_type = Option<String>, format = DateTime)]
    pub calendar_synced_at: Option<DateTime<FixedOffset>>,
    /// Absent unless the caller may see Mylar data and a snapshot exists.
    pub mylar: Option<UpcomingMylarView>,
    /// Publishers with a release in the window, for the filter.
    pub publishers: Vec<String>,
    /// `false` for a caller held to "my series" (an age-rating cap).
    pub all_releases_available: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesNextReleaseView {
    /// Soonest first; empty when nothing is announced.
    pub entries: Vec<UpcomingEntryView>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WeekCursor {
    /// Monday of the next week to return.
    w: NaiveDate,
}

fn internal(e: &sea_orm::DbErr) -> Response {
    tracing::error!(error = %e, "upcoming query failed");
    respond(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Internal,
        "internal",
    )
}

/// The caller's date when it is plausibly today (the server only knows
/// UTC, and "this week" turns over at the caller's midnight), else the
/// server's.
fn resolve_today(claimed: Option<NaiveDate>) -> NaiveDate {
    let server = Utc::now().date_naive();
    match claimed {
        Some(d) if (d - server).num_days().abs() <= 1 => d,
        _ => server,
    }
}

fn entry_view(e: Entry) -> UpcomingEntryView {
    let p = e.paired;
    UpcomingEntryView {
        key: p.key(),
        source: p.source,
        cover_url: p.image_url.filter(|u| u.starts_with(COVER_HOST_PREFIX)),
        metron_url: p
            .metron_issue_id
            .and_then(|id| canonical_url(Source::Metron, "issue", &id.to_string())),
        series_name: p.series_name,
        volume: p.volume,
        year_began: p.year_began,
        publisher: p.publisher,
        number: p.number,
        release_date: p.date,
        cover_date: p.cover_date,
        first_issue: e.first_issue,
        library_series: e.library_series.map(|l| UpcomingSeriesLinkView {
            slug: l.slug,
            name: l.name,
        }),
        owned: e.owned,
        tracked: e.tracked,
        mylar_status: e.mylar_status,
        stage: e.stage,
    }
}

/// The last day the calendar covers, from the Monday of `today`'s week.
fn window_end(first_week: NaiveDate, weeks_ahead: u32) -> NaiveDate {
    first_week + Duration::weeks(i64::from(weeks_ahead)) + Duration::days(6)
}

#[utoipa::path(
    operation_id = "upcoming_list", get,
    path = "/upcoming",
    params(UpcomingQuery),
    responses(
        (status = 200, body = CursorPage<UpcomingWeekView>),
        (status = 400, description = "invalid cursor"),
    )
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<UpcomingQuery>,
) -> Response {
    let cfg = app.cfg();
    let empty = || {
        Json(CursorPage::<UpcomingWeekView>::paginated(
            Vec::new(),
            None,
            Some(0),
        ))
        .into_response()
    };
    if !cfg.upcoming_enabled {
        return empty();
    }
    let after = match q.cursor.as_deref() {
        Some(c) => match decode_cursor::<WeekCursor>(c) {
            Ok(k) => Some(k.w),
            Err(_) => {
                return respond(
                    StatusCode::BAD_REQUEST,
                    ApiErrorCode::Validation,
                    "invalid cursor",
                );
            }
        },
        None => None,
    };
    let limit = usize::try_from(q.limit.unwrap_or(DEFAULT_WEEKS).clamp(1, MAX_WEEKS)).unwrap_or(3);
    let first_week = week_start(resolve_today(q.today));
    let last_day = window_end(first_week, cfg.upcoming_weeks_ahead);

    let loaded = match view::load(&app, &user, first_week, last_day).await {
        Ok(l) => l,
        Err(e) => return internal(&e),
    };
    // A capped viewer is held to their own series, whatever they ask for.
    let scope = if loaded.capped {
        UpcomingScope::Mine
    } else {
        q.scope.unwrap_or_default()
    };
    let publisher = q
        .publisher
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_lowercase);
    let keep = |e: &Entry| {
        (scope == UpcomingScope::All || e.is_mine())
            && (q.tracked != Some(true) || e.tracked)
            && (q.in_library != Some(true) || e.library_series.is_some())
            && (q.first_issues != Some(true) || e.first_issue)
            && publisher.as_deref().is_none_or(|want| {
                e.paired
                    .publisher
                    .as_deref()
                    .is_some_and(|p| p.to_lowercase() == want)
            })
    };
    let mut weeks: BTreeMap<NaiveDate, Vec<Entry>> = BTreeMap::new();
    let mut total = 0u64;
    for e in loaded.entries.into_iter().filter(keep) {
        total += 1;
        weeks.entry(week_start(e.paired.date)).or_default().push(e);
    }

    let start = after.unwrap_or(first_week);
    let mut page: Vec<UpcomingWeekView> = Vec::new();
    let mut next: Option<NaiveDate> = None;
    for (monday, entries) in weeks.into_iter().filter(|(m, _)| *m >= start) {
        if page.len() == limit {
            next = Some(monday);
            break;
        }
        page.push(UpcomingWeekView {
            week_start: monday,
            week_end: monday + Duration::days(6),
            entries: entries.into_iter().map(entry_view).collect(),
        });
    }
    let next_cursor = match next.map(|w| encode_cursor(&WeekCursor { w })) {
        Some(Ok(c)) => Some(c),
        Some(Err(_)) => {
            return respond(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Internal,
                "internal",
            );
        }
        None => None,
    };
    Json(CursorPage::paginated(
        page,
        next_cursor,
        after.is_none().then_some(total),
    ))
    .into_response()
}

#[utoipa::path(
    operation_id = "upcoming_summary", get,
    path = "/upcoming/summary",
    params(TodayQuery),
    responses((status = 200, body = UpcomingSummaryView))
)]
#[handler]
pub async fn summary(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<TodayQuery>,
) -> Response {
    let cfg = app.cfg();
    if !cfg.upcoming_enabled {
        return Json(UpcomingSummaryView {
            enabled: false,
            this_week: 0,
            next_week: 0,
            calendar_synced_at: None,
            mylar: None,
            publishers: Vec::new(),
            all_releases_available: false,
        })
        .into_response();
    }
    let first_week = week_start(resolve_today(q.today));
    let next_week = first_week + Duration::weeks(1);
    // The badge needs two weeks; the publisher filter needs the window.
    let loaded = match view::load(&app, &user, first_week, next_week + Duration::days(6)).await {
        Ok(l) => l,
        Err(e) => return internal(&e),
    };
    let mine = |week: NaiveDate| {
        u32::try_from(
            loaded
                .entries
                .iter()
                .filter(|e| e.is_mine() && week_start(e.paired.date) == week)
                .count(),
        )
        .unwrap_or(u32::MAX)
    };
    let publishers = match view::publishers(
        &app.db,
        first_week,
        window_end(first_week, cfg.upcoming_weeks_ahead),
    )
    .await
    {
        Ok(p) => p,
        Err(e) => return internal(&e),
    };
    let synced = match calendar::load_state(&app.db).await {
        Ok(s) => s.last_finished_at,
        Err(e) => return internal(&e),
    };
    Json(UpcomingSummaryView {
        enabled: true,
        this_week: mine(first_week),
        next_week: mine(next_week),
        calendar_synced_at: synced,
        mylar: loaded.mylar.map(|m| UpcomingMylarView {
            received_at: m.received_at,
            freshness: m.freshness,
        }),
        publishers: if loaded.capped {
            Vec::new()
        } else {
            publishers
        },
        all_releases_available: !loaded.capped,
    })
    .into_response()
}

#[utoipa::path(
    operation_id = "series_next_release", get,
    path = "/series/{slug}/next-release",
    params(("slug" = String, Path), TodayQuery),
    responses(
        (status = 200, body = SeriesNextReleaseView),
        (status = 404, description = "series not found or not visible"),
    )
)]
#[handler]
pub async fn series_next_release(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Query(q): Query<TodayQuery>,
) -> Response {
    let s = match find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if !access::series_page_visible(&app, &user, &s).await {
        return respond(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            "series not found",
        );
    }
    if !app.cfg().upcoming_enabled {
        return Json(SeriesNextReleaseView {
            entries: Vec::new(),
        })
        .into_response();
    }
    let today = resolve_today(q.today);
    match view::next_for_series(&app, &user, s.id, today, NEXT_RELEASES).await {
        Ok(loaded) => Json(SeriesNextReleaseView {
            entries: loaded.entries.into_iter().map(entry_view).collect(),
        })
        .into_response(),
        Err(e) => internal(&e),
    }
}
