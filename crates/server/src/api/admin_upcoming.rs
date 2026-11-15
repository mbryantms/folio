//! Upcoming releases: the admin surface (upcoming-releases 1.0).
//!
//! | method | path | |
//! |---|---|---|
//! | `GET`  | `/admin/upcoming/status` | the release calendar's and the Mylar snapshot's state |
//! | `POST` | `/admin/upcoming/sync` | queue a list pass now |
//! | `POST` | `/admin/integrations/mylar/token` | create (or replace) the Mylar ingest token; the plaintext is returned once |
//! | `DELETE` | `/admin/integrations/mylar/token` | revoke it; `?purge=true` also deletes the stored snapshot |
//!
//! All admin-only (`RequireAdmin`). The switches themselves
//! (`upcoming.enabled`, `integrations.mylar.enabled`, …) are ordinary
//! runtime settings, changed through `/admin/settings`. Mutations write
//! `admin.upcoming.sync` and `admin.integration.mylar.token.{create,revoke}`
//! audit rows.
//!
//! Nothing here reaches out to Mylar: the token only lets Mylar's host
//! push a snapshot in (`api::mylar_ingest`).

use axum::{
    Extension, Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use shared::error::ApiErrorCode;
use std::collections::BTreeMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::respond;
use crate::auth::RequireAdmin;
use crate::middleware::RequestContext;
use crate::record_admin_action;
use crate::state::AppState;
use crate::upcoming::mylar::{self, Freshness};
use crate::upcoming::{self, calendar};
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(status))
        .routes(routes!(sync))
        .routes(routes!(create_mylar_token, revoke_mylar_token))
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
    pub mylar: MylarStatusView,
}

/// The ingest token, without its value (only a digest is stored).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MylarTokenView {
    #[schema(value_type = String, format = DateTime)]
    pub created_at: DateTime<FixedOffset>,
    /// When a snapshot was last accepted with this token.
    #[schema(value_type = Option<String>, format = DateTime)]
    pub last_used_at: Option<DateTime<FixedOffset>>,
    pub last_used_ip: Option<String>,
}

/// The stored Mylar snapshot's header.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MylarSnapshotStatusView {
    /// When Folio received it; `freshness` is judged by this.
    #[schema(value_type = String, format = DateTime)]
    pub received_at: DateTime<FixedOffset>,
    /// When the script read Mylar's database.
    #[schema(value_type = String, format = DateTime)]
    pub generated_at: DateTime<FixedOffset>,
    pub freshness: Freshness,
    pub pull_count: i32,
    pub watch_count: i32,
    pub wanted_count: i32,
    /// Mylar's download queue, entries per status.
    pub queue_counts: BTreeMap<String, i64>,
    pub annuals_included: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MylarStatusView {
    /// `integrations.mylar.enabled`. Snapshots are accepted only while
    /// this and `upcoming.enabled` are both on.
    pub enabled: bool,
    /// `integrations.mylar.stale_after_minutes`.
    pub stale_after_minutes: u32,
    pub token: Option<MylarTokenView>,
    pub snapshot: Option<MylarSnapshotStatusView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MylarTokenResp {
    /// The token, shown this once. Folio keeps only a digest.
    pub token: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct RevokeMylarTokenQuery {
    /// Also delete the stored snapshot.
    #[serde(default)]
    pub purge: bool,
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
    let token = match mylar::token_row(&app.db).await {
        Ok(t) => t,
        Err(e) => return internal(&e),
    };
    let snapshot = match mylar::snapshot_row(&app.db).await {
        Ok(s) => s,
        Err(e) => return internal(&e),
    };
    let now = Utc::now().fixed_offset();
    let stale_after = cfg.integrations_mylar_stale_after_minutes;
    let mylar = MylarStatusView {
        enabled: cfg.integrations_mylar_enabled,
        stale_after_minutes: stale_after,
        token: token.map(|t| MylarTokenView {
            created_at: t.created_at,
            last_used_at: t.last_used_at,
            last_used_ip: t.last_used_ip,
        }),
        snapshot: snapshot.map(|s| MylarSnapshotStatusView {
            received_at: s.received_at,
            generated_at: s.generated_at,
            freshness: mylar::freshness(s.received_at, now, stale_after),
            pull_count: s.pull_count,
            watch_count: s.watch_count,
            wanted_count: s.wanted_count,
            queue_counts: s
                .queue
                .get("counts")
                .and_then(|c| serde_json::from_value(c.clone()).ok())
                .unwrap_or_default(),
            annuals_included: s.annuals_included,
        }),
    };
    let window = upcoming::window(now.date_naive(), cfg.upcoming_weeks_ahead);
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
        mylar,
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

#[utoipa::path(
    operation_id = "admin_mylar_token_create", post,
    path = "/admin/integrations/mylar/token",
    responses(
        (status = 201, body = MylarTokenResp, description = "token created; any earlier token stops working"),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn create_mylar_token(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
) -> Response {
    let replaced = match mylar::token_row(&app.db).await {
        Ok(existing) => existing.is_some(),
        Err(e) => return internal(&e),
    };
    let token = match mylar::issue_token(&app.db, actor.id).await {
        Ok(t) => t,
        Err(e) => return internal(&e),
    };
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.integration.mylar.token.create",
        payload = serde_json::json!({ "replaced": replaced }),
    );
    (StatusCode::CREATED, Json(MylarTokenResp { token })).into_response()
}

#[utoipa::path(
    operation_id = "admin_mylar_token_revoke", delete,
    path = "/admin/integrations/mylar/token",
    params(RevokeMylarTokenQuery),
    responses(
        (status = 204, description = "token revoked (and the stored snapshot deleted when asked)"),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn revoke_mylar_token(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Query(q): Query<RevokeMylarTokenQuery>,
) -> Response {
    let had_token = match mylar::revoke_token(&app.db).await {
        Ok(v) => v,
        Err(e) => return internal(&e),
    };
    if q.purge
        && let Err(e) = mylar::purge(&app.db).await
    {
        return internal(&e);
    }
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.integration.mylar.token.revoke",
        payload = serde_json::json!({ "had_token": had_token, "purged": q.purge }),
    );
    StatusCode::NO_CONTENT.into_response()
}
