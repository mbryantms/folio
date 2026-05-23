//! `GET /me/reading-log` — reverse-chronological union of every
//! reading-activity event a user has generated. Backs the customizable
//! Reading Log page; spec at `~/.claude/plans/reading-log.md`.
//!
//! Four event kinds union into one cursor-paginated feed:
//!
//! | kind                | source                                              |
//! |---------------------|-----------------------------------------------------|
//! | `issue_finished`    | `progress_records` rows with `finished_at IS NOT NULL` |
//! | `series_finished`   | derived: MAX(`finished_at`) over a series whose every active issue has been finished |
//! | `session_completed` | `reading_sessions` with `ended_at IS NOT NULL` AND `active_ms ≥ 60_000` AND `distinct_pages_read > 0` |
//! | `marker_created`    | `markers` rows (all four kinds) keyed on `created_at` |
//!
//! Cursor: opaque base64 of `{occurred_at}|{kind}|{id}`. Compared as
//! `(occurred_at, id) DESC` tuple — `kind` is in the cursor for
//! debugging but not used in the comparison; `id` is the synthetic
//! event id (`iss-fin:<issueId>`, `ses:<sessionId>`,
//! `ser-fin:<seriesId>`, `mrk:<markerId>`).
//!
//! Strategy: per-kind queries each fetch `limit + 1` (or so) rows
//! ordered DESC by their natural timestamp, then we merge in memory,
//! take the first `limit`, and emit a `next_cursor` from the tail.
//! With `limit ≤ 100` the worst-case fan-out is 4 × 100 = 400 rows —
//! cheap, indexed reads.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, FixedOffset};
use entity::{issue, library_user_access, marker, progress_record, reading_session, series};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, EntityTrait, FromQueryResult, QueryFilter, QueryOrder,
    QuerySelect, Statement, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use super::error;
use crate::auth::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me/reading-log", get(reading_log))
        .route("/me/reading-log/hide", post(hide))
        .route("/me/reading-log/unhide", post(unhide))
}

// ───────── Request / response shapes ─────────

#[derive(Debug, Deserialize)]
pub struct ReadingLogQuery {
    /// Opaque base64 cursor returned by the previous page. Omit for
    /// the first page.
    pub cursor: Option<String>,
    /// Max events to return. Clamped to `[1, 100]`; default 30.
    pub limit: Option<u32>,
    /// Comma-separated event kinds to include. Default: all four.
    /// Unknown kinds are silently ignored so old clients survive new
    /// kinds being added later.
    pub kind: Option<String>,
    /// Lower bound (inclusive), RFC3339.
    pub from: Option<String>,
    /// Upper bound (exclusive), RFC3339.
    pub to: Option<String>,
    /// Restrict to events whose issue lives in this library. Optional.
    pub library_id: Option<Uuid>,
    /// Restrict to events for issues in this series. Optional.
    pub series_id: Option<Uuid>,
    /// When `true`, include events the user has hidden via
    /// `POST /me/reading-log/hide`. Defaults to `false` so the
    /// regular feed reads as the user expects ("only what I haven't
    /// asked to hide"). Hidden events carry `is_hidden: true` in the
    /// response so the UI can render them differently (faded + an
    /// unhide affordance). Used by the `/log` page's "Show hidden"
    /// toggle.
    #[serde(default)]
    pub include_hidden: Option<bool>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ReadingLogPageView {
    pub events: Vec<ReadingLogEventView>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ReadingLogEventView {
    /// Synthetic stable id: `{kind-prefix}:{underlying-id}`. Same id
    /// across pages — clients dedupe with it.
    pub id: String,
    /// One of `issue_finished` / `series_finished` / `session_completed`
    /// / `marker_created`.
    pub kind: String,
    /// RFC3339 — the canonical sort key for the feed.
    pub occurred_at: String,
    /// Hydrated series row (when the event has one). All four kinds
    /// have a series; `None` only on the very rare case where the
    /// referenced series was deleted between event capture and the
    /// hydrate query.
    pub series: Option<EventSeries>,
    /// Hydrated issue row (when the event has one). `series_finished`
    /// is the only kind with `issue = None`; the others always
    /// resolve.
    pub issue: Option<EventIssue>,
    /// Kind-specific metadata. Different schema per kind, see
    /// `EventPayload` variants.
    pub payload: EventPayload,
    /// `true` when this event was hidden via
    /// `POST /me/reading-log/hide` and is only being surfaced
    /// because the caller passed `?include_hidden=true`. UI uses it
    /// to render a faded card + an "Unhide" affordance. Default
    /// `false` is `skip_serializing_if`-elided so the wire payload
    /// stays the same shape for clients that don't use the feature.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_hidden: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EventSeries {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub year: Option<i32>,
    pub publisher: Option<String>,
    pub imprint: Option<String>,
    pub cover_url: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EventIssue {
    pub id: String,
    pub slug: String,
    pub number: Option<String>,
    pub title: Option<String>,
    pub year: Option<i32>,
    pub month: Option<i32>,
    pub day: Option<i32>,
    pub page_count: Option<i32>,
    pub cover_url: Option<String>,
    pub writer: Option<String>,
    pub penciller: Option<String>,
    pub inker: Option<String>,
    pub colorist: Option<String>,
    pub letterer: Option<String>,
    pub cover_artist: Option<String>,
    pub editor: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventPayload {
    IssueFinished {
        /// Best-effort first-read timestamp (earliest reading_session
        /// `started_at` for this user+issue). `None` when no session
        /// rows survived for the issue.
        first_read_at: Option<String>,
        /// Count of finalized reading_sessions for this user+issue.
        total_sessions: i64,
        /// Sum of `active_ms` across those sessions.
        total_active_ms: i64,
        /// Heuristic: `total_sessions > 1` is a reread. Refined in M6+.
        is_reread: bool,
    },
    SeriesFinished {
        /// Earliest `finished_at` across the series's matched issues —
        /// when the user first finished any issue in the series.
        started_at: Option<String>,
        /// Active issues in the series. Always > 0 (you can't finish
        /// a series with zero issues).
        total_issues: i64,
        /// Sum of all `active_ms` across the series's sessions.
        total_active_ms: i64,
        /// `(finished_at - started_at)` whole days. `None` when
        /// `started_at` is missing.
        span_days: Option<i64>,
    },
    SessionCompleted {
        started_at: String,
        ended_at: String,
        active_ms: i64,
        pages_read: i32,
        device: Option<String>,
        view_mode: Option<String>,
    },
    MarkerCreated {
        marker_id: String,
        marker_kind: String,
        page_index: i32,
        tags: Vec<String>,
        /// First ~80 chars of the marker body. `None` when the marker
        /// has no body (bookmarks / favorites typically don't).
        body_preview: Option<String>,
    },
}

// ───────── Constants ─────────

/// Sessions shorter than this OR with 0 pages read are dropped from
/// the feed — heartbeat-only thumbnail bounces would otherwise
/// dominate. See plan §M1 question 2.
const SESSION_MIN_ACTIVE_MS: i64 = 60_000;
const DEFAULT_LIMIT: u32 = 30;
const MAX_LIMIT: u32 = 100;

// ───────── Cursor ─────────

#[derive(Debug, Clone)]
struct Cursor {
    occurred_at: DateTime<FixedOffset>,
    id: String,
}

fn encode_cursor(occurred_at: DateTime<FixedOffset>, kind: &str, id: &str) -> String {
    use base64::Engine;
    let raw = format!("{}|{}|{}", occurred_at.to_rfc3339(), kind, id);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw.as_bytes())
}

fn decode_cursor(s: &str) -> Result<Cursor, ()> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .map_err(|_| ())?;
    let txt = std::str::from_utf8(&bytes).map_err(|_| ())?;
    let mut parts = txt.splitn(3, '|');
    let ts = parts.next().ok_or(())?;
    let _kind = parts.next().ok_or(())?;
    let id = parts.next().ok_or(())?;
    let occurred_at = DateTime::parse_from_rfc3339(ts).map_err(|_| ())?;
    Ok(Cursor {
        occurred_at,
        id: id.to_owned(),
    })
}

// ───────── Internal candidate row ─────────

/// One unhydrated event harvested from a source query. We merge a
/// slice of these in memory before hydration so the issue/series
/// joins happen exactly once.
#[derive(Debug, Clone)]
struct Candidate {
    occurred_at: DateTime<FixedOffset>,
    kind: EventKind,
    /// Synthetic stable id.
    id: String,
    /// FK to the issue row (when applicable). `None` for
    /// `SeriesFinished`.
    issue_id: Option<String>,
    /// FK to the series row. Always set.
    series_id: Uuid,
    /// Carries kind-specific raw data through the hydration step.
    raw: CandidateRaw,
    /// `true` when this event was hidden by the user (via
    /// `POST /me/reading-log/hide`) and is only being included in
    /// the merged candidate list because the caller passed
    /// `?include_hidden=true`. Surfaces as `is_hidden` on the wire
    /// payload. Always `false` for non-include-hidden requests since
    /// the hidden rows are filtered out by the source queries.
    is_hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum EventKind {
    IssueFinished,
    SeriesFinished,
    SessionCompleted,
    MarkerCreated,
}

impl EventKind {
    fn as_str(self) -> &'static str {
        match self {
            EventKind::IssueFinished => "issue_finished",
            EventKind::SeriesFinished => "series_finished",
            EventKind::SessionCompleted => "session_completed",
            EventKind::MarkerCreated => "marker_created",
        }
    }

    fn parse(s: &str) -> Option<EventKind> {
        match s {
            "issue_finished" => Some(EventKind::IssueFinished),
            "series_finished" => Some(EventKind::SeriesFinished),
            "session_completed" => Some(EventKind::SessionCompleted),
            "marker_created" => Some(EventKind::MarkerCreated),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
enum CandidateRaw {
    IssueFinished,
    SeriesFinished,
    Session {
        started_at: DateTime<FixedOffset>,
        active_ms: i64,
        pages_read: i32,
        device: Option<String>,
        view_mode: Option<String>,
    },
    Marker {
        marker_id: Uuid,
        marker_kind: String,
        page_index: i32,
        tags: Vec<String>,
        body: Option<String>,
    },
}

// ───────── Handler ─────────

#[utoipa::path(
    get,
    path = "/me/reading-log",
    params(
        ("cursor"     = Option<String>, Query,),
        ("limit"      = Option<u32>,    Query,),
        ("kind"       = Option<String>, Query,),
        ("from"       = Option<String>, Query,),
        ("to"         = Option<String>, Query,),
        ("library_id" = Option<String>, Query,),
        ("series_id"  = Option<String>, Query,),
    ),
    responses(
        (status = 200, body = ReadingLogPageView),
        (status = 400, description = "validation"),
    )
)]
pub async fn reading_log(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ReadingLogQuery>,
) -> Response {
    // ── Parse & validate query params ──
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as i64;

    let kinds_requested: HashSet<EventKind> = match q.kind.as_deref() {
        None => [
            EventKind::IssueFinished,
            EventKind::SeriesFinished,
            EventKind::SessionCompleted,
            EventKind::MarkerCreated,
        ]
        .into_iter()
        .collect(),
        Some(csv) => csv.split(',').filter_map(EventKind::parse).collect(),
    };
    if kinds_requested.is_empty() {
        // Unknown / empty kind filter → empty page rather than 400, so
        // a forward-compatible client probing for new kinds doesn't
        // crash on a server that hasn't shipped them.
        return Json(ReadingLogPageView {
            events: Vec::new(),
            next_cursor: None,
        })
        .into_response();
    }

    let from = match q.from.as_deref().map(DateTime::parse_from_rfc3339) {
        Some(Ok(t)) => Some(t),
        Some(Err(_)) => {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "from must be RFC3339",
            );
        }
        None => None,
    };
    let to = match q.to.as_deref().map(DateTime::parse_from_rfc3339) {
        Some(Ok(t)) => Some(t),
        Some(Err(_)) => {
            return error(StatusCode::BAD_REQUEST, "validation", "to must be RFC3339");
        }
        None => None,
    };

    let cursor = match q.cursor.as_deref().map(decode_cursor) {
        Some(Ok(c)) => Some(c),
        Some(Err(_)) => return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor"),
        None => None,
    };
    let include_hidden = q.include_hidden.unwrap_or(false);

    // ── Per-source candidate harvest ──
    // Each source pulls `limit` rows (the worst-case feed contribution
    // when all of them are concentrated at the top of the timeline).
    // After merging we keep only the first `limit` — over-fetching is
    // the cost of unioning ordered sources without a global index.

    let mut candidates: Vec<Candidate> = Vec::with_capacity((limit as usize) * 4);
    // Each source over-fetches by one so the merge layer can detect
    // "more remain" even when a single source dominates the page. With
    // limit=3 and a source returning exactly 3 rows, has_more couldn't
    // distinguish "exactly 3" from "≥ 3, truncated"; fetching 4 makes
    // the truncate->pop branch fire iff the source actually has more.
    let fetch_limit = limit + 1;

    macro_rules! collect_from {
        ($call:expr) => {
            match $call.await {
                Ok(rows) => candidates.extend(rows),
                Err(e) => return internal_err(e),
            }
        };
    }

    if kinds_requested.contains(&EventKind::IssueFinished) {
        collect_from!(fetch_issue_finished(
            &app,
            user.id,
            fetch_limit,
            cursor.as_ref(),
            from.as_ref(),
            to.as_ref(),
            q.series_id,
            include_hidden,
        ));
    }
    if kinds_requested.contains(&EventKind::SessionCompleted) {
        collect_from!(fetch_sessions(
            &app,
            user.id,
            fetch_limit,
            cursor.as_ref(),
            from.as_ref(),
            to.as_ref(),
            q.series_id,
            include_hidden,
        ));
    }
    if kinds_requested.contains(&EventKind::MarkerCreated) {
        collect_from!(fetch_markers(
            &app,
            user.id,
            fetch_limit,
            cursor.as_ref(),
            from.as_ref(),
            to.as_ref(),
            q.series_id,
            include_hidden,
        ));
    }
    if kinds_requested.contains(&EventKind::SeriesFinished) {
        // Series-finished is a derived event (MAX(finished_at) per
        // series); there's no single row to flag as hidden. The
        // fetcher continues to filter `is_backfill = false` so the
        // event surfaces consistently with the issue-finished side.
        collect_from!(fetch_series_finished(
            &app,
            user.id,
            fetch_limit,
            cursor.as_ref(),
            from.as_ref(),
            to.as_ref(),
            q.series_id,
        ));
    }

    // ── Merge & truncate ──
    candidates.sort_by(|a, b| {
        b.occurred_at
            .cmp(&a.occurred_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    candidates.truncate((limit as usize) + 1);
    let has_more = candidates.len() > limit as usize;
    if has_more {
        candidates.pop();
    }

    // ── ACL: collect series + issue rows, then drop events whose
    //   series sits in a library the user can't see. The library_id
    //   filter (when supplied) is enforced here too. ──
    let series_ids: HashSet<Uuid> = candidates.iter().map(|c| c.series_id).collect();
    let issue_ids: HashSet<String> = candidates
        .iter()
        .filter_map(|c| c.issue_id.clone())
        .collect();

    let series_rows = if series_ids.is_empty() {
        Vec::new()
    } else {
        match series::Entity::find()
            .filter(series::Column::Id.is_in(series_ids.iter().copied().collect::<Vec<_>>()))
            .all(&app.db)
            .await
        {
            Ok(v) => v,
            Err(e) => return internal_err(e),
        }
    };
    let issue_rows = if issue_ids.is_empty() {
        Vec::new()
    } else {
        match issue::Entity::find()
            .filter(issue::Column::Id.is_in(issue_ids.iter().cloned().collect::<Vec<_>>()))
            .all(&app.db)
            .await
        {
            Ok(v) => v,
            Err(e) => return internal_err(e),
        }
    };
    let series_by_id: HashMap<Uuid, series::Model> =
        series_rows.into_iter().map(|s| (s.id, s)).collect();
    let issue_by_id: HashMap<String, issue::Model> =
        issue_rows.into_iter().map(|i| (i.id.clone(), i)).collect();

    // Representative cover thumbnail per series. `series_finished`
    // events carry no `event.issue`, so without this the frontend
    // would render a grey placeholder for every "Series complete"
    // row. We pick the lowest-`sort_number` active issue per series
    // as the cover — one extra batched query per page.
    let series_cover_by_id: HashMap<Uuid, String> = if series_ids.is_empty() {
        HashMap::new()
    } else {
        let candidates_for_cover = match issue::Entity::find()
            .filter(issue::Column::SeriesId.is_in(series_ids.iter().copied().collect::<Vec<_>>()))
            .filter(issue::Column::State.eq("active"))
            .filter(issue::Column::RemovedAt.is_null())
            .all(&app.db)
            .await
        {
            Ok(v) => v,
            Err(e) => return internal_err(e),
        };
        let mut by_series: HashMap<Uuid, (Option<f64>, String)> = HashMap::new();
        for i in candidates_for_cover {
            let entry = by_series.entry(i.series_id);
            // Keep the issue with the lowest `sort_number` (NULLs
            // sort last); ties broken by whichever we see first
            // since the query order isn't guaranteed.
            let url = format!("/issues/{}/pages/0/thumb", i.id);
            entry
                .and_modify(|cur| {
                    let take = match (cur.0, i.sort_number) {
                        (Some(a), Some(b)) => b < a,
                        (None, Some(_)) => true,
                        _ => false,
                    };
                    if take {
                        *cur = (i.sort_number, url.clone());
                    }
                })
                .or_insert_with(|| (i.sort_number, url));
        }
        by_series
            .into_iter()
            .map(|(k, (_, url))| (k, url))
            .collect()
    };

    let allowed_libraries: Option<HashSet<Uuid>> = if user.role == "admin" {
        None
    } else {
        match library_user_access::Entity::find()
            .filter(library_user_access::Column::UserId.eq(user.id))
            .all(&app.db)
            .await
        {
            Ok(v) => Some(v.into_iter().map(|r| r.library_id).collect()),
            Err(e) => return internal_err(e),
        }
    };

    let visible = |library_id: Uuid| -> bool {
        if let Some(filter) = q.library_id
            && filter != library_id
        {
            return false;
        }
        match &allowed_libraries {
            None => true, // admin
            Some(set) => set.contains(&library_id),
        }
    };

    let events: Vec<ReadingLogEventView> = candidates
        .into_iter()
        .filter_map(|c| {
            hydrate(
                c,
                &series_by_id,
                &issue_by_id,
                &series_cover_by_id,
                &visible,
            )
        })
        .collect();

    let next_cursor = if has_more {
        events
            .last()
            .map(|e| encode_cursor(parse_rfc3339(&e.occurred_at), &e.kind, &e.id))
    } else {
        None
    };

    Json(ReadingLogPageView {
        events,
        next_cursor,
    })
    .into_response()
}

fn parse_rfc3339(s: &str) -> DateTime<FixedOffset> {
    // The strings round-trip from our own `to_rfc3339()` output, so
    // failure here is a programmer error.
    DateTime::parse_from_rfc3339(s).expect("internal rfc3339")
}

fn internal_err<E: std::fmt::Display>(e: E) -> Response {
    tracing::warn!(error = %e, "reading_log query failed");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}

// ───────── Hydration ─────────

fn hydrate(
    c: Candidate,
    series_by_id: &HashMap<Uuid, series::Model>,
    issue_by_id: &HashMap<String, issue::Model>,
    series_cover_by_id: &HashMap<Uuid, String>,
    visible: &dyn Fn(Uuid) -> bool,
) -> Option<ReadingLogEventView> {
    let series = series_by_id.get(&c.series_id)?;
    if !visible(series.library_id) {
        return None;
    }
    let issue_row = c.issue_id.as_deref().and_then(|id| issue_by_id.get(id));
    // Issue is required for everything except series_finished.
    if matches!(c.kind, EventKind::SeriesFinished) {
        // OK; payload pulled from raw.
    } else if issue_row.is_none() {
        return None;
    }

    let event_series = EventSeries {
        id: series.id.to_string(),
        slug: series.slug.clone(),
        name: series.name.clone(),
        year: series.year,
        publisher: series.publisher.clone(),
        imprint: series.imprint.clone(),
        // Lowest-`sort_number` active issue thumbnail. Used as the
        // cover for `series_finished` events (no `event.issue`)
        // and as a fallback for events whose issue happens to lack
        // a renderable thumbnail.
        cover_url: series_cover_by_id.get(&series.id).cloned(),
    };
    let event_issue = issue_row.map(|i| EventIssue {
        id: i.id.clone(),
        slug: i.slug.clone(),
        number: i.number_raw.clone(),
        title: i.title.clone(),
        year: i.year,
        month: i.month,
        day: i.day,
        page_count: i.page_count,
        cover_url: (i.state == "active").then(|| format!("/issues/{}/pages/0/thumb", i.id)),
        writer: i.writer.clone(),
        penciller: i.penciller.clone(),
        inker: i.inker.clone(),
        colorist: i.colorist.clone(),
        letterer: i.letterer.clone(),
        cover_artist: i.cover_artist.clone(),
        editor: i.editor.clone(),
    });

    let payload = match &c.raw {
        CandidateRaw::IssueFinished => EventPayload::IssueFinished {
            first_read_at: None,
            total_sessions: 0,
            total_active_ms: 0,
            is_reread: false,
        },
        CandidateRaw::SeriesFinished => EventPayload::SeriesFinished {
            started_at: None,
            total_issues: 0,
            total_active_ms: 0,
            span_days: None,
        },
        CandidateRaw::Session {
            started_at,
            active_ms,
            pages_read,
            device,
            view_mode,
        } => EventPayload::SessionCompleted {
            started_at: started_at.to_rfc3339(),
            ended_at: c.occurred_at.to_rfc3339(),
            active_ms: *active_ms,
            pages_read: *pages_read,
            device: device.clone(),
            view_mode: view_mode.clone(),
        },
        CandidateRaw::Marker {
            marker_id,
            marker_kind,
            page_index,
            tags,
            body,
        } => EventPayload::MarkerCreated {
            marker_id: marker_id.to_string(),
            marker_kind: marker_kind.clone(),
            page_index: *page_index,
            tags: tags.clone(),
            body_preview: body.as_deref().map(|b| {
                let trimmed: String = b.chars().take(80).collect();
                if b.chars().count() > 80 {
                    format!("{trimmed}…")
                } else {
                    trimmed
                }
            }),
        },
    };

    Some(ReadingLogEventView {
        id: c.id,
        kind: c.kind.as_str().to_owned(),
        occurred_at: c.occurred_at.to_rfc3339(),
        series: Some(event_series),
        issue: event_issue,
        payload,
        is_hidden: c.is_hidden,
    })
}

// ───────── Per-source harvesters ─────────

#[allow(clippy::too_many_arguments)]
async fn fetch_issue_finished(
    app: &AppState,
    user_id: Uuid,
    limit: i64,
    cursor: Option<&Cursor>,
    from: Option<&DateTime<FixedOffset>>,
    to: Option<&DateTime<FixedOffset>>,
    series_id: Option<Uuid>,
    include_hidden: bool,
) -> Result<Vec<Candidate>, sea_orm::DbErr> {
    // Pull `progress_records` for the user where `finished_at IS NOT
    // NULL`, then join to `issues` for the series id (needed for ACL
    // + series filter). Cursor + range comparisons happen in SQL so
    // we don't over-fetch.
    let mut query = progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user_id))
        .filter(progress_record::Column::FinishedAt.is_not_null());
    // Catalog/sync writes (`is_backfill = true`) are intentionally
    // excluded from the reading-log feed by default — they represent
    // issues the user is *recording* as read, not *just read*. The
    // same flag also doubles as the per-event hide marker for
    // issue_finished events (the `/me/reading-log/hide` endpoint
    // sets `is_backfill = true` on the underlying progress_records
    // row), so `?include_hidden=true` surfaces both kinds: bulk-
    // catalog rows AND user-hidden rows, with `is_hidden: true` on
    // the wire payload.
    if !include_hidden {
        query = query.filter(progress_record::Column::IsBackfill.eq(false));
    }
    if let Some(t) = from {
        query = query.filter(progress_record::Column::FinishedAt.gte(*t));
    }
    if let Some(t) = to {
        query = query.filter(progress_record::Column::FinishedAt.lt(*t));
    }
    if let Some(c) = cursor {
        // (finished_at, issue_id) < (cursor.occurred_at, cursor.issue_id)
        // expressed in SQL via OR-pair.
        let id =
            c.id.strip_prefix("iss-fin:")
                .map(str::to_owned)
                .unwrap_or_default();
        query = query.filter(
            Expr::col(progress_record::Column::FinishedAt)
                .lt(c.occurred_at)
                .or(Expr::col(progress_record::Column::FinishedAt)
                    .eq(c.occurred_at)
                    .and(Expr::col(progress_record::Column::IssueId).lt(id))),
        );
    }
    let rows = query
        .order_by_desc(progress_record::Column::FinishedAt)
        .order_by_desc(progress_record::Column::IssueId)
        .limit(limit as u64)
        .all(&app.db)
        .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    // Need series_id per issue for ACL + series filtering. Batch.
    let issue_ids: Vec<String> = rows.iter().map(|r| r.issue_id.clone()).collect();
    let issue_rows = issue::Entity::find()
        .filter(issue::Column::Id.is_in(issue_ids))
        .all(&app.db)
        .await?;
    let series_by_issue: HashMap<String, Uuid> = issue_rows
        .into_iter()
        .map(|i| (i.id, i.series_id))
        .collect();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(occurred_at) = row.finished_at else {
            continue;
        };
        let Some(sid) = series_by_issue.get(&row.issue_id).copied() else {
            continue;
        };
        if let Some(filter) = series_id
            && filter != sid
        {
            continue;
        }
        out.push(Candidate {
            occurred_at,
            kind: EventKind::IssueFinished,
            id: format!("iss-fin:{}", row.issue_id),
            issue_id: Some(row.issue_id),
            series_id: sid,
            raw: CandidateRaw::IssueFinished,
            // Reuse `is_backfill` as the hide flag for issue-finished
            // events. When include_hidden=true, rows with
            // `is_backfill = true` (whether from bulk-mark cataloging
            // or from a manual `POST /me/reading-log/hide` call) are
            // surfaced with `is_hidden: true` on the wire.
            is_hidden: row.is_backfill,
        });
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
async fn fetch_sessions(
    app: &AppState,
    user_id: Uuid,
    limit: i64,
    cursor: Option<&Cursor>,
    from: Option<&DateTime<FixedOffset>>,
    to: Option<&DateTime<FixedOffset>>,
    series_id: Option<Uuid>,
    include_hidden: bool,
) -> Result<Vec<Candidate>, sea_orm::DbErr> {
    let mut query = reading_session::Entity::find()
        .filter(reading_session::Column::UserId.eq(user_id))
        .filter(reading_session::Column::EndedAt.is_not_null())
        .filter(reading_session::Column::ActiveMs.gte(SESSION_MIN_ACTIVE_MS))
        .filter(reading_session::Column::DistinctPagesRead.gt(0));
    if !include_hidden {
        query = query.filter(reading_session::Column::HiddenFromLog.eq(false));
    }
    if let Some(s) = series_id {
        query = query.filter(reading_session::Column::SeriesId.eq(s));
    }
    if let Some(t) = from {
        query = query.filter(reading_session::Column::EndedAt.gte(*t));
    }
    if let Some(t) = to {
        query = query.filter(reading_session::Column::EndedAt.lt(*t));
    }
    if let Some(c) = cursor {
        let id_str = c.id.strip_prefix("ses:").unwrap_or_default();
        let cursor_uuid = Uuid::parse_str(id_str).unwrap_or(Uuid::nil());
        query = query.filter(
            Expr::col(reading_session::Column::EndedAt)
                .lt(c.occurred_at)
                .or(Expr::col(reading_session::Column::EndedAt)
                    .eq(c.occurred_at)
                    .and(Expr::col(reading_session::Column::Id).lt(cursor_uuid))),
        );
    }
    let rows = query
        .order_by_desc(reading_session::Column::EndedAt)
        .order_by_desc(reading_session::Column::Id)
        .limit(limit as u64)
        .all(&app.db)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let Some(occurred_at) = r.ended_at else {
            continue;
        };
        out.push(Candidate {
            occurred_at,
            kind: EventKind::SessionCompleted,
            id: format!("ses:{}", r.id),
            issue_id: Some(r.issue_id),
            series_id: r.series_id,
            raw: CandidateRaw::Session {
                started_at: r.started_at,
                active_ms: r.active_ms,
                pages_read: r.distinct_pages_read,
                device: r.device,
                view_mode: r.view_mode,
            },
            is_hidden: r.hidden_from_log,
        });
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
async fn fetch_markers(
    app: &AppState,
    user_id: Uuid,
    limit: i64,
    cursor: Option<&Cursor>,
    from: Option<&DateTime<FixedOffset>>,
    to: Option<&DateTime<FixedOffset>>,
    series_id: Option<Uuid>,
    include_hidden: bool,
) -> Result<Vec<Candidate>, sea_orm::DbErr> {
    let mut query = marker::Entity::find().filter(marker::Column::UserId.eq(user_id));
    if !include_hidden {
        query = query.filter(marker::Column::HiddenFromLog.eq(false));
    }
    if let Some(s) = series_id {
        query = query.filter(marker::Column::SeriesId.eq(s));
    }
    if let Some(t) = from {
        query = query.filter(marker::Column::CreatedAt.gte(*t));
    }
    if let Some(t) = to {
        query = query.filter(marker::Column::CreatedAt.lt(*t));
    }
    if let Some(c) = cursor {
        let id_str = c.id.strip_prefix("mrk:").unwrap_or_default();
        let cursor_uuid = Uuid::parse_str(id_str).unwrap_or(Uuid::nil());
        query = query.filter(
            Expr::col(marker::Column::CreatedAt)
                .lt(c.occurred_at)
                .or(Expr::col(marker::Column::CreatedAt)
                    .eq(c.occurred_at)
                    .and(Expr::col(marker::Column::Id).lt(cursor_uuid))),
        );
    }
    let rows = query
        .order_by_desc(marker::Column::CreatedAt)
        .order_by_desc(marker::Column::Id)
        .limit(limit as u64)
        .all(&app.db)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let is_hidden = r.hidden_from_log;
        out.push(Candidate {
            occurred_at: r.created_at,
            kind: EventKind::MarkerCreated,
            id: format!("mrk:{}", r.id),
            issue_id: Some(r.issue_id),
            series_id: r.series_id,
            is_hidden,
            raw: CandidateRaw::Marker {
                marker_id: r.id,
                marker_kind: r.kind,
                page_index: r.page_index,
                tags: r.tags,
                body: r.body,
            },
        });
    }
    Ok(out)
}

#[derive(Debug, FromQueryResult)]
struct SeriesFinishedRow {
    series_id: Uuid,
    series_finished_at: DateTime<FixedOffset>,
}

async fn fetch_series_finished(
    app: &AppState,
    user_id: Uuid,
    limit: i64,
    cursor: Option<&Cursor>,
    from: Option<&DateTime<FixedOffset>>,
    to: Option<&DateTime<FixedOffset>>,
    series_id: Option<Uuid>,
) -> Result<Vec<Candidate>, sea_orm::DbErr> {
    // Derived event: a series is "finished" when every one of its
    // active, on-disk issues has a `progress_records.finished = TRUE`
    // row for the user. We compute the series-finish timestamp as the
    // MAX(finished_at) across those rows — i.e. the moment the user
    // crossed the line.
    //
    // The query is structured so the active-issue count + the
    // user's-finished count happen in two cheap CTEs; the SELECT
    // joins them and filters where they match. This is one round
    // trip even though Sea-ORM doesn't expose a fluent API for the
    // pattern.

    // Cursor: (occurred_at, series_id::text). The "::text" cast
    // mirrors how the rest of the query orders the id field; using
    // text comparison keeps the cursor compatible across the merge
    // step which compares synthetic ids as strings.
    let cursor_clause = match cursor {
        Some(c) => {
            let id_str = c.id.strip_prefix("ser-fin:").unwrap_or("");
            format!(
                "AND (uf.series_finished_at, uf.series_id::text) < (TIMESTAMPTZ '{}', '{}')",
                c.occurred_at.to_rfc3339().replace('\'', "''"),
                id_str.replace('\'', "''"),
            )
        }
        None => String::new(),
    };
    let from_clause = match from {
        Some(t) => format!(
            "AND uf.series_finished_at >= TIMESTAMPTZ '{}'",
            t.to_rfc3339().replace('\'', "''")
        ),
        None => String::new(),
    };
    let to_clause = match to {
        Some(t) => format!(
            "AND uf.series_finished_at < TIMESTAMPTZ '{}'",
            t.to_rfc3339().replace('\'', "''")
        ),
        None => String::new(),
    };
    let series_clause = match series_id {
        Some(s) => format!("AND uf.series_id = '{s}'"),
        None => String::new(),
    };

    let sql = format!(
        r#"
        WITH active_counts AS (
            SELECT s.id AS series_id, COUNT(*) AS total_active
            FROM   series s
            JOIN   issues i ON i.series_id = s.id
            WHERE  i.state = 'active' AND i.removed_at IS NULL
            GROUP BY s.id
        ),
        user_finishes AS (
            SELECT i.series_id,
                   MAX(pr.finished_at) AS series_finished_at,
                   COUNT(*)             AS finished_count
            FROM   progress_records pr
            JOIN   issues i ON i.id = pr.issue_id
            WHERE  pr.user_id = $1
              AND  pr.finished = TRUE
              AND  pr.finished_at IS NOT NULL
              -- Mirror the issue-side filter: backfill rows shouldn't
              -- crown a series as "just finished" in the reading log.
              AND  pr.is_backfill = FALSE
              AND  i.state = 'active'
              AND  i.removed_at IS NULL
            GROUP BY i.series_id
        )
        SELECT uf.series_id, uf.series_finished_at
        FROM   user_finishes uf
        JOIN   active_counts ac ON ac.series_id = uf.series_id
        WHERE  uf.finished_count = ac.total_active
          {cursor_clause}
          {from_clause}
          {to_clause}
          {series_clause}
        ORDER BY uf.series_finished_at DESC, uf.series_id DESC
        LIMIT $2
        "#
    );

    let rows: Vec<SeriesFinishedRow> =
        SeriesFinishedRow::find_by_statement(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            sql,
            [user_id.into(), limit.into()],
        ))
        .all(&app.db)
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| Candidate {
            occurred_at: r.series_finished_at,
            kind: EventKind::SeriesFinished,
            id: format!("ser-fin:{}", r.series_id),
            issue_id: None,
            series_id: r.series_id,
            raw: CandidateRaw::SeriesFinished,
            // series_finished is derived (MAX over progress_records);
            // there's no source row to mark hidden. Always false.
            is_hidden: false,
        })
        .collect())
}

// ───────── Hide / unhide endpoints ─────────

/// Body for `POST /me/reading-log/hide` and `/unhide`. Identifies a
/// single event in the feed by its `kind` and the raw source id
/// (without the `iss-fin:` / `ses:` / `mrk:` prefix the wire payload
/// uses on `ReadingLogEventView.id` — clients strip that before
/// calling).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct HideEventReq {
    /// One of `issue_finished | session_completed | marker_created`.
    /// `series_finished` is rejected (derived event with no source
    /// row to flag).
    pub kind: String,
    /// For `issue_finished`: the `issues.id` BLAKE3 hex.
    /// For `session_completed`: the `reading_sessions.id` UUID.
    /// For `marker_created`: the `markers.id` UUID.
    pub source_id: String,
}

#[utoipa::path(
    post,
    path = "/me/reading-log/hide",
    request_body = HideEventReq,
    responses(
        (status = 204, description = "hidden"),
        (status = 400, description = "validation"),
        (status = 404, description = "no such event for this user"),
    )
)]
pub async fn hide(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<HideEventReq>,
) -> Response {
    set_hidden(&app, user.id, &req, true).await
}

#[utoipa::path(
    post,
    path = "/me/reading-log/unhide",
    request_body = HideEventReq,
    responses(
        (status = 204, description = "unhidden"),
        (status = 400, description = "validation"),
        (status = 404, description = "no such event for this user"),
    )
)]
pub async fn unhide(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<HideEventReq>,
) -> Response {
    set_hidden(&app, user.id, &req, false).await
}

/// Shared implementation for hide + unhide. Routes the flag flip to
/// the correct source table based on `kind`. All updates filter on
/// `user_id` so a caller can't toggle another user's events.
async fn set_hidden(
    app: &AppState,
    user_id: Uuid,
    req: &HideEventReq,
    hidden: bool,
) -> Response {
    match req.kind.as_str() {
        "issue_finished" => {
            // Issue-finished uses `progress_records.is_backfill` as
            // the hide flag (same semantic: exclude from the feed +
            // every time-bound activity surface). The source_id is
            // the issue id; we update every row for (user, issue)
            // since a user could have multiple progress rows in the
            // future (re-read sessions), and the feed currently keys
            // off the latest one anyway.
            let res = progress_record::Entity::update_many()
                .col_expr(progress_record::Column::IsBackfill, Expr::value(hidden))
                .filter(progress_record::Column::UserId.eq(user_id))
                .filter(progress_record::Column::IssueId.eq(req.source_id.clone()))
                .filter(progress_record::Column::FinishedAt.is_not_null())
                .exec(&app.db)
                .await;
            handle_update_result(res, "issue_finished")
        }
        "session_completed" => {
            let session_uuid = match Uuid::parse_str(&req.source_id) {
                Ok(u) => u,
                Err(_) => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "validation",
                        "source_id must be a session UUID",
                    );
                }
            };
            let am = reading_session::ActiveModel {
                id: Set(session_uuid),
                hidden_from_log: Set(hidden),
                ..Default::default()
            };
            // `update` returns 404 when no row matches the PK — but
            // we also need to enforce the user_id ACL. Do it via
            // `update_many` with the filter.
            let res = reading_session::Entity::update_many()
                .col_expr(reading_session::Column::HiddenFromLog, Expr::value(hidden))
                .filter(reading_session::Column::UserId.eq(user_id))
                .filter(reading_session::Column::Id.eq(session_uuid))
                .exec(&app.db)
                .await;
            drop(am);
            handle_update_result(res, "session_completed")
        }
        "marker_created" => {
            let marker_uuid = match Uuid::parse_str(&req.source_id) {
                Ok(u) => u,
                Err(_) => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "validation",
                        "source_id must be a marker UUID",
                    );
                }
            };
            let res = marker::Entity::update_many()
                .col_expr(marker::Column::HiddenFromLog, Expr::value(hidden))
                .filter(marker::Column::UserId.eq(user_id))
                .filter(marker::Column::Id.eq(marker_uuid))
                .exec(&app.db)
                .await;
            handle_update_result(res, "marker_created")
        }
        "series_finished" => error(
            StatusCode::BAD_REQUEST,
            "validation",
            "series_finished is a derived event and can't be hidden directly",
        ),
        _ => error(
            StatusCode::BAD_REQUEST,
            "validation",
            "kind must be issue_finished | session_completed | marker_created",
        ),
    }
}

fn handle_update_result(
    res: Result<sea_orm::UpdateResult, sea_orm::DbErr>,
    kind: &'static str,
) -> Response {
    match res {
        Ok(r) if r.rows_affected == 0 => error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no matching event for this user",
        ),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::warn!(error = %e, kind, "reading-log hide/unhide update failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}
