//! `POST /me/reading-sessions` — idempotent heartbeat upsert keyed on
//! `(user_id, client_session_id)`.
//! `GET  /me/reading-sessions` — cursor-paginated list.
//! `GET  /me/reading-stats`    — aggregated totals + per-day buckets in the
//! user's timezone, plus current/longest streak.
//!
//! M6a — captures intentional reading sessions client-side via 30s heartbeats
//! and a final flush on close. The user's preference fields gate the minimum
//! active duration / pages and host the opt-out kill switch.

use axum::{
    Extension, Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine;
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use chrono_tz::Tz;
use entity::{
    issue, library_user_access,
    reading_session::{self, ActiveModel as ReadingSessionAM, Entity as ReadingSessionEntity},
    series,
    user::Entity as UserEntity,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, QueryFilter,
    QueryOrder, QuerySelect, Set, Statement, Value,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::auth::CurrentUser;
use crate::middleware::RequestContext;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me/reading-sessions", post(upsert).get(list))
        .route("/me/reading-sessions/clear", post(clear_history))
        .route("/me/reading-stats", get(stats))
}

const MAX_CLIENT_SESSION_ID_LEN: usize = 64;
const FUTURE_SLACK_START_SECS: i64 = 5 * 60;
const FUTURE_SLACK_END_SECS: i64 = 60;
const PAST_LIMIT_SECS: i64 = 14 * 24 * 60 * 60;
const DEFAULT_LIMIT: u64 = 50;
const MAX_LIMIT: u64 = 200;

// ────────────── DTOs ──────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpsertReq {
    /// Client-generated UUID v4 (or any 1-64 char unique tag) used as the
    /// idempotency key across the heartbeat + final-flush sequence.
    pub client_session_id: String,
    pub issue_id: String,
    /// RFC 3339 timestamp.
    pub started_at: String,
    /// RFC 3339 timestamp; present only on the final flush.
    #[serde(default)]
    pub ended_at: Option<String>,
    pub active_ms: i64,
    pub distinct_pages_read: i32,
    pub page_turns: i32,
    pub start_page: i32,
    pub end_page: i32,
    #[serde(default)]
    pub device: Option<String>,
    /// `'single' | 'double' | 'webtoon'` or null.
    #[serde(default)]
    pub view_mode: Option<String>,
    #[serde(default)]
    pub client_meta: serde_json::Value,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ReadingSessionView {
    pub id: String,
    pub issue_id: String,
    pub series_id: String,
    pub client_session_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub last_heartbeat_at: String,
    pub active_ms: i64,
    pub distinct_pages_read: i32,
    pub page_turns: i32,
    pub start_page: i32,
    pub end_page: i32,
    pub furthest_page: i32,
    pub device: Option<String>,
    pub view_mode: Option<String>,
    /// Joined from `issues.title`. The list endpoint populates this; the
    /// upsert response leaves it null since the client doesn't render
    /// upsert results inline.
    #[serde(default)]
    pub issue_title: Option<String>,
    /// Joined from `issues.number_raw`.
    #[serde(default)]
    pub issue_number: Option<String>,
    /// Joined from `series.name`.
    #[serde(default)]
    pub series_name: Option<String>,
}

impl From<reading_session::Model> for ReadingSessionView {
    fn from(m: reading_session::Model) -> Self {
        Self {
            id: m.id.to_string(),
            issue_id: m.issue_id,
            series_id: m.series_id.to_string(),
            client_session_id: m.client_session_id,
            started_at: m.started_at.to_rfc3339(),
            ended_at: m.ended_at.map(|t| t.to_rfc3339()),
            last_heartbeat_at: m.last_heartbeat_at.to_rfc3339(),
            active_ms: m.active_ms,
            distinct_pages_read: m.distinct_pages_read,
            page_turns: m.page_turns,
            start_page: m.start_page,
            end_page: m.end_page,
            furthest_page: m.furthest_page,
            device: m.device,
            view_mode: m.view_mode,
            issue_title: None,
            issue_number: None,
            series_name: None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub issue_id: Option<String>,
    pub series_id: Option<String>,
    pub limit: Option<u64>,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ReadingSessionListView {
    pub records: Vec<ReadingSessionView>,
    /// Opaque cursor; pass back as `?cursor=` to fetch the next page.
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    /// `'7d' | '30d' | '90d' | 'all'`. Defaults to `'30d'`.
    pub range: Option<String>,
    pub issue_id: Option<String>,
    pub series_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ReadingStatsView {
    pub range: String,
    pub timezone: String,
    pub totals: TotalsView,
    pub per_day: Vec<DayBucket>,
    /// M6b — most-read series in the range. Capped at 10. Empty when the
    /// query is already issue-scoped (a single series is implied).
    #[serde(default)]
    pub top_series: Vec<TopSeriesEntry>,
    /// M6b — most-read genres derived from issue `genre` CSVs.
    #[serde(default)]
    pub top_genres: Vec<TopNameEntry>,
    /// M6b — most-read tags derived from issue `tags` CSVs.
    #[serde(default)]
    pub top_tags: Vec<TopNameEntry>,
    /// M6b — most-read publishers (series.publisher with issue.publisher fallback).
    #[serde(default)]
    pub top_publishers: Vec<TopNameEntry>,
    /// Stats v2 — most-read imprints (series.imprint).
    #[serde(default)]
    pub top_imprints: Vec<TopNameEntry>,
    /// Stats v2 — top creators across read series, partitioned per role
    /// (writer/penciller/inker/colorist/letterer/cover_artist). Up to 10 per
    /// role. Source: `series_credits`.
    #[serde(default)]
    pub top_creators: Vec<TopCreatorEntry>,
    /// Stats v2 — sparse 7×24 day-of-week × hour grid (in user's timezone).
    /// Only cells with sessions are emitted.
    #[serde(default)]
    pub dow_hour: Vec<DowHourCell>,
    /// Stats v2 — 4 time-of-day buckets (morning/afternoon/evening/night)
    /// derived from `dow_hour`.
    #[serde(default)]
    pub time_of_day: TimeOfDayBuckets,
    /// Stats v2 — per-session pace samples: `(started_at, sec_per_page)`.
    /// Sessions with `distinct_pages_read < 3` are excluded to drop noise.
    #[serde(default)]
    pub pace_series: Vec<PacePoint>,
    /// Stats v2 — top issues by read count (number of sessions). For
    /// "most rereads" displays.
    #[serde(default)]
    pub reread_top_issues: Vec<RereadIssueEntry>,
    /// Stats v2 — top series by total session count.
    #[serde(default)]
    pub reread_top_series: Vec<RereadSeriesEntry>,
    /// Stats v2 — distinct issues the user has read past the last page (or
    /// flagged finished in `progress_records`). Used with
    /// `totals.distinct_issues` to compute completion rate.
    pub completion: CompletionView,
    /// Stats v2 — first session's `started_at` within the scope.
    #[serde(default)]
    pub first_read_at: Option<String>,
    /// Stats v2 — last session's `started_at` within the scope.
    #[serde(default)]
    pub last_read_at: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TopSeriesEntry {
    pub series_id: String,
    pub name: String,
    pub sessions: i64,
    pub active_ms: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TopNameEntry {
    pub name: String,
    pub sessions: i64,
    pub active_ms: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TopCreatorEntry {
    /// `writer | penciller | inker | colorist | letterer | cover_artist | editor | translator`
    pub role: String,
    pub person: String,
    pub sessions: i64,
    pub active_ms: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DowHourCell {
    /// 0 = Sunday, 1 = Monday, …, 6 = Saturday. Matches Postgres
    /// `EXTRACT(DOW FROM …)` so the client doesn't need to remap.
    pub dow: i32,
    /// 0–23 in the user's timezone.
    pub hour: i32,
    pub sessions: i64,
    pub active_ms: i64,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct TimeOfDayBuckets {
    /// 05:00–11:59
    pub morning: TimeOfDayCell,
    /// 12:00–16:59
    pub afternoon: TimeOfDayCell,
    /// 17:00–21:59
    pub evening: TimeOfDayCell,
    /// 22:00–04:59
    pub night: TimeOfDayCell,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct TimeOfDayCell {
    pub sessions: i64,
    pub active_ms: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PacePoint {
    /// RFC 3339 timestamp of the session start.
    pub started_at: String,
    /// `active_ms / (1000 * distinct_pages_read)` — average seconds per
    /// distinct page within that session.
    pub sec_per_page: f64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RereadIssueEntry {
    pub issue_id: String,
    /// Issue title (may be null in metadata) or the parsed number_raw fallback.
    pub title: Option<String>,
    pub number_raw: Option<String>,
    pub series_id: String,
    pub series_name: String,
    pub reads: i64,
    pub active_ms: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RereadSeriesEntry {
    pub series_id: String,
    pub name: String,
    /// Distinct issues from this series the user has read.
    pub distinct_issues: i64,
    /// Total sessions (reads) across the series.
    pub reads: i64,
    pub active_ms: i64,
}

#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct CompletionView {
    /// Issues completed (progress.finished OR end_page >= page_count - 1).
    pub completed: i64,
    /// Issues touched (distinct issue_ids in scope).
    pub started: i64,
    /// `completed / started`. 0.0 when `started == 0`.
    pub rate: f64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TotalsView {
    pub sessions: i64,
    pub active_ms: i64,
    pub distinct_pages_read: i64,
    pub distinct_issues: i64,
    /// Days within the range with at least one session.
    pub days_active: i64,
    /// Consecutive days ending today with activity (global, ignores range).
    pub current_streak: i64,
    /// Longest run of consecutive active days ever (global).
    pub longest_streak: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DayBucket {
    /// `YYYY-MM-DD` in the user's timezone.
    pub date: String,
    pub sessions: i64,
    pub active_ms: i64,
    pub pages: i64,
}

// ────────────── Handlers ──────────────

#[utoipa::path(
    post,
    path = "/me/reading-sessions",
    request_body = UpsertReq,
    responses(
        (status = 201, body = ReadingSessionView, description = "new session row created"),
        (status = 200, body = ReadingSessionView, description = "existing row updated"),
        (status = 204, description = "discarded (opt-out or below threshold)"),
        (status = 400, description = "validation error"),
        (status = 404, description = "issue not visible to user"),
    )
)]
pub async fn upsert(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<UpsertReq>,
) -> Response {
    if req.client_session_id.is_empty() || req.client_session_id.len() > MAX_CLIENT_SESSION_ID_LEN {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "client_session_id required (1-64 chars)",
        );
    }
    if req.start_page < 0 || req.end_page < req.start_page {
        return error(StatusCode::BAD_REQUEST, "validation", "page range invalid");
    }
    if req.distinct_pages_read < 0 || req.page_turns < 0 || req.active_ms < 0 {
        return error(StatusCode::BAD_REQUEST, "validation", "negative counter");
    }

    let started_at = match DateTime::parse_from_rfc3339(&req.started_at) {
        Ok(t) => t,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "started_at must be RFC3339",
            );
        }
    };
    let ended_at = match req.ended_at.as_deref() {
        Some(s) => match DateTime::parse_from_rfc3339(s) {
            Ok(t) => Some(t),
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "validation",
                    "ended_at must be RFC3339",
                );
            }
        },
        None => None,
    };
    let now = Utc::now();
    if started_at.with_timezone(&Utc) > now + Duration::seconds(FUTURE_SLACK_START_SECS) {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "started_at in future",
        );
    }
    if started_at.with_timezone(&Utc) < now - Duration::seconds(PAST_LIMIT_SECS) {
        return error(StatusCode::BAD_REQUEST, "validation", "started_at too old");
    }
    if let Some(et) = ended_at {
        if et < started_at {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "ended_at before started_at",
            );
        }
        if et.with_timezone(&Utc) > now + Duration::seconds(FUTURE_SLACK_END_SECS) {
            return error(StatusCode::BAD_REQUEST, "validation", "ended_at in future");
        }
    }
    let view_mode = match req.view_mode.as_deref() {
        Some(v) if matches!(v, "single" | "double" | "webtoon") => Some(v.to_owned()),
        Some(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "view_mode must be 'single', 'double', 'webtoon', or null",
            );
        }
        None => None,
    };
    let client_meta = if req.client_meta.is_null() {
        serde_json::json!({})
    } else if req.client_meta.is_object() {
        // Cap the JSON size so a misbehaving client can't stuff KBs of metadata
        // into every heartbeat.
        let s = req.client_meta.to_string();
        if s.len() > 1024 {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "client_meta too large (>1KB)",
            );
        }
        req.client_meta
    } else {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "client_meta must be an object",
        );
    };

    // Honor the user's opt-out and threshold prefs. Look up once.
    let user_row = match UserEntity::find_by_id(user.id).one(&app.db).await {
        Ok(Some(r)) => r,
        _ => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    if !user_row.activity_tracking_enabled {
        // Silently discard so a client that hasn't picked up the toggle yet
        // doesn't spam the user with 4xx toasts.
        return StatusCode::NO_CONTENT.into_response();
    }
    // A finalized session below threshold is dropped without persistence. We
    // intentionally allow in-flight heartbeats below threshold to survive so a
    // session that *will* cross the threshold can grow into it.
    if ended_at.is_some()
        && (req.active_ms < user_row.reading_min_active_ms as i64
            || req.distinct_pages_read < user_row.reading_min_pages)
    {
        return StatusCode::NO_CONTENT.into_response();
    }

    // ACL: confirm the user can see the issue.
    let issue_row = match issue::Entity::find_by_id(req.issue_id.clone())
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "issue not found"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    if !visible(&app, &user, issue_row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    let furthest = req.end_page.max(req.start_page);
    let now_off = now.fixed_offset();
    let started_off = started_at.with_timezone(&FixedOffset::east_opt(0).unwrap());
    let ended_off = ended_at.map(|t| t.with_timezone(&FixedOffset::east_opt(0).unwrap()));

    let span = tracing::info_span!(
        "reading.session.commit",
        user_id = %user.id,
        issue_id = %req.issue_id,
        client_session_id = %req.client_session_id,
    );
    let _enter = span.enter();

    let existing = ReadingSessionEntity::find()
        .filter(reading_session::Column::UserId.eq(user.id))
        .filter(reading_session::Column::ClientSessionId.eq(req.client_session_id.clone()))
        .one(&app.db)
        .await;

    let (status, model) = match existing {
        Ok(Some(prior)) => {
            // Heartbeat or final-close on an existing session. Take MAX of all
            // monotonic counters; expand the start_page/end_page envelope.
            let prior_id = prior.id;
            let prior_ended = prior.ended_at;
            let mut am: ReadingSessionAM = prior.clone().into();
            am.active_ms = Set(prior.active_ms.max(req.active_ms));
            am.distinct_pages_read = Set(prior.distinct_pages_read.max(req.distinct_pages_read));
            am.page_turns = Set(prior.page_turns.max(req.page_turns));
            am.start_page = Set(prior.start_page.min(req.start_page));
            am.end_page = Set(prior.end_page.max(req.end_page));
            am.furthest_page = Set(prior.furthest_page.max(furthest));
            am.last_heartbeat_at = Set(now_off);
            // Only set ended_at forward; never re-open a closed session.
            if prior_ended.is_none() && ended_off.is_some() {
                am.ended_at = Set(ended_off);
            }
            if view_mode.is_some() {
                am.view_mode = Set(view_mode.clone());
            }
            if req.device.is_some() {
                am.device = Set(req.device.clone());
            }
            am.client_meta = Set(client_meta);
            match am.update(&app.db).await {
                Ok(m) => (StatusCode::OK, m),
                Err(e) => {
                    tracing::warn!(error = %e, session_id = %prior_id, "reading_session update failed");
                    return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
                }
            }
        }
        Ok(None) => {
            // First write for this client_session_id.
            let new_id = Uuid::now_v7();
            let am = ReadingSessionAM {
                id: Set(new_id),
                user_id: Set(user.id),
                issue_id: Set(req.issue_id.clone()),
                series_id: Set(issue_row.series_id),
                client_session_id: Set(req.client_session_id.clone()),
                started_at: Set(started_off),
                ended_at: Set(ended_off),
                last_heartbeat_at: Set(now_off),
                active_ms: Set(req.active_ms),
                distinct_pages_read: Set(req.distinct_pages_read),
                page_turns: Set(req.page_turns),
                start_page: Set(req.start_page),
                end_page: Set(req.end_page),
                furthest_page: Set(furthest),
                device: Set(req.device.clone()),
                view_mode: Set(view_mode),
                client_meta: Set(client_meta),
            };
            match am.insert(&app.db).await {
                Ok(m) => (StatusCode::CREATED, m),
                Err(e) => {
                    tracing::warn!(error = %e, "reading_session insert failed");
                    return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
                }
            }
        }
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };

    let view: ReadingSessionView = model.into();
    (status, Json(view)).into_response()
}

#[utoipa::path(
    get,
    path = "/me/reading-sessions",
    params(
        ("issue_id" = Option<String>, Query, description = "filter to one issue"),
        ("series_id" = Option<String>, Query, description = "filter to one series"),
        ("limit" = Option<u64>, Query, description = "1-200 (default 50)"),
        ("cursor" = Option<String>, Query, description = "opaque cursor from a prior response"),
    ),
    responses(
        (status = 200, body = ReadingSessionListView),
        (status = 400, description = "validation error"),
    )
)]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut query = ReadingSessionEntity::find()
        .filter(reading_session::Column::UserId.eq(user.id))
        .order_by_desc(reading_session::Column::StartedAt)
        .order_by_desc(reading_session::Column::Id)
        .limit(Some(limit + 1));

    if let Some(iss) = q.issue_id.as_deref() {
        query = query.filter(reading_session::Column::IssueId.eq(iss));
    }
    if let Some(sid) = q.series_id.as_deref() {
        let Ok(uuid) = Uuid::parse_str(sid) else {
            return error(StatusCode::BAD_REQUEST, "validation", "invalid series_id");
        };
        query = query.filter(reading_session::Column::SeriesId.eq(uuid));
    }
    if let Some(c) = q.cursor.as_deref() {
        match decode_cursor(c) {
            Some((started_at, id)) => {
                use sea_orm::sea_query::{Cond, Expr};
                query = query.filter(
                    Cond::any()
                        .add(reading_session::Column::StartedAt.lt(started_at))
                        .add(
                            Cond::all()
                                .add(reading_session::Column::StartedAt.eq(started_at))
                                .add(Expr::col(reading_session::Column::Id).lt(id)),
                        ),
                );
            }
            None => {
                return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor");
            }
        }
    }

    let mut rows = match query.all(&app.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "reading_sessions list failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let next_cursor = if rows.len() as u64 > limit {
        rows.truncate(limit as usize);
        rows.last()
            .map(|last| encode_cursor(&last.started_at, &last.id))
    } else {
        None
    };

    // Batch-fetch the labels (issue title/number, series name) so the
    // timeline can render `Series #N · Title` instead of the raw BLAKE3
    // hash. Two extra queries, each bounded by `limit`.
    let issue_ids: HashSet<String> = rows.iter().map(|r| r.issue_id.clone()).collect();
    let series_ids: HashSet<Uuid> = rows.iter().map(|r| r.series_id).collect();

    let issue_map = if issue_ids.is_empty() {
        HashMap::new()
    } else {
        issue::Entity::find()
            .filter(issue::Column::Id.is_in(issue_ids))
            .all(&app.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|i| (i.id.clone(), i))
            .collect::<HashMap<_, _>>()
    };
    let series_map = if series_ids.is_empty() {
        HashMap::new()
    } else {
        series::Entity::find()
            .filter(series::Column::Id.is_in(series_ids))
            .all(&app.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.id, s))
            .collect::<HashMap<_, _>>()
    };

    let records: Vec<ReadingSessionView> = rows
        .into_iter()
        .map(|m| {
            let issue_row = issue_map.get(&m.issue_id);
            let series_row = series_map.get(&m.series_id);
            let mut view: ReadingSessionView = m.into();
            view.issue_title = issue_row.and_then(|i| i.title.clone());
            view.issue_number = issue_row.and_then(|i| i.number_raw.clone());
            view.series_name = series_row.map(|s| s.name.clone());
            view
        })
        .collect();
    Json(ReadingSessionListView {
        records,
        next_cursor,
    })
    .into_response()
}

#[utoipa::path(
    get,
    path = "/me/reading-stats",
    params(
        ("range" = Option<String>, Query, description = "'7d', '30d', '90d', 'all'"),
        ("issue_id" = Option<String>, Query),
        ("series_id" = Option<String>, Query),
    ),
    responses(
        (status = 200, body = ReadingStatsView),
        (status = 400, description = "validation error"),
    )
)]
pub async fn stats(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<StatsQuery>,
) -> Response {
    match compute_stats_for_user(&app, user.id, q).await {
        Ok(view) => Json(view).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ClearHistoryResp {
    pub deleted: i64,
}

#[utoipa::path(
    post,
    path = "/me/reading-sessions/clear",
    responses(
        (status = 200, body = ClearHistoryResp),
    ),
    description = "Destructive — deletes ALL of the caller's reading_sessions rows. Audited."
)]
pub async fn clear_history(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
) -> Response {
    use sea_orm::EntityTrait;
    let res = ReadingSessionEntity::delete_many()
        .filter(reading_session::Column::UserId.eq(user.id))
        .exec(&app.db)
        .await;
    let deleted = match res {
        Ok(r) => r.rows_affected as i64,
        Err(e) => {
            tracing::warn!(error = %e, "reading_sessions clear failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    crate::audit::record(
        &app.db,
        crate::audit::AuditEntry {
            actor_id: user.id,
            action: "me.activity.history.clear",
            target_type: Some("user"),
            target_id: Some(user.id.to_string()),
            payload: serde_json::json!({ "deleted": deleted }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    Json(ClearHistoryResp { deleted }).into_response()
}

/// Reusable stats computation. Used by `GET /me/reading-stats` (callee = self)
/// and `GET /admin/users/{id}/reading-stats` (callee = admin acting on a
/// target user). Validation + error mapping lives here so both endpoints stay
/// thin.
pub async fn compute_stats_for_user(
    app: &AppState,
    user_id: Uuid,
    q: StatsQuery,
) -> Result<ReadingStatsView, StatsError> {
    let range = q.range.as_deref().unwrap_or("30d").to_string();
    let lookback = match range.as_str() {
        "7d" => Some(Duration::days(7)),
        "30d" => Some(Duration::days(30)),
        "60d" => Some(Duration::days(60)),
        "90d" => Some(Duration::days(90)),
        "1y" => Some(Duration::days(365)),
        "all" => None,
        _ => {
            return Err(StatsError::bad(
                "validation",
                "range must be '7d', '30d', '60d', '90d', '1y', or 'all'",
            ));
        }
    };

    let user_row = match UserEntity::find_by_id(user_id).one(&app.db).await {
        Ok(Some(r)) => r,
        _ => return Err(StatsError::internal()),
    };
    let tz: Tz = user_row.timezone.parse().unwrap_or(chrono_tz::UTC);

    let issue_filter = q.issue_id.clone();
    let series_filter = match q.series_id.as_deref() {
        Some(s) => match Uuid::parse_str(s) {
            Ok(u) => Some(u),
            Err(_) => return Err(StatsError::bad("validation", "invalid series_id")),
        },
        None => None,
    };
    let since: Option<DateTime<FixedOffset>> = lookback.map(|d| (Utc::now() - d).fixed_offset());

    let backend = app.db.get_database_backend();

    // ── Totals (in-range) ──
    let mut totals_sql = String::from(
        "SELECT \
           COUNT(*)::bigint AS sessions, \
           COALESCE(SUM(active_ms),0)::bigint AS active_ms, \
           COALESCE(SUM(distinct_pages_read),0)::bigint AS distinct_pages_read, \
           COUNT(DISTINCT issue_id)::bigint AS distinct_issues \
         FROM reading_sessions \
         WHERE user_id = $1",
    );
    let mut totals_params: Vec<Value> = vec![user_id.into()];
    append_scope(
        &mut totals_sql,
        &mut totals_params,
        2,
        "",
        since.as_ref(),
        issue_filter.as_deref(),
        series_filter.as_ref(),
    );

    #[derive(FromQueryResult)]
    struct TotalsRow {
        sessions: i64,
        active_ms: i64,
        distinct_pages_read: i64,
        distinct_issues: i64,
    }

    let totals_row = match TotalsRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        &totals_sql,
        totals_params,
    ))
    .one(&app.db)
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => TotalsRow {
            sessions: 0,
            active_ms: 0,
            distinct_pages_read: 0,
            distinct_issues: 0,
        },
        Err(e) => {
            tracing::warn!(error = %e, "reading stats totals failed");
            return Err(StatsError::internal());
        }
    };

    // ── Per-day (in-range), bucketed in user's tz ──
    let mut day_sql = String::from(
        "SELECT \
           to_char((started_at AT TIME ZONE $2)::date, 'YYYY-MM-DD') AS date, \
           COUNT(*)::bigint AS sessions, \
           COALESCE(SUM(active_ms),0)::bigint AS active_ms, \
           COALESCE(SUM(distinct_pages_read),0)::bigint AS pages \
         FROM reading_sessions \
         WHERE user_id = $1",
    );
    let mut day_params: Vec<Value> = vec![user_id.into(), tz.name().to_string().into()];
    append_scope(
        &mut day_sql,
        &mut day_params,
        3,
        "",
        since.as_ref(),
        issue_filter.as_deref(),
        series_filter.as_ref(),
    );
    day_sql.push_str(" GROUP BY 1 ORDER BY 1 ASC");

    #[derive(FromQueryResult)]
    struct DayRow {
        date: String,
        sessions: i64,
        active_ms: i64,
        pages: i64,
    }

    let day_rows = match DayRow::find_by_statement(Statement::from_sql_and_values(
        backend, &day_sql, day_params,
    ))
    .all(&app.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "reading stats per-day failed");
            return Err(StatsError::internal());
        }
    };
    let per_day: Vec<DayBucket> = day_rows
        .into_iter()
        .map(|r| DayBucket {
            date: r.date,
            sessions: r.sessions,
            active_ms: r.active_ms,
            pages: r.pages,
        })
        .collect();
    let days_active = per_day.len() as i64;

    // ── Distinct active days globally (no range filter) for streak math ──
    let streak_sql = "SELECT DISTINCT (started_at AT TIME ZONE $2)::date AS d \
         FROM reading_sessions \
         WHERE user_id = $1 \
         ORDER BY d ASC";

    #[derive(FromQueryResult)]
    struct StreakRow {
        d: NaiveDate,
    }
    let streak_rows = match StreakRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        streak_sql,
        vec![user_id.into(), tz.name().to_string().into()],
    ))
    .all(&app.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "reading stats streak failed");
            return Err(StatsError::internal());
        }
    };
    let active_days: Vec<NaiveDate> = streak_rows.into_iter().map(|r| r.d).collect();
    let today_local = Utc::now().with_timezone(&tz).date_naive();
    let (current_streak, longest_streak) = streak_lengths(&active_days, today_local);

    // ── Top-N rankings (M6b) ─────────────────────────────────────────────
    // The rule of thumb: only emit rankings whose dimension *can* vary
    // within the requested scope. Series and issue scopes have a single
    // implied series + publisher, so those rankings would be tautological.
    // Issue scope additionally has a single set of genres/tags (already on
    // the issue's Genres tab), so we skip those too.
    let scope_is_series_or_issue = issue_filter.is_some() || series_filter.is_some();
    let scope_is_issue = issue_filter.is_some();

    let top_series = if scope_is_series_or_issue {
        Vec::new()
    } else {
        let mut sql = String::from(
            "SELECT s.id::text AS series_id, s.name AS name, \
               COUNT(*)::bigint AS sessions, \
               COALESCE(SUM(rs.active_ms),0)::bigint AS active_ms \
             FROM reading_sessions rs \
             JOIN series s ON s.id = rs.series_id \
             WHERE rs.user_id = $1",
        );
        let mut params: Vec<Value> = vec![user_id.into()];
        append_scope(
            &mut sql,
            &mut params,
            2,
            "rs.",
            since.as_ref(),
            issue_filter.as_deref(),
            series_filter.as_ref(),
        );
        sql.push_str(" GROUP BY s.id, s.name ORDER BY active_ms DESC, sessions DESC LIMIT 10");

        #[derive(FromQueryResult)]
        struct Row {
            series_id: String,
            name: String,
            sessions: i64,
            active_ms: i64,
        }
        match Row::find_by_statement(Statement::from_sql_and_values(backend, &sql, params))
            .all(&app.db)
            .await
        {
            Ok(rows) => rows
                .into_iter()
                .map(|r| TopSeriesEntry {
                    series_id: r.series_id,
                    name: r.name,
                    sessions: r.sessions,
                    active_ms: r.active_ms,
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "reading stats top_series failed");
                Vec::new()
            }
        }
    };

    let top_genres = if scope_is_issue {
        Vec::new()
    } else {
        top_csv_column(
            app,
            backend,
            user_id,
            "i.genre",
            since.as_ref(),
            issue_filter.as_deref(),
            series_filter.as_ref(),
        )
        .await
    };

    let top_tags = if scope_is_issue {
        Vec::new()
    } else {
        top_csv_column(
            app,
            backend,
            user_id,
            "i.tags",
            since.as_ref(),
            issue_filter.as_deref(),
            series_filter.as_ref(),
        )
        .await
    };

    let top_publishers = if scope_is_series_or_issue {
        Vec::new()
    } else {
        let mut sql = String::from(
            "SELECT COALESCE(NULLIF(s.publisher,''), NULLIF(i.publisher,'')) AS name, \
               COUNT(*)::bigint AS sessions, \
               COALESCE(SUM(rs.active_ms),0)::bigint AS active_ms \
             FROM reading_sessions rs \
             JOIN issues i ON i.id = rs.issue_id \
             JOIN series s ON s.id = rs.series_id \
             WHERE rs.user_id = $1",
        );
        let mut params: Vec<Value> = vec![user_id.into()];
        append_scope(
            &mut sql,
            &mut params,
            2,
            "rs.",
            since.as_ref(),
            issue_filter.as_deref(),
            series_filter.as_ref(),
        );
        // Filter out NULL publishers post-COALESCE.
        sql.push_str(
            " AND COALESCE(NULLIF(s.publisher,''), NULLIF(i.publisher,'')) IS NOT NULL \
             GROUP BY 1 ORDER BY active_ms DESC, sessions DESC LIMIT 10",
        );

        #[derive(FromQueryResult)]
        struct Row {
            name: String,
            sessions: i64,
            active_ms: i64,
        }
        match Row::find_by_statement(Statement::from_sql_and_values(backend, &sql, params))
            .all(&app.db)
            .await
        {
            Ok(rows) => rows
                .into_iter()
                .map(|r| TopNameEntry {
                    name: r.name,
                    sessions: r.sessions,
                    active_ms: r.active_ms,
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "reading stats top_publishers failed");
                Vec::new()
            }
        }
    };

    // ── Stats v2 enrichments ─────────────────────────────────────────────

    let top_imprints = if scope_is_series_or_issue {
        Vec::new()
    } else {
        top_column_with_alias(
            app,
            backend,
            user_id,
            "s.imprint",
            "JOIN series s ON s.id = rs.series_id",
            since.as_ref(),
            issue_filter.as_deref(),
            series_filter.as_ref(),
        )
        .await
    };

    let top_creators = compute_top_creators(
        app,
        backend,
        user_id,
        since.as_ref(),
        issue_filter.as_deref(),
        series_filter.as_ref(),
    )
    .await;

    let dow_hour = compute_dow_hour(
        app,
        backend,
        user_id,
        tz,
        since.as_ref(),
        issue_filter.as_deref(),
        series_filter.as_ref(),
    )
    .await;
    let time_of_day = time_of_day_from(&dow_hour);

    let pace_series = compute_pace_series(
        app,
        backend,
        user_id,
        since.as_ref(),
        issue_filter.as_deref(),
        series_filter.as_ref(),
    )
    .await;

    // When scope is series, the grid heatmap wants every issue the user
    // has touched in that series (not just top 10). For all-scope and
    // issue-scope, stay capped at 10.
    let reread_limit = if series_filter.is_some() { 500 } else { 10 };
    let reread_top_issues = compute_reread_top_issues(
        app,
        backend,
        user_id,
        since.as_ref(),
        issue_filter.as_deref(),
        series_filter.as_ref(),
        reread_limit,
    )
    .await;

    let reread_top_series = if scope_is_series_or_issue {
        Vec::new()
    } else {
        compute_reread_top_series(app, backend, user_id, since.as_ref()).await
    };

    let completion = compute_completion(
        app,
        backend,
        user_id,
        since.as_ref(),
        issue_filter.as_deref(),
        series_filter.as_ref(),
    )
    .await;

    let (first_read_at, last_read_at) = compute_first_last(
        app,
        backend,
        user_id,
        since.as_ref(),
        issue_filter.as_deref(),
        series_filter.as_ref(),
    )
    .await;

    Ok(ReadingStatsView {
        range,
        timezone: tz.name().to_string(),
        totals: TotalsView {
            sessions: totals_row.sessions,
            active_ms: totals_row.active_ms,
            distinct_pages_read: totals_row.distinct_pages_read,
            distinct_issues: totals_row.distinct_issues,
            days_active,
            current_streak,
            longest_streak,
        },
        per_day,
        top_series,
        top_genres,
        top_tags,
        top_publishers,
        top_imprints,
        top_creators,
        dow_hour,
        time_of_day,
        pace_series,
        reread_top_issues,
        reread_top_series,
        completion,
        first_read_at,
        last_read_at,
    })
}

/// Error type for [`compute_stats_for_user`]. Carries the HTTP status, error
/// code, and message so callers can either propagate via `into_response()` or
/// embed the failure into a richer admin-side response.
pub struct StatsError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl StatsError {
    pub fn bad(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }
    pub fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: "internal".into(),
        }
    }
    pub fn into_response(self) -> Response {
        error(self.status, self.code, &self.message)
    }
}

// ────────────── Helpers ──────────────

/// Append the standard scope filter (since / issue / series) to a SQL
/// builder, mutating `params` to include any newly-bound values. `prefix`
/// is the table alias to prepend to the column references — `""` for the
/// non-joined queries (totals + per-day) and `"rs."` for the top-N
/// queries that join through `reading_sessions rs`.
fn append_scope(
    sql: &mut String,
    params: &mut Vec<Value>,
    start_idx: usize,
    prefix: &str,
    since: Option<&DateTime<FixedOffset>>,
    issue_filter: Option<&str>,
    series_filter: Option<&Uuid>,
) -> usize {
    let mut idx = start_idx;
    if let Some(s) = since {
        sql.push_str(&format!(" AND {prefix}started_at >= ${idx}"));
        params.push((*s).into());
        idx += 1;
    }
    if let Some(iss) = issue_filter {
        sql.push_str(&format!(" AND {prefix}issue_id = ${idx}"));
        params.push(iss.to_string().into());
        idx += 1;
    }
    if let Some(sid) = series_filter {
        sql.push_str(&format!(" AND {prefix}series_id = ${idx}"));
        params.push((*sid).into());
        idx += 1;
    }
    idx
}

/// Top-N for a scalar column reachable from `reading_sessions rs` via an
/// extra `JOIN` clause. `column_expr` is `SELECT`-side; `join` is appended
/// to the FROM. Filters out NULL/empty after coalesce.
#[allow(clippy::too_many_arguments)]
async fn top_column_with_alias(
    app: &AppState,
    backend: sea_orm::DbBackend,
    user_id: Uuid,
    column_expr: &str,
    join: &str,
    since: Option<&DateTime<FixedOffset>>,
    issue_filter: Option<&str>,
    series_filter: Option<&Uuid>,
) -> Vec<TopNameEntry> {
    let mut sql = format!(
        "SELECT NULLIF(TRIM(COALESCE({column_expr}, '')), '') AS name, \
           COUNT(*)::bigint AS sessions, \
           COALESCE(SUM(rs.active_ms),0)::bigint AS active_ms \
         FROM reading_sessions rs \
         {join} \
         WHERE rs.user_id = $1",
    );
    let mut params: Vec<Value> = vec![user_id.into()];
    append_scope(
        &mut sql,
        &mut params,
        2,
        "rs.",
        since,
        issue_filter,
        series_filter,
    );
    sql.push_str(" AND NULLIF(TRIM(COALESCE(");
    sql.push_str(column_expr);
    sql.push_str(
        ", '')), '') IS NOT NULL \
         GROUP BY 1 ORDER BY active_ms DESC, sessions DESC LIMIT 10",
    );

    #[derive(FromQueryResult)]
    struct Row {
        name: String,
        sessions: i64,
        active_ms: i64,
    }
    match Row::find_by_statement(Statement::from_sql_and_values(backend, &sql, params))
        .all(&app.db)
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|r| TopNameEntry {
                name: r.name,
                sessions: r.sessions,
                active_ms: r.active_ms,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, column = column_expr, "top_column_with_alias failed");
            Vec::new()
        }
    }
}

/// Top creators across read series, partitioned per role. Uses a window
/// function so each role gets its own top-10. Single SQL round-trip.
async fn compute_top_creators(
    app: &AppState,
    backend: sea_orm::DbBackend,
    user_id: Uuid,
    since: Option<&DateTime<FixedOffset>>,
    issue_filter: Option<&str>,
    series_filter: Option<&Uuid>,
) -> Vec<TopCreatorEntry> {
    let mut inner = String::from(
        "SELECT sc.role AS role, sc.person AS person, \
           COUNT(*)::bigint AS sessions, \
           COALESCE(SUM(rs.active_ms),0)::bigint AS active_ms \
         FROM reading_sessions rs \
         JOIN series_credits sc ON sc.series_id = rs.series_id \
         WHERE rs.user_id = $1",
    );
    let mut params: Vec<Value> = vec![user_id.into()];
    append_scope(
        &mut inner,
        &mut params,
        2,
        "rs.",
        since,
        issue_filter,
        series_filter,
    );
    inner.push_str(" GROUP BY sc.role, sc.person");

    let sql = format!(
        "SELECT role, person, sessions, active_ms FROM ( \
           SELECT t.*, ROW_NUMBER() OVER (PARTITION BY role ORDER BY active_ms DESC, sessions DESC) AS rn \
           FROM ({inner}) t \
         ) ranked WHERE rn <= 10 ORDER BY role ASC, active_ms DESC, sessions DESC"
    );

    #[derive(FromQueryResult)]
    struct Row {
        role: String,
        person: String,
        sessions: i64,
        active_ms: i64,
    }
    match Row::find_by_statement(Statement::from_sql_and_values(backend, &sql, params))
        .all(&app.db)
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|r| TopCreatorEntry {
                role: r.role,
                person: r.person,
                sessions: r.sessions,
                active_ms: r.active_ms,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "compute_top_creators failed");
            Vec::new()
        }
    }
}

/// 7×24 sparse grid of day-of-week × hour. `dow` follows Postgres EXTRACT:
/// 0 = Sunday … 6 = Saturday. Times bucketed in user's timezone.
async fn compute_dow_hour(
    app: &AppState,
    backend: sea_orm::DbBackend,
    user_id: Uuid,
    tz: Tz,
    since: Option<&DateTime<FixedOffset>>,
    issue_filter: Option<&str>,
    series_filter: Option<&Uuid>,
) -> Vec<DowHourCell> {
    let mut sql = String::from(
        "SELECT EXTRACT(DOW FROM (started_at AT TIME ZONE $2))::int AS dow, \
           EXTRACT(HOUR FROM (started_at AT TIME ZONE $2))::int AS hour, \
           COUNT(*)::bigint AS sessions, \
           COALESCE(SUM(active_ms),0)::bigint AS active_ms \
         FROM reading_sessions \
         WHERE user_id = $1",
    );
    let mut params: Vec<Value> = vec![user_id.into(), tz.name().to_string().into()];
    append_scope(
        &mut sql,
        &mut params,
        3,
        "",
        since,
        issue_filter,
        series_filter,
    );
    sql.push_str(" GROUP BY 1, 2 ORDER BY 1, 2");

    #[derive(FromQueryResult)]
    struct Row {
        dow: i32,
        hour: i32,
        sessions: i64,
        active_ms: i64,
    }
    match Row::find_by_statement(Statement::from_sql_and_values(backend, &sql, params))
        .all(&app.db)
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|r| DowHourCell {
                dow: r.dow,
                hour: r.hour,
                sessions: r.sessions,
                active_ms: r.active_ms,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "compute_dow_hour failed");
            Vec::new()
        }
    }
}

/// Roll the dow_hour grid into 4 time-of-day buckets:
/// morning 05-11, afternoon 12-16, evening 17-21, night 22-04.
fn time_of_day_from(cells: &[DowHourCell]) -> TimeOfDayBuckets {
    let mut buckets = TimeOfDayBuckets::default();
    for c in cells {
        let bucket = match c.hour {
            5..=11 => &mut buckets.morning,
            12..=16 => &mut buckets.afternoon,
            17..=21 => &mut buckets.evening,
            _ => &mut buckets.night,
        };
        bucket.sessions += c.sessions;
        bucket.active_ms += c.active_ms;
    }
    buckets
}

/// Per-session pace samples (sec/page) in scope. Sessions with
/// `distinct_pages_read < 3` are filtered to drop noise.
async fn compute_pace_series(
    app: &AppState,
    backend: sea_orm::DbBackend,
    user_id: Uuid,
    since: Option<&DateTime<FixedOffset>>,
    issue_filter: Option<&str>,
    series_filter: Option<&Uuid>,
) -> Vec<PacePoint> {
    let mut sql = String::from(
        "SELECT started_at AS started_at, \
           (active_ms::float8 / NULLIF(distinct_pages_read, 0)::float8 / 1000.0) AS sec_per_page \
         FROM reading_sessions \
         WHERE user_id = $1 AND distinct_pages_read >= 3 AND active_ms > 0",
    );
    let mut params: Vec<Value> = vec![user_id.into()];
    append_scope(
        &mut sql,
        &mut params,
        2,
        "",
        since,
        issue_filter,
        series_filter,
    );
    sql.push_str(" ORDER BY started_at ASC LIMIT 1000");

    #[derive(FromQueryResult)]
    struct Row {
        started_at: DateTime<FixedOffset>,
        sec_per_page: f64,
    }
    match Row::find_by_statement(Statement::from_sql_and_values(backend, &sql, params))
        .all(&app.db)
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|r| PacePoint {
                started_at: r.started_at.to_rfc3339(),
                sec_per_page: r.sec_per_page,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "compute_pace_series failed");
            Vec::new()
        }
    }
}

/// Top reread issues — most sessions per issue, with title + series labels.
/// `limit` caps the result; series-scoped requests pass a large value so the
/// grid heatmap on the series page can color every issue the user has read.
#[allow(clippy::too_many_arguments)]
async fn compute_reread_top_issues(
    app: &AppState,
    backend: sea_orm::DbBackend,
    user_id: Uuid,
    since: Option<&DateTime<FixedOffset>>,
    issue_filter: Option<&str>,
    series_filter: Option<&Uuid>,
    limit: i64,
) -> Vec<RereadIssueEntry> {
    let mut sql = String::from(
        "SELECT rs.issue_id::text AS issue_id, i.title AS title, i.number_raw AS number_raw, \
           s.id::text AS series_id, s.name AS series_name, \
           COUNT(*)::bigint AS reads, \
           COALESCE(SUM(rs.active_ms),0)::bigint AS active_ms \
         FROM reading_sessions rs \
         JOIN issues i ON i.id = rs.issue_id \
         JOIN series s ON s.id = rs.series_id \
         WHERE rs.user_id = $1",
    );
    let mut params: Vec<Value> = vec![user_id.into()];
    append_scope(
        &mut sql,
        &mut params,
        2,
        "rs.",
        since,
        issue_filter,
        series_filter,
    );
    sql.push_str(&format!(
        " GROUP BY rs.issue_id, i.title, i.number_raw, s.id, s.name \
         ORDER BY reads DESC, active_ms DESC LIMIT {limit}",
    ));

    #[derive(FromQueryResult)]
    struct Row {
        issue_id: String,
        title: Option<String>,
        number_raw: Option<String>,
        series_id: String,
        series_name: String,
        reads: i64,
        active_ms: i64,
    }
    match Row::find_by_statement(Statement::from_sql_and_values(backend, &sql, params))
        .all(&app.db)
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|r| RereadIssueEntry {
                issue_id: r.issue_id,
                title: r.title,
                number_raw: r.number_raw,
                series_id: r.series_id,
                series_name: r.series_name,
                reads: r.reads,
                active_ms: r.active_ms,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "compute_reread_top_issues failed");
            Vec::new()
        }
    }
}

/// Top reread series — most total sessions per series, with distinct issue
/// count. Skipped when scope is issue/series (tautological).
async fn compute_reread_top_series(
    app: &AppState,
    backend: sea_orm::DbBackend,
    user_id: Uuid,
    since: Option<&DateTime<FixedOffset>>,
) -> Vec<RereadSeriesEntry> {
    let mut sql = String::from(
        "SELECT s.id::text AS series_id, s.name AS name, \
           COUNT(DISTINCT rs.issue_id)::bigint AS distinct_issues, \
           COUNT(*)::bigint AS reads, \
           COALESCE(SUM(rs.active_ms),0)::bigint AS active_ms \
         FROM reading_sessions rs \
         JOIN series s ON s.id = rs.series_id \
         WHERE rs.user_id = $1",
    );
    let mut params: Vec<Value> = vec![user_id.into()];
    append_scope(&mut sql, &mut params, 2, "rs.", since, None, None);
    sql.push_str(
        " GROUP BY s.id, s.name \
         ORDER BY reads DESC, active_ms DESC LIMIT 10",
    );

    #[derive(FromQueryResult)]
    struct Row {
        series_id: String,
        name: String,
        distinct_issues: i64,
        reads: i64,
        active_ms: i64,
    }
    match Row::find_by_statement(Statement::from_sql_and_values(backend, &sql, params))
        .all(&app.db)
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|r| RereadSeriesEntry {
                series_id: r.series_id,
                name: r.name,
                distinct_issues: r.distinct_issues,
                reads: r.reads,
                active_ms: r.active_ms,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "compute_reread_top_series failed");
            Vec::new()
        }
    }
}

/// Completion rate: (distinct issues finished) / (distinct issues touched)
/// within the scope. Finished := `progress_records.finished = TRUE` OR a
/// session whose `furthest_page` reached `page_count - 1` (last index).
async fn compute_completion(
    app: &AppState,
    backend: sea_orm::DbBackend,
    user_id: Uuid,
    since: Option<&DateTime<FixedOffset>>,
    issue_filter: Option<&str>,
    series_filter: Option<&Uuid>,
) -> CompletionView {
    let mut sql = String::from(
        "WITH touched AS ( \
           SELECT DISTINCT rs.issue_id AS issue_id \
           FROM reading_sessions rs \
           WHERE rs.user_id = $1",
    );
    let mut params: Vec<Value> = vec![user_id.into()];
    append_scope(
        &mut sql,
        &mut params,
        2,
        "rs.",
        since,
        issue_filter,
        series_filter,
    );
    sql.push_str(
        " ), max_page AS ( \
           SELECT rs.issue_id AS issue_id, MAX(rs.furthest_page) AS furthest \
           FROM reading_sessions rs \
           JOIN touched t ON t.issue_id = rs.issue_id \
           WHERE rs.user_id = $1 \
           GROUP BY rs.issue_id \
         ) \
         SELECT \
           (SELECT COUNT(*)::bigint FROM touched) AS started, \
           ( \
             SELECT COUNT(DISTINCT t.issue_id)::bigint \
             FROM touched t \
             LEFT JOIN max_page mp ON mp.issue_id = t.issue_id \
             LEFT JOIN issues i ON i.id = t.issue_id \
             LEFT JOIN progress_records p ON p.issue_id = t.issue_id AND p.user_id = $1 \
             WHERE COALESCE(p.finished, FALSE) = TRUE \
                OR (i.page_count IS NOT NULL AND mp.furthest >= i.page_count - 1) \
           ) AS completed",
    );

    #[derive(FromQueryResult)]
    struct Row {
        started: i64,
        completed: i64,
    }
    match Row::find_by_statement(Statement::from_sql_and_values(backend, &sql, params))
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => {
            let rate = if r.started > 0 {
                r.completed as f64 / r.started as f64
            } else {
                0.0
            };
            CompletionView {
                started: r.started,
                completed: r.completed,
                rate,
            }
        }
        Ok(None) => CompletionView::default(),
        Err(e) => {
            tracing::warn!(error = %e, "compute_completion failed");
            CompletionView::default()
        }
    }
}

/// First/last session start times in scope. Returned as RFC 3339 strings;
/// `None` when no sessions match.
async fn compute_first_last(
    app: &AppState,
    backend: sea_orm::DbBackend,
    user_id: Uuid,
    since: Option<&DateTime<FixedOffset>>,
    issue_filter: Option<&str>,
    series_filter: Option<&Uuid>,
) -> (Option<String>, Option<String>) {
    let mut sql = String::from(
        "SELECT MIN(started_at) AS first_at, MAX(started_at) AS last_at \
         FROM reading_sessions \
         WHERE user_id = $1",
    );
    let mut params: Vec<Value> = vec![user_id.into()];
    append_scope(
        &mut sql,
        &mut params,
        2,
        "",
        since,
        issue_filter,
        series_filter,
    );

    #[derive(FromQueryResult)]
    struct Row {
        first_at: Option<DateTime<FixedOffset>>,
        last_at: Option<DateTime<FixedOffset>>,
    }
    match Row::find_by_statement(Statement::from_sql_and_values(backend, &sql, params))
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => (
            r.first_at.map(|t| t.to_rfc3339()),
            r.last_at.map(|t| t.to_rfc3339()),
        ),
        Ok(None) | Err(_) => (None, None),
    }
}

/// Top-N by trimmed-non-empty values from a comma-separated text column on
/// `issues`. Used for both genres and tags. Returns up to 10 entries
/// sorted by accumulated active_ms desc, sessions desc.
async fn top_csv_column(
    app: &AppState,
    backend: sea_orm::DbBackend,
    user_id: Uuid,
    column_expr: &str,
    since: Option<&DateTime<FixedOffset>>,
    issue_filter: Option<&str>,
    series_filter: Option<&Uuid>,
) -> Vec<TopNameEntry> {
    let mut sql = format!(
        "SELECT TRIM(g.name) AS name, \
           COUNT(*)::bigint AS sessions, \
           COALESCE(SUM(rs.active_ms),0)::bigint AS active_ms \
         FROM reading_sessions rs \
         JOIN issues i ON i.id = rs.issue_id \
         CROSS JOIN LATERAL unnest(string_to_array(COALESCE({column_expr}, ''), ',')) AS g(name) \
         WHERE rs.user_id = $1 AND TRIM(g.name) <> ''",
    );
    let mut params: Vec<Value> = vec![user_id.into()];
    append_scope(
        &mut sql,
        &mut params,
        2,
        "rs.",
        since,
        issue_filter,
        series_filter,
    );
    sql.push_str(" GROUP BY TRIM(g.name) ORDER BY active_ms DESC, sessions DESC LIMIT 10");

    #[derive(FromQueryResult)]
    struct Row {
        name: String,
        sessions: i64,
        active_ms: i64,
    }
    match Row::find_by_statement(Statement::from_sql_and_values(backend, &sql, params))
        .all(&app.db)
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|r| TopNameEntry {
                name: r.name,
                sessions: r.sessions,
                active_ms: r.active_ms,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, column = column_expr, "reading stats top_csv failed");
            Vec::new()
        }
    }
}

fn encode_cursor(started_at: &DateTime<FixedOffset>, id: &Uuid) -> String {
    let payload = format!("{}|{}", started_at.to_rfc3339(), id);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload)
}

fn decode_cursor(s: &str) -> Option<(DateTime<FixedOffset>, Uuid)> {
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .ok()?;
    let payload = std::str::from_utf8(&decoded).ok()?;
    let (ts, id) = payload.split_once('|')?;
    let started = DateTime::parse_from_rfc3339(ts).ok()?;
    let uuid = Uuid::parse_str(id).ok()?;
    Some((started, uuid))
}

/// Compute (current, longest) consecutive-day streaks from a sorted list of
/// active days. `today` is the user's local "today" (so a streak that ends
/// yesterday with no read today still counts in `current_streak` if
/// yesterday's row is the last active day; we only break the streak once the
/// gap exceeds 1 day).
fn streak_lengths(days: &[NaiveDate], today: NaiveDate) -> (i64, i64) {
    if days.is_empty() {
        return (0, 0);
    }
    let mut longest = 1i64;
    let mut run = 1i64;
    for w in days.windows(2) {
        let prev = w[0];
        let cur = w[1];
        if (cur - prev).num_days() == 1 {
            run += 1;
        } else {
            run = 1;
        }
        if run > longest {
            longest = run;
        }
    }
    // Current streak: the last contiguous run that ends today or yesterday.
    let last = *days.last().expect("non-empty above");
    let gap_to_today = (today - last).num_days();
    let current = if gap_to_today <= 1 {
        // Walk back from the end while consecutive.
        let mut c = 1i64;
        for w in days.windows(2).rev() {
            let prev = w[0];
            let cur = w[1];
            if (cur - prev).num_days() == 1 {
                c += 1;
            } else {
                break;
            }
        }
        c
    } else {
        0
    };
    (current, longest)
}

async fn visible(app: &AppState, user: &CurrentUser, lib_id: Uuid) -> bool {
    if user.role == "admin" {
        return true;
    }
    library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .filter(library_user_access::Column::LibraryId.eq(lib_id))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_some()
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn streak_empty() {
        assert_eq!(streak_lengths(&[], d("2026-05-06")), (0, 0));
    }

    #[test]
    fn streak_single_day_today() {
        let days = vec![d("2026-05-06")];
        assert_eq!(streak_lengths(&days, d("2026-05-06")), (1, 1));
    }

    #[test]
    fn streak_consecutive_ending_today() {
        let days = vec![d("2026-05-04"), d("2026-05-05"), d("2026-05-06")];
        assert_eq!(streak_lengths(&days, d("2026-05-06")), (3, 3));
    }

    #[test]
    fn streak_with_gap_resets_current() {
        let days = vec![d("2026-04-01"), d("2026-04-02"), d("2026-05-05")];
        // longest = 2 (Apr 1-2). Current = 1 (May 5 ends yesterday relative to today=May 6).
        assert_eq!(streak_lengths(&days, d("2026-05-06")), (1, 2));
    }

    #[test]
    fn streak_too_old_is_zero_current() {
        let days = vec![d("2026-04-01"), d("2026-04-02"), d("2026-04-03")];
        // Last day is more than 1 day before today → current = 0.
        assert_eq!(streak_lengths(&days, d("2026-05-06")), (0, 3));
    }

    #[test]
    fn cursor_round_trip() {
        let now = Utc::now().fixed_offset();
        let id = Uuid::now_v7();
        let s = encode_cursor(&now, &id);
        let (back_ts, back_id) = decode_cursor(&s).unwrap();
        assert_eq!(back_id, id);
        assert_eq!(back_ts.timestamp_millis(), now.timestamp_millis());
    }
}
