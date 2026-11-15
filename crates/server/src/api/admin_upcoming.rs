//! Upcoming releases: the admin surface (upcoming-releases 1.0).
//!
//! | method | path | |
//! |---|---|---|
//! | `GET`  | `/admin/upcoming/status` | the release calendar's state and row counts |
//! | `POST` | `/admin/upcoming/sync` | queue a list pass now |
//!
//! Both admin-only (`RequireAdmin`). The switches themselves
//! (`upcoming.enabled`, `upcoming.weeks_ahead`, …) are ordinary runtime
//! settings, changed through `/admin/settings`. `sync` writes an
//! `admin.upcoming.sync` audit row.

use axum::{
    Extension, Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use serde::Serialize;
use shared::error::ApiErrorCode;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::respond;
use crate::auth::RequireAdmin;
use crate::middleware::RequestContext;
use crate::record_admin_action;
use crate::state::AppState;
use crate::upcoming::{self, calendar};
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(status))
        .routes(routes!(sync))
}

/// Issues the calendar holds for one Monday–Sunday week.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpcomingWeekCountView {
    /// The Monday, `YYYY-MM-DD`.
    #[schema(value_type = String, format = Date)]
    pub week_start: NaiveDate,
    pub issues: i64,
}

/// The release calendar as the list pass left it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpcomingCalendarStatusView {
    /// First and last store date the current settings cover.
    #[schema(value_type = String, format = Date)]
    pub window_start: NaiveDate,
    #[schema(value_type = String, format = Date)]
    pub window_end: NaiveDate,
    pub issues: i64,
    pub series: i64,
    /// Series whose publisher and ComicVine id have not been fetched yet.
    pub series_pending_detail: i64,
    /// Series Metron has not linked to ComicVine.
    pub series_without_comicvine: i64,
    /// Per-week issue counts, oldest first.
    pub weeks: Vec<UpcomingWeekCountView>,
    /// When a pass last listed every week of its window.
    #[schema(value_type = Option<String>, format = DateTime)]
    pub last_finished_at: Option<DateTime<FixedOffset>>,
    /// A pass stopped part-way and will resume at this Monday.
    #[schema(value_type = Option<String>, format = Date)]
    pub resumes_at_week: Option<NaiveDate>,
    pub last_error: Option<String>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub last_error_at: Option<DateTime<FixedOffset>>,
    /// After a failure, no pass starts before this time.
    #[schema(value_type = Option<String>, format = DateTime)]
    pub next_attempt_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpcomingStatusView {
    /// `upcoming.enabled`.
    pub enabled: bool,
    /// Metron is enabled and has credentials. The calendar needs both
    /// this and `enabled`.
    pub metron_ready: bool,
    /// `upcoming.weeks_ahead`.
    pub weeks_ahead: u32,
    pub calendar: UpcomingCalendarStatusView,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpcomingSyncResp {
    /// `false` when a pass was already queued.
    pub queued: bool,
}

fn internal(e: &sea_orm::DbErr) -> Response {
    tracing::error!(error = %e, "upcoming admin query failed");
    respond(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Internal,
        "internal",
    )
}

fn metron_ready(app: &AppState) -> bool {
    let cfg = app.cfg();
    cfg.metron_enabled && crate::metadata::metron::MetronAuth::from_config(&cfg).is_some()
}

#[utoipa::path(
    operation_id = "admin_upcoming_status", get,
    path = "/admin/upcoming/status",
    responses(
        (status = 200, body = UpcomingStatusView),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn status(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let cfg = app.cfg();
    let state = match calendar::load_state(&app.db).await {
        Ok(s) => s,
        Err(e) => return internal(&e),
    };
    let counts = match calendar::counts(&app.db).await {
        Ok(c) => c,
        Err(e) => return internal(&e),
    };
    let window = upcoming::window(Utc::now().date_naive(), cfg.upcoming_weeks_ahead);
    Json(UpcomingStatusView {
        enabled: cfg.upcoming_enabled,
        metron_ready: metron_ready(&app),
        weeks_ahead: cfg.upcoming_weeks_ahead,
        calendar: UpcomingCalendarStatusView {
            window_start: window.start,
            window_end: window.end,
            issues: counts.issues,
            series: counts.series,
            series_pending_detail: counts.series_pending_detail,
            series_without_comicvine: counts.series_without_comicvine,
            weeks: counts
                .weeks
                .into_iter()
                .map(|w| UpcomingWeekCountView {
                    week_start: w.week,
                    issues: w.issues,
                })
                .collect(),
            last_finished_at: state.last_finished_at,
            resumes_at_week: state.pass_next_week,
            last_error: state.last_error,
            last_error_at: state.last_error_at,
            next_attempt_at: state.next_attempt_at,
        },
    })
    .into_response()
}

#[utoipa::path(
    operation_id = "admin_upcoming_sync", post,
    path = "/admin/upcoming/sync",
    responses(
        (status = 202, body = UpcomingSyncResp, description = "list pass queued, or one already was"),
        (status = 403, description = "admin only"),
        (status = 409, description = "upcoming releases are off, or Metron is not ready"),
    )
)]
#[handler]
pub async fn sync(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
) -> Response {
    if !calendar::ready(&app) {
        return respond(
            StatusCode::CONFLICT,
            ApiErrorCode::Conflict,
            "Upcoming releases are turned off, or Metron is not enabled with credentials",
        );
    }
    // An admin asking by hand overrides a failure backoff: clear it so
    // the queued pass runs instead of reporting "backing off".
    if let Err(e) = calendar::clear_backoff(&app.db).await {
        return internal(&e);
    }
    let queued = crate::jobs::release_calendar::enqueue(&app, "manual").await;
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.upcoming.sync",
        payload = serde_json::json!({ "queued": queued }),
    );
    (StatusCode::ACCEPTED, Json(UpcomingSyncResp { queued })).into_response()
}
