//! `GET /admin/stats/overview` — system-wide totals + recent activity.
//! `GET /admin/users/{id}/reading-stats` — per-user reading-stats payload.
//!
//! M6c. Admin-only. Both endpoints return aggregate, anonymized data on the
//! overview path; the per-user reading-stats endpoint is the one place where
//! an admin can drill into another user's activity, and every access there
//! emits an `admin.user.activity.view` audit row.

use axum::{
    Extension, Json, Router,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{Duration, Utc};
use entity::{
    issue, library, scan_run, series,
    user::{self, Entity as UserEntity},
};
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, PaginatorTrait, QueryFilter,
    Statement,
};
use serde::Serialize;
use uuid::Uuid;

use crate::audit::{self, AuditEntry};
use crate::auth::RequireAdmin;
use crate::middleware::RequestContext;
use crate::state::AppState;

use super::reading_sessions::{
    DayBucket, ReadingStatsView, StatsError, StatsQuery, TopSeriesEntry, compute_stats_for_user,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/stats/overview", get(overview))
        .route("/admin/stats/users", get(users_list))
        .route("/admin/stats/engagement", get(engagement))
        .route("/admin/stats/content", get(content))
        .route("/admin/stats/quality", get(quality))
        .route("/admin/users/{id}/reading-stats", get(user_reading_stats))
}

// ───────── views ─────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OverviewView {
    pub totals: TotalsBlock,
    /// Apalis scan jobs currently running (state = 'running').
    pub scans_in_flight: i64,
    pub open_health: HealthBlock,
    /// Reading sessions started in the last 24h.
    pub sessions_today: i64,
    /// Distinct users with a heartbeat in the last 5 minutes.
    pub active_readers_now: i64,
    /// Daily reads-volume buckets for the last 14 days (UTC). Each row
    /// aggregates across all users — never per-user, so the dashboard never
    /// leaks anyone's reading habits.
    pub reads_per_day: Vec<DayBucket>,
    /// System-wide most-read series in the last 30 days. Aggregated across
    /// users; consumed by `/admin/stats`. Capped at 10 entries.
    pub top_series_all_users: Vec<TopSeriesEntry>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TotalsBlock {
    pub libraries: i64,
    pub series: i64,
    pub issues: i64,
    pub users: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct HealthBlock {
    pub error: i64,
    pub warning: i64,
    pub info: i64,
}

// ───────── Stats v2 views ─────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AdminUserStatsListView {
    pub users: Vec<AdminUserStatsRow>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AdminUserStatsRow {
    pub user_id: String,
    pub display_name: String,
    pub email: Option<String>,
    pub role: String,
    pub state: String,
    pub last_active_at: Option<String>,
    pub sessions_30d: i64,
    pub active_ms_30d: i64,
    pub sessions_all_time: i64,
    pub active_ms_all_time: i64,
    pub top_series_name: Option<String>,
    pub device_breakdown: Vec<DeviceBucket>,
    pub excluded_from_aggregates: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeviceBucket {
    pub device: String,
    pub sessions: i64,
    pub active_ms: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EngagementView {
    /// Trailing-window DAU/WAU/MAU samples per day, last 90 days.
    pub series: Vec<EngagementPoint>,
    /// Aggregate device breakdown across all (non-excluded) users over the
    /// last 30 days.
    pub devices_30d: Vec<DeviceBucket>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EngagementPoint {
    /// `YYYY-MM-DD` (UTC).
    pub date: String,
    /// Distinct users active in the trailing 1 day window ending on `date`.
    pub dau: i64,
    /// Distinct users active in the trailing 7-day window ending on `date`.
    pub wau: i64,
    /// Distinct users active in the trailing 30-day window ending on `date`.
    pub mau: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContentInsightsView {
    pub dead_stock: Vec<DeadStockEntry>,
    pub abandoned: Vec<AbandonedEntry>,
    pub completion_funnel: Vec<FunnelBucket>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DeadStockEntry {
    pub series_id: String,
    pub name: String,
    pub publisher: Option<String>,
    pub library_id: String,
    pub library_name: String,
    pub issue_count: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AbandonedEntry {
    pub series_id: String,
    pub name: String,
    pub sessions: i64,
    pub unfinished_issues: i64,
    pub readers: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FunnelBucket {
    /// One of `"0-25"`, `"25-50"`, `"50-75"`, `"75-99"`, `"100"`.
    pub bucket: String,
    pub issues: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataQualityView {
    pub orphan_sessions: i64,
    pub long_sessions: i64,
    pub dangling_sessions: i64,
    pub metadata: MetadataCoverageView,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MetadataCoverageView {
    pub total_issues: i64,
    pub missing_writer: i64,
    pub missing_cover_artist: i64,
    pub missing_page_count: i64,
    pub missing_genre: i64,
    pub missing_publisher: i64,
}

// ───────── handlers ─────────

#[utoipa::path(
    get,
    path = "/admin/stats/overview",
    responses(
        (status = 200, body = OverviewView),
        (status = 403, description = "admin only"),
    )
)]
pub async fn overview(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let totals = match compute_totals(&app).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "admin overview totals failed");
            return internal();
        }
    };

    let scans_in_flight = scan_run::Entity::find()
        .filter(scan_run::Column::State.eq("running"))
        .count(&app.db)
        .await
        .unwrap_or(0) as i64;

    let open_health = match compute_open_health(&app).await {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(error = %e, "admin overview open_health failed");
            HealthBlock {
                error: 0,
                warning: 0,
                info: 0,
            }
        }
    };

    // Sessions today — bucket window is the last 24h regardless of any user
    // timezone, since the dashboard owner is the admin (system-time view).
    // Excludes opted-out users so the aggregate honors the privacy toggle.
    let since_today = (Utc::now() - Duration::hours(24)).fixed_offset();
    let sessions_today = compute_sessions_today(&app, since_today).await.unwrap_or(0);

    // Active readers — distinct user_id whose last heartbeat was within the
    // last 5 minutes. Gated by the `reading_sessions_dangling_idx` partial
    // index; cheap.
    let active_readers_now = compute_active_readers(&app).await.unwrap_or(0);

    let reads_per_day = compute_reads_per_day(&app).await.unwrap_or_default();
    let top_series_all_users = compute_top_series_all_users(&app).await.unwrap_or_default();

    Json(OverviewView {
        totals,
        scans_in_flight,
        open_health,
        sessions_today,
        active_readers_now,
        reads_per_day,
        top_series_all_users,
    })
    .into_response()
}

#[utoipa::path(
    get,
    path = "/admin/users/{id}/reading-stats",
    params(
        ("id" = String, Path, description = "target user id (uuid)"),
        ("range" = Option<String>, Query, description = "'7d', '30d', '90d', 'all'"),
    ),
    responses(
        (status = 200, body = ReadingStatsView),
        (status = 400, description = "validation error"),
        (status = 403, description = "admin only"),
        (status = 404, description = "user not found"),
    )
)]
pub async fn user_reading_stats(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<String>,
    Query(q): Query<StatsQuery>,
) -> Response {
    let Ok(target_id) = Uuid::parse_str(&id) else {
        return bad("validation", "invalid user id");
    };
    // Confirm the target exists so a 404 is faithful (compute_stats_for_user
    // would 500 on a missing user otherwise).
    if UserEntity::find_by_id(target_id)
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return error_response(StatusCode::NOT_FOUND, "not_found", "user not found");
    }

    // Audit the access *before* the query, with the requested range.
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.user.activity.view",
            target_type: Some("user"),
            target_id: Some(id.clone()),
            payload: serde_json::json!({
                "range": q.range.clone().unwrap_or_else(|| "30d".into()),
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    match compute_stats_for_user(&app, target_id, q).await {
        Ok(view) => Json(view).into_response(),
        Err(StatsError {
            status,
            code,
            message,
        }) => error_response(status, code, &message),
    }
}

// ───────── computation ─────────

async fn compute_totals(app: &AppState) -> Result<TotalsBlock, sea_orm::DbErr> {
    let libraries = library::Entity::find().count(&app.db).await? as i64;
    let series = series::Entity::find().count(&app.db).await? as i64;
    let issues = issue::Entity::find()
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .count(&app.db)
        .await? as i64;
    let users = user::Entity::find().count(&app.db).await? as i64;
    Ok(TotalsBlock {
        libraries,
        series,
        issues,
        users,
    })
}

async fn compute_open_health(app: &AppState) -> Result<HealthBlock, sea_orm::DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        severity: String,
        n: i64,
    }
    let backend = app.db.get_database_backend();
    let stmt = Statement::from_string(
        backend,
        "SELECT severity, COUNT(*)::bigint AS n \
         FROM library_health_issues \
         WHERE resolved_at IS NULL AND dismissed_at IS NULL \
         GROUP BY severity"
            .to_string(),
    );
    let rows = Row::find_by_statement(stmt).all(&app.db).await?;
    let mut block = HealthBlock {
        error: 0,
        warning: 0,
        info: 0,
    };
    for r in rows {
        match r.severity.as_str() {
            "error" => block.error = r.n,
            "warning" => block.warning = r.n,
            "info" => block.info = r.n,
            _ => {}
        }
    }
    Ok(block)
}

async fn compute_sessions_today(
    app: &AppState,
    since: chrono::DateTime<chrono::FixedOffset>,
) -> Result<i64, sea_orm::DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        n: i64,
    }
    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(
        backend,
        "SELECT COUNT(*)::bigint AS n \
         FROM reading_sessions rs \
         JOIN users u ON u.id = rs.user_id \
         WHERE rs.started_at >= $1 AND u.exclude_from_aggregates = FALSE",
        vec![since.into()],
    );
    let row = Row::find_by_statement(stmt).one(&app.db).await?;
    Ok(row.map(|r| r.n).unwrap_or(0))
}

async fn compute_active_readers(app: &AppState) -> Result<i64, sea_orm::DbErr> {
    let cutoff = (Utc::now() - Duration::minutes(5)).fixed_offset();
    #[derive(FromQueryResult)]
    struct Row {
        n: i64,
    }
    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(
        backend,
        "SELECT COUNT(DISTINCT rs.user_id)::bigint AS n \
         FROM reading_sessions rs \
         JOIN users u ON u.id = rs.user_id \
         WHERE rs.last_heartbeat_at >= $1 AND u.exclude_from_aggregates = FALSE",
        vec![cutoff.into()],
    );
    let row = Row::find_by_statement(stmt).one(&app.db).await?;
    Ok(row.map(|r| r.n).unwrap_or(0))
}

async fn compute_top_series_all_users(
    app: &AppState,
) -> Result<Vec<TopSeriesEntry>, sea_orm::DbErr> {
    let since = (Utc::now() - Duration::days(30)).fixed_offset();
    #[derive(FromQueryResult)]
    struct Row {
        series_id: String,
        name: String,
        sessions: i64,
        active_ms: i64,
    }
    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(
        backend,
        "SELECT s.id::text AS series_id, s.name AS name, \
           COUNT(*)::bigint AS sessions, \
           COALESCE(SUM(rs.active_ms),0)::bigint AS active_ms \
         FROM reading_sessions rs \
         JOIN series s ON s.id = rs.series_id \
         JOIN users u ON u.id = rs.user_id \
         WHERE rs.started_at >= $1 AND u.exclude_from_aggregates = FALSE \
         GROUP BY s.id, s.name \
         ORDER BY active_ms DESC, sessions DESC \
         LIMIT 10",
        vec![since.into()],
    );
    let rows = Row::find_by_statement(stmt).all(&app.db).await?;
    Ok(rows
        .into_iter()
        .map(|r| TopSeriesEntry {
            series_id: r.series_id,
            name: r.name,
            sessions: r.sessions,
            active_ms: r.active_ms,
        })
        .collect())
}

async fn compute_reads_per_day(app: &AppState) -> Result<Vec<DayBucket>, sea_orm::DbErr> {
    let since = (Utc::now() - Duration::days(14)).fixed_offset();
    #[derive(FromQueryResult)]
    struct Row {
        date: String,
        sessions: i64,
        active_ms: i64,
        pages: i64,
    }
    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(
        backend,
        "SELECT to_char((rs.started_at AT TIME ZONE 'UTC')::date, 'YYYY-MM-DD') AS date, \
           COUNT(*)::bigint AS sessions, \
           COALESCE(SUM(rs.active_ms),0)::bigint AS active_ms, \
           COALESCE(SUM(rs.distinct_pages_read),0)::bigint AS pages \
         FROM reading_sessions rs \
         JOIN users u ON u.id = rs.user_id \
         WHERE rs.started_at >= $1 AND u.exclude_from_aggregates = FALSE \
         GROUP BY 1 \
         ORDER BY 1 ASC",
        vec![since.into()],
    );
    let rows = Row::find_by_statement(stmt).all(&app.db).await?;
    Ok(rows
        .into_iter()
        .map(|r| DayBucket {
            date: r.date,
            sessions: r.sessions,
            active_ms: r.active_ms,
            pages: r.pages,
        })
        .collect())
}

// ───────── Stats v2 handlers ─────────

#[utoipa::path(
    get,
    path = "/admin/stats/users",
    responses(
        (status = 200, body = AdminUserStatsListView),
        (status = 403, description = "admin only"),
    )
)]
pub async fn users_list(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    match compute_users_list(&app).await {
        Ok(view) => Json(view).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "admin users_list failed");
            internal()
        }
    }
}

#[utoipa::path(
    get,
    path = "/admin/stats/engagement",
    responses(
        (status = 200, body = EngagementView),
        (status = 403, description = "admin only"),
    )
)]
pub async fn engagement(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    match compute_engagement(&app).await {
        Ok(view) => Json(view).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "admin engagement failed");
            internal()
        }
    }
}

#[utoipa::path(
    get,
    path = "/admin/stats/content",
    responses(
        (status = 200, body = ContentInsightsView),
        (status = 403, description = "admin only"),
    )
)]
pub async fn content(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    match compute_content(&app).await {
        Ok(view) => Json(view).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "admin content failed");
            internal()
        }
    }
}

#[utoipa::path(
    get,
    path = "/admin/stats/quality",
    responses(
        (status = 200, body = DataQualityView),
        (status = 403, description = "admin only"),
    )
)]
pub async fn quality(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    match compute_quality(&app).await {
        Ok(view) => Json(view).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "admin quality failed");
            internal()
        }
    }
}

// ───────── Stats v2 computation ─────────

async fn compute_users_list(app: &AppState) -> Result<AdminUserStatsListView, sea_orm::DbErr> {
    let users = UserEntity::find().all(&app.db).await?;
    let backend = app.db.get_database_backend();

    let cutoff_30d = (Utc::now() - Duration::days(30)).fixed_offset();

    #[derive(FromQueryResult)]
    struct AggRow {
        user_id: Uuid,
        sessions: i64,
        active_ms: i64,
        last_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    }
    let all_stmt = Statement::from_string(
        backend,
        "SELECT user_id, COUNT(*)::bigint AS sessions, \
                COALESCE(SUM(active_ms),0)::bigint AS active_ms, \
                MAX(started_at) AS last_at \
         FROM reading_sessions \
         GROUP BY user_id"
            .to_string(),
    );
    let all_rows = AggRow::find_by_statement(all_stmt).all(&app.db).await?;
    let mut all_map: std::collections::HashMap<Uuid, AggRow> = std::collections::HashMap::new();
    for r in all_rows {
        all_map.insert(r.user_id, r);
    }
    let recent_stmt = Statement::from_sql_and_values(
        backend,
        "SELECT user_id, COUNT(*)::bigint AS sessions, \
                COALESCE(SUM(active_ms),0)::bigint AS active_ms, \
                MAX(started_at) AS last_at \
         FROM reading_sessions \
         WHERE started_at >= $1 \
         GROUP BY user_id",
        vec![cutoff_30d.into()],
    );
    let recent_rows = AggRow::find_by_statement(recent_stmt).all(&app.db).await?;
    let mut recent_map: std::collections::HashMap<Uuid, AggRow> = std::collections::HashMap::new();
    for r in recent_rows {
        recent_map.insert(r.user_id, r);
    }

    #[derive(FromQueryResult)]
    struct TopSeriesRow {
        user_id: Uuid,
        name: String,
    }
    let top_stmt = Statement::from_string(
        backend,
        "SELECT t.user_id AS user_id, s.name AS name FROM ( \
           SELECT user_id, series_id, COUNT(*) AS c, \
             ROW_NUMBER() OVER (PARTITION BY user_id ORDER BY COUNT(*) DESC) AS rn \
           FROM reading_sessions GROUP BY user_id, series_id \
         ) t JOIN series s ON s.id = t.series_id WHERE t.rn = 1"
            .to_string(),
    );
    let top_rows = TopSeriesRow::find_by_statement(top_stmt)
        .all(&app.db)
        .await?;
    let mut top_map: std::collections::HashMap<Uuid, String> = std::collections::HashMap::new();
    for r in top_rows {
        top_map.insert(r.user_id, r.name);
    }

    #[derive(FromQueryResult)]
    struct DeviceRow {
        user_id: Uuid,
        device: Option<String>,
        sessions: i64,
        active_ms: i64,
    }
    let dev_stmt = Statement::from_string(
        backend,
        "SELECT user_id, device, COUNT(*)::bigint AS sessions, \
                COALESCE(SUM(active_ms),0)::bigint AS active_ms \
         FROM reading_sessions GROUP BY user_id, device"
            .to_string(),
    );
    let dev_rows = DeviceRow::find_by_statement(dev_stmt).all(&app.db).await?;
    let mut dev_map: std::collections::HashMap<Uuid, Vec<DeviceBucket>> =
        std::collections::HashMap::new();
    for r in dev_rows {
        dev_map.entry(r.user_id).or_default().push(DeviceBucket {
            device: r.device.unwrap_or_else(|| "unknown".into()),
            sessions: r.sessions,
            active_ms: r.active_ms,
        });
    }

    let mut rows: Vec<AdminUserStatsRow> = users
        .into_iter()
        .map(|u| {
            let all = all_map.get(&u.id);
            let recent = recent_map.get(&u.id);
            AdminUserStatsRow {
                user_id: u.id.to_string(),
                display_name: u.display_name,
                email: u.email,
                role: u.role,
                state: u.state,
                last_active_at: all.and_then(|r| r.last_at).map(|t| t.to_rfc3339()),
                sessions_30d: recent.map(|r| r.sessions).unwrap_or(0),
                active_ms_30d: recent.map(|r| r.active_ms).unwrap_or(0),
                sessions_all_time: all.map(|r| r.sessions).unwrap_or(0),
                active_ms_all_time: all.map(|r| r.active_ms).unwrap_or(0),
                top_series_name: top_map.get(&u.id).cloned(),
                device_breakdown: dev_map.remove(&u.id).unwrap_or_default(),
                excluded_from_aggregates: u.exclude_from_aggregates,
            }
        })
        .collect();
    // Most active first.
    rows.sort_by(|a, b| b.active_ms_30d.cmp(&a.active_ms_30d));
    Ok(AdminUserStatsListView { users: rows })
}

async fn compute_engagement(app: &AppState) -> Result<EngagementView, sea_orm::DbErr> {
    let backend = app.db.get_database_backend();
    let since = (Utc::now() - Duration::days(120)).fixed_offset();

    // Pull (user_id, date) distinct pairs for the last ~120 days from
    // non-excluded users, then roll DAU/WAU/MAU in-app — cheap given the
    // single-server scale.
    #[derive(FromQueryResult)]
    struct UDRow {
        user_id: Uuid,
        d: chrono::NaiveDate,
    }
    let stmt = Statement::from_sql_and_values(
        backend,
        "SELECT DISTINCT rs.user_id AS user_id, (rs.started_at AT TIME ZONE 'UTC')::date AS d \
         FROM reading_sessions rs \
         JOIN users u ON u.id = rs.user_id \
         WHERE rs.started_at >= $1 AND u.exclude_from_aggregates = FALSE",
        vec![since.into()],
    );
    let rows = UDRow::find_by_statement(stmt).all(&app.db).await?;

    let mut by_day: std::collections::HashMap<chrono::NaiveDate, std::collections::HashSet<Uuid>> =
        std::collections::HashMap::new();
    for r in rows {
        by_day.entry(r.d).or_default().insert(r.user_id);
    }

    let today = Utc::now().date_naive();
    let start = today - chrono::Duration::days(89);
    let mut series = Vec::with_capacity(90);
    for offset in 0..90 {
        let d = start + chrono::Duration::days(offset);
        let dau = by_day.get(&d).map(|s| s.len() as i64).unwrap_or(0);

        let mut wau_set = std::collections::HashSet::new();
        for k in 0..7 {
            if let Some(s) = by_day.get(&(d - chrono::Duration::days(k))) {
                for u in s {
                    wau_set.insert(*u);
                }
            }
        }
        let mut mau_set = std::collections::HashSet::new();
        for k in 0..30 {
            if let Some(s) = by_day.get(&(d - chrono::Duration::days(k))) {
                for u in s {
                    mau_set.insert(*u);
                }
            }
        }
        series.push(EngagementPoint {
            date: d.format("%Y-%m-%d").to_string(),
            dau,
            wau: wau_set.len() as i64,
            mau: mau_set.len() as i64,
        });
    }

    #[derive(FromQueryResult)]
    struct DevRow {
        device: Option<String>,
        sessions: i64,
        active_ms: i64,
    }
    let dev_cutoff = (Utc::now() - Duration::days(30)).fixed_offset();
    let dev_stmt = Statement::from_sql_and_values(
        backend,
        "SELECT rs.device AS device, COUNT(*)::bigint AS sessions, \
                COALESCE(SUM(rs.active_ms),0)::bigint AS active_ms \
         FROM reading_sessions rs \
         JOIN users u ON u.id = rs.user_id \
         WHERE rs.started_at >= $1 AND u.exclude_from_aggregates = FALSE \
         GROUP BY rs.device ORDER BY active_ms DESC",
        vec![dev_cutoff.into()],
    );
    let devices_30d: Vec<DeviceBucket> = DevRow::find_by_statement(dev_stmt)
        .all(&app.db)
        .await?
        .into_iter()
        .map(|r| DeviceBucket {
            device: r.device.unwrap_or_else(|| "unknown".into()),
            sessions: r.sessions,
            active_ms: r.active_ms,
        })
        .collect();

    Ok(EngagementView {
        series,
        devices_30d,
    })
}

async fn compute_content(app: &AppState) -> Result<ContentInsightsView, sea_orm::DbErr> {
    let backend = app.db.get_database_backend();

    // Dead stock: series with active issues but no sessions across any user.
    #[derive(FromQueryResult)]
    struct DeadRow {
        series_id: String,
        name: String,
        publisher: Option<String>,
        library_id: String,
        library_name: String,
        issue_count: i64,
    }
    let dead_stmt = Statement::from_string(
        backend,
        "SELECT s.id::text AS series_id, s.name AS name, s.publisher AS publisher, \
                l.id::text AS library_id, l.name AS library_name, \
                COUNT(i.id)::bigint AS issue_count \
         FROM series s \
         JOIN libraries l ON l.id = s.library_id \
         LEFT JOIN issues i ON i.series_id = s.id AND i.state = 'active' AND i.removed_at IS NULL \
         LEFT JOIN reading_sessions rs ON rs.series_id = s.id \
         WHERE rs.id IS NULL AND s.removed_at IS NULL \
         GROUP BY s.id, s.name, s.publisher, l.id, l.name \
         HAVING COUNT(i.id) > 0 \
         ORDER BY issue_count DESC, s.name ASC \
         LIMIT 50"
            .to_string(),
    );
    let dead_stock: Vec<DeadStockEntry> = DeadRow::find_by_statement(dead_stmt)
        .all(&app.db)
        .await?
        .into_iter()
        .map(|r| DeadStockEntry {
            series_id: r.series_id,
            name: r.name,
            publisher: r.publisher,
            library_id: r.library_id,
            library_name: r.library_name,
            issue_count: r.issue_count,
        })
        .collect();

    // Abandoned: series with high session count and high unfinished-issue
    // ratio. An issue counts as "unfinished" when no user's furthest_page has
    // reached page_count - 1 AND no progress_records.finished row exists.
    #[derive(FromQueryResult)]
    struct AbandonedRow {
        series_id: String,
        name: String,
        sessions: i64,
        unfinished_issues: i64,
        readers: i64,
    }
    let aban_stmt = Statement::from_string(
        backend,
        "WITH per_issue AS ( \
           SELECT rs.issue_id AS issue_id, rs.series_id AS series_id, \
             MAX(rs.furthest_page) AS furthest, \
             COUNT(*) AS sess, \
             COUNT(DISTINCT rs.user_id) AS rdrs \
           FROM reading_sessions rs \
           JOIN users u ON u.id = rs.user_id \
           WHERE u.exclude_from_aggregates = FALSE \
           GROUP BY rs.issue_id, rs.series_id \
         ), \
         tagged AS ( \
           SELECT pi.*, i.page_count AS page_count, \
             EXISTS ( \
               SELECT 1 FROM progress_records p \
               WHERE p.issue_id = pi.issue_id AND p.finished = TRUE \
             ) AS any_finished \
           FROM per_issue pi \
           JOIN issues i ON i.id = pi.issue_id \
         ) \
         SELECT s.id::text AS series_id, s.name AS name, \
                SUM(t.sess)::bigint AS sessions, \
                COUNT(*) FILTER ( \
                  WHERE t.any_finished = FALSE AND ( \
                    t.page_count IS NULL OR t.furthest < t.page_count - 1 \
                  ) \
                )::bigint AS unfinished_issues, \
                SUM(t.rdrs)::bigint AS readers \
         FROM tagged t JOIN series s ON s.id = t.series_id \
         GROUP BY s.id, s.name \
         HAVING SUM(t.sess) >= 3 \
         ORDER BY unfinished_issues DESC, sessions DESC \
         LIMIT 20"
            .to_string(),
    );
    let abandoned: Vec<AbandonedEntry> = AbandonedRow::find_by_statement(aban_stmt)
        .all(&app.db)
        .await?
        .into_iter()
        .map(|r| AbandonedEntry {
            series_id: r.series_id,
            name: r.name,
            sessions: r.sessions,
            unfinished_issues: r.unfinished_issues,
            readers: r.readers,
        })
        .collect();

    // Completion funnel: per (user, issue) bucket the highest progress and
    // count the issue once per bucket.
    #[derive(FromQueryResult)]
    struct FunnelRow {
        bucket: String,
        n: i64,
    }
    let fun_stmt = Statement::from_string(
        backend,
        "WITH per_pair AS ( \
           SELECT rs.user_id AS user_id, rs.issue_id AS issue_id, \
             MAX(rs.furthest_page) AS furthest, \
             MAX(i.page_count) AS page_count, \
             BOOL_OR(COALESCE(p.finished, FALSE)) AS any_finished \
           FROM reading_sessions rs \
           JOIN users u ON u.id = rs.user_id \
           JOIN issues i ON i.id = rs.issue_id \
           LEFT JOIN progress_records p ON p.user_id = rs.user_id AND p.issue_id = rs.issue_id \
           WHERE u.exclude_from_aggregates = FALSE \
           GROUP BY rs.user_id, rs.issue_id \
         ) \
         SELECT bucket, COUNT(*)::bigint AS n FROM ( \
           SELECT CASE \
             WHEN any_finished THEN '100' \
             WHEN page_count IS NULL OR page_count <= 1 THEN '0-25' \
             WHEN furthest::float8 / GREATEST(page_count - 1, 1) >= 0.75 THEN '75-99' \
             WHEN furthest::float8 / GREATEST(page_count - 1, 1) >= 0.5 THEN '50-75' \
             WHEN furthest::float8 / GREATEST(page_count - 1, 1) >= 0.25 THEN '25-50' \
             ELSE '0-25' END AS bucket \
           FROM per_pair \
         ) labeled GROUP BY bucket"
            .to_string(),
    );
    let raw = FunnelRow::find_by_statement(fun_stmt).all(&app.db).await?;
    let mut buckets: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for r in raw {
        buckets.insert(r.bucket, r.n);
    }
    let order = ["0-25", "25-50", "50-75", "75-99", "100"];
    let completion_funnel: Vec<FunnelBucket> = order
        .iter()
        .map(|k| FunnelBucket {
            bucket: (*k).to_string(),
            issues: buckets.get(*k).copied().unwrap_or(0),
        })
        .collect();

    Ok(ContentInsightsView {
        dead_stock,
        abandoned,
        completion_funnel,
    })
}

async fn compute_quality(app: &AppState) -> Result<DataQualityView, sea_orm::DbErr> {
    let backend = app.db.get_database_backend();

    #[derive(FromQueryResult)]
    struct OneRow {
        n: i64,
    }

    // Orphan sessions (issue row missing — should be 0 with FK).
    let orphan_stmt = Statement::from_string(
        backend,
        "SELECT COUNT(*)::bigint AS n FROM reading_sessions rs \
         LEFT JOIN issues i ON i.id = rs.issue_id WHERE i.id IS NULL"
            .to_string(),
    );
    let orphan_sessions = OneRow::find_by_statement(orphan_stmt)
        .one(&app.db)
        .await?
        .map(|r| r.n)
        .unwrap_or(0);

    // Suspiciously long: active_ms > 6h OR span(started→last_heartbeat) > 12h.
    let long_stmt = Statement::from_string(
        backend,
        "SELECT COUNT(*)::bigint AS n FROM reading_sessions \
         WHERE active_ms > 21600000 \
            OR EXTRACT(EPOCH FROM (last_heartbeat_at - started_at)) > 43200"
            .to_string(),
    );
    let long_sessions = OneRow::find_by_statement(long_stmt)
        .one(&app.db)
        .await?
        .map(|r| r.n)
        .unwrap_or(0);

    // Dangling: still NULL ended_at and heartbeat older than 1h.
    let dangling_stmt = Statement::from_sql_and_values(
        backend,
        "SELECT COUNT(*)::bigint AS n FROM reading_sessions \
         WHERE ended_at IS NULL AND last_heartbeat_at < $1",
        vec![(Utc::now() - Duration::hours(1)).fixed_offset().into()],
    );
    let dangling_sessions = OneRow::find_by_statement(dangling_stmt)
        .one(&app.db)
        .await?
        .map(|r| r.n)
        .unwrap_or(0);

    // Metadata coverage.
    let meta_stmt = Statement::from_string(
        backend,
        "SELECT \
           COUNT(*)::bigint AS total_issues, \
           COUNT(*) FILTER (WHERE writer IS NULL OR writer = '')::bigint AS missing_writer, \
           COUNT(*) FILTER (WHERE cover_artist IS NULL OR cover_artist = '')::bigint AS missing_cover_artist, \
           COUNT(*) FILTER (WHERE page_count IS NULL)::bigint AS missing_page_count, \
           COUNT(*) FILTER (WHERE genre IS NULL OR genre = '')::bigint AS missing_genre, \
           COUNT(*) FILTER (WHERE publisher IS NULL OR publisher = '')::bigint AS missing_publisher \
         FROM issues WHERE state = 'active' AND removed_at IS NULL"
            .to_string(),
    );
    #[derive(FromQueryResult)]
    struct MetaRow {
        total_issues: i64,
        missing_writer: i64,
        missing_cover_artist: i64,
        missing_page_count: i64,
        missing_genre: i64,
        missing_publisher: i64,
    }
    let meta = MetaRow::find_by_statement(meta_stmt)
        .one(&app.db)
        .await?
        .unwrap_or(MetaRow {
            total_issues: 0,
            missing_writer: 0,
            missing_cover_artist: 0,
            missing_page_count: 0,
            missing_genre: 0,
            missing_publisher: 0,
        });

    Ok(DataQualityView {
        orphan_sessions,
        long_sessions,
        dangling_sessions,
        metadata: MetadataCoverageView {
            total_issues: meta.total_issues,
            missing_writer: meta.missing_writer,
            missing_cover_artist: meta.missing_cover_artist,
            missing_page_count: meta.missing_page_count,
            missing_genre: meta.missing_genre,
            missing_publisher: meta.missing_publisher,
        },
    })
}

// ───────── error helpers ─────────

fn internal() -> Response {
    error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}

fn bad(code: &str, message: &str) -> Response {
    error_response(StatusCode::BAD_REQUEST, code, message)
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
