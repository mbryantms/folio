//! `GET /libraries/{id}/scan-runs` — paginated scan history (spec §8.2).
//!
//! Library Scanner v1, Milestone 11.
//!
//! Plus [`prune`] — the trim-to-last-50 helper used by the daily cron.

use axum::{
    Extension, Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, FixedOffset};
use entity::{library, scan_run, series};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, Condition, ConnectionTrait,
    DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, QueryFilter, QueryOrder,
    QuerySelect, Statement,
};
use serde::{Deserialize, Serialize};
use shared::pagination::{CursorPage, decode_cursor, encode_cursor};
use std::collections::{HashMap, HashSet};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::audit::{self, AuditEntry};
use crate::auth::RequireAdmin;
use crate::library::events::ScanEvent;
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(admin_list))
        .routes(routes!(admin_latest_per_library))
        .routes(routes!(cancel))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ScanRunView {
    pub id: String,
    pub state: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub stats: serde_json::Value,
    pub error: Option<String>,
    /// `library` | `series` | `issue` — drives the History tab filter chips.
    pub kind: String,
    /// Target series id for `series` / `issue` kinds. `null` on library scans.
    pub series_id: Option<String>,
    /// Joined `series.name` so the table can render a target label without
    /// chasing one extra request per row. `null` when the series row was
    /// since deleted (orphan scan_runs).
    pub series_name: Option<String>,
    /// Originating issue id for `issue` kinds. `null` otherwise.
    pub issue_id: Option<String>,
    /// Joined issue label shaped as `{series} #{number}` (or `{series} —
    /// {title}` when there's no number). `null` when the issue row was
    /// since deleted, or when this scan isn't issue-kinded.
    pub issue_label: Option<String>,
}

impl ScanRunView {
    pub(crate) fn from_model(
        m: scan_run::Model,
        series_names: &HashMap<Uuid, String>,
        issue_labels: &HashMap<String, String>,
    ) -> Self {
        let issue_label = m
            .issue_id
            .as_deref()
            .and_then(|id| issue_labels.get(id).cloned());
        Self {
            id: m.id.to_string(),
            state: m.state,
            started_at: m.started_at.to_rfc3339(),
            ended_at: m.ended_at.map(|t| t.to_rfc3339()),
            stats: m.stats,
            error: m.error,
            kind: m.kind,
            series_id: m.series_id.map(|u| u.to_string()),
            series_name: m.series_id.and_then(|id| series_names.get(&id).cloned()),
            issue_id: m.issue_id,
            issue_label,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub limit: Option<u64>,
    /// Filter by `kind`: `library` | `series` | `issue`. Omit to return all.
    /// Unknown values 400 so the caller catches typos rather than getting
    /// a silent empty list.
    #[serde(default)]
    pub kind: Option<String>,
}

#[utoipa::path(
    operation_id = "scan_runs_list",    get,
    path = "/libraries/{slug}/scan-runs",
    params(
        ("slug" = String, Path,),
        ("limit" = Option<u64>, Query,),
        ("kind" = Option<String>, Query,),
    ),
    responses(
        (status = 200, body = Vec<ScanRunView>),
        (status = 400, description = "invalid kind filter"),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(slug): AxPath<String>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let lib = match crate::api::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let uuid = lib.id;
    let limit = q.limit.unwrap_or(50).min(500);

    let kind_filter = match q.kind.as_deref() {
        None | Some("") | Some("all") => None,
        Some(k @ ("library" | "series" | "issue")) => Some(k.to_owned()),
        Some(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.kind",
                "kind must be one of: library, series, issue, all",
            );
        }
    };

    let mut query = scan_run::Entity::find()
        .filter(scan_run::Column::LibraryId.eq(uuid))
        .order_by_desc(scan_run::Column::StartedAt)
        .limit(limit);
    if let Some(kind) = kind_filter.as_deref() {
        query = query.filter(scan_run::Column::Kind.eq(kind));
    }

    let rows = match query.all(&app.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "list scan_runs failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    // Batch-resolve series names so the table doesn't fan out one query per
    // row. Skip the lookup entirely when no rows actually reference a series.
    let mut series_ids: HashSet<Uuid> = rows.iter().filter_map(|r| r.series_id).collect();
    let issue_ids: HashSet<String> = rows.iter().filter_map(|r| r.issue_id.clone()).collect();

    // Issue scans don't always carry the originating series_id (the worker
    // sometimes fills only issue_id). Pull it from the issues table so the
    // joined label below can still build the "Series #N" shape.
    let mut issue_meta: HashMap<String, (Uuid, Option<String>, Option<String>)> = HashMap::new();
    if !issue_ids.is_empty() {
        let issue_rows = entity::issue::Entity::find()
            .filter(entity::issue::Column::Id.is_in(issue_ids.iter().cloned().collect::<Vec<_>>()))
            .all(&app.db)
            .await
            .unwrap_or_default();
        for i in issue_rows {
            series_ids.insert(i.series_id);
            issue_meta.insert(i.id.clone(), (i.series_id, i.number_raw, i.title));
        }
    }

    let mut series_names: HashMap<Uuid, String> = HashMap::new();
    if !series_ids.is_empty() {
        let names = series::Entity::find()
            .filter(series::Column::Id.is_in(series_ids.iter().copied().collect::<Vec<_>>()))
            .all(&app.db)
            .await
            .unwrap_or_default();
        for s in names {
            series_names.insert(s.id, s.name);
        }
    }

    let mut issue_labels: HashMap<String, String> = HashMap::new();
    for (id, (sid, number, title)) in issue_meta {
        let series_name = series_names
            .get(&sid)
            .cloned()
            .unwrap_or_else(|| "Unknown series".to_owned());
        let label = match (number.as_deref(), title.as_deref()) {
            (Some(n), _) if !n.is_empty() => format!("{series_name} #{n}"),
            (_, Some(t)) if !t.is_empty() => format!("{series_name} — {t}"),
            _ => series_name,
        };
        issue_labels.insert(id, label);
    }

    Json(
        rows.into_iter()
            .map(|m| ScanRunView::from_model(m, &series_names, &issue_labels))
            .collect::<Vec<_>>(),
    )
    .into_response()
}

/// Cross-library scan-run row. Adds library context fields so the
/// admin findings table can render a Library column without N+1
/// lookups. Same shape as the per-library [`ScanRunView`] otherwise.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CrossLibScanRunView {
    #[serde(flatten)]
    pub base: ScanRunView,
    pub library_id: String,
    pub library_name: String,
    pub library_slug: String,
}

#[derive(Debug, Deserialize)]
pub struct AdminListQuery {
    /// Restrict to one library (UUID). Omit / `all` for cross-library.
    #[serde(default)]
    pub library_id: Option<String>,
    /// `library` | `series` | `issue`. Unknown values 422.
    #[serde(default)]
    pub kind: Option<String>,
    /// `running` | `complete` | `failed` | `cancelled`. Unknown values 422.
    #[serde(default)]
    pub state: Option<String>,
    /// Restrict to runs started at-or-after this RFC3339 timestamp.
    /// Used by the dashboard's "Recent failures (7d)" card.
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[utoipa::path(
    operation_id = "admin_scan_runs_list",    get,
    path = "/admin/scan-runs",
    params(
        ("library_id" = Option<String>, Query,),
        ("kind" = Option<String>, Query,),
        ("state" = Option<String>, Query,),
        ("since" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
        ("cursor" = Option<String>, Query,),
    ),
    responses(
        (status = 200, body = shared::pagination::CursorPage<CrossLibScanRunView>),
        (status = 403, description = "admin only"),
        (status = 422, description = "invalid filter value"),
    )
)]
#[handler]
pub async fn admin_list(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<AdminListQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    let kind_filter = match q.kind.as_deref() {
        None | Some("") | Some("all") => None,
        Some(k @ ("library" | "series" | "issue")) => Some(k.to_owned()),
        Some(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.kind",
                "kind must be one of: library, series, issue, all",
            );
        }
    };

    let state_filter = match q.state.as_deref() {
        None | Some("") | Some("all") => None,
        Some(s @ ("running" | "complete" | "failed" | "cancelled")) => Some(s.to_owned()),
        Some(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.state",
                "state must be one of: running, complete, failed, cancelled, all",
            );
        }
    };

    let library_filter = match q.library_id.as_deref() {
        None | Some("") | Some("all") => None,
        Some(raw) => match Uuid::parse_str(raw) {
            Ok(u) => Some(u),
            Err(_) => {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation.library_id",
                    "library_id must be a UUID or 'all'",
                );
            }
        },
    };

    let since_filter: Option<DateTime<FixedOffset>> = match q.since.as_deref() {
        None | Some("") => None,
        Some(s) => match DateTime::parse_from_rfc3339(s) {
            Ok(t) => Some(t),
            Err(_) => {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation.since",
                    "since must be an RFC3339 timestamp",
                );
            }
        },
    };

    let cursor: Option<(DateTime<FixedOffset>, Uuid)> = match q.cursor.as_deref() {
        None => None,
        Some(c) => match decode_cursor::<(DateTime<FixedOffset>, Uuid)>(c) {
            Ok(parsed) => Some(parsed),
            Err(_) => return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor"),
        },
    };

    let mut query = scan_run::Entity::find();
    if let Some(lib_id) = library_filter {
        query = query.filter(scan_run::Column::LibraryId.eq(lib_id));
    }
    if let Some(kind) = kind_filter.as_deref() {
        query = query.filter(scan_run::Column::Kind.eq(kind));
    }
    if let Some(state) = state_filter.as_deref() {
        query = query.filter(scan_run::Column::State.eq(state));
    }
    if let Some(ts) = since_filter {
        query = query.filter(scan_run::Column::StartedAt.gte(ts));
    }
    if let Some((c_at, c_id)) = cursor {
        query = query.filter(
            Condition::any()
                .add(scan_run::Column::StartedAt.lt(c_at))
                .add(
                    Condition::all()
                        .add(scan_run::Column::StartedAt.eq(c_at))
                        .add(scan_run::Column::Id.lt(c_id)),
                ),
        );
    }
    query = query
        .order_by_desc(scan_run::Column::StartedAt)
        .order_by_desc(scan_run::Column::Id)
        .limit(limit + 1);

    let rows = match query.all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "admin list scan_runs failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let next_cursor = if rows.len() as u64 > limit {
        rows.get((limit - 1) as usize)
            .and_then(|r| encode_cursor(&(r.started_at, r.id)).ok())
    } else {
        None
    };
    let page: Vec<scan_run::Model> = rows.into_iter().take(limit as usize).collect();

    let (series_names, issue_labels, library_meta) = resolve_joins(&app, &page).await;

    let items: Vec<CrossLibScanRunView> = page
        .into_iter()
        .map(|m| {
            let lib_id = m.library_id;
            let (lname, lslug) = library_meta
                .get(&lib_id)
                .cloned()
                .unwrap_or_else(|| (String::from("(deleted library)"), String::new()));
            CrossLibScanRunView {
                library_id: lib_id.to_string(),
                library_name: lname,
                library_slug: lslug,
                base: ScanRunView::from_model(m, &series_names, &issue_labels),
            }
        })
        .collect();

    Json(CursorPage::<CrossLibScanRunView>::paginated(
        items,
        next_cursor,
        None,
    ))
    .into_response()
}

/// Most-recent scan_run row per library. One row per library, ordered
/// oldest-scanned-first so the "What hasn't been touched in months"
/// libraries float to the top — drives the dashboard's "Latest scan
/// per library" card.
#[utoipa::path(
    operation_id = "admin_scan_runs_latest_per_library",    get,
    path = "/admin/scan-runs/latest-per-library",
    responses(
        (status = 200, body = Vec<CrossLibScanRunView>),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn admin_latest_per_library(
    State(app): State<AppState>,
    _admin: RequireAdmin,
) -> impl IntoResponse {
    // `DISTINCT ON` keeps the first row per library_id after the
    // `(library_id, started_at DESC)` order — i.e. the most recent
    // scan per library. Then the outer ORDER BY ascending puts
    // oldest-scanned libraries first.
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT * FROM (
                SELECT DISTINCT ON (library_id)
                    id, library_id, state, started_at, ended_at, stats, error,
                    kind, series_id, issue_id
                FROM scan_runs
                ORDER BY library_id, started_at DESC
            ) latest
            ORDER BY started_at ASC
        "#,
        [],
    );
    let rows = match scan_run::Model::find_by_statement(stmt).all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "latest-per-library scan_runs failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let (series_names, issue_labels, library_meta) = resolve_joins(&app, &rows).await;

    let items: Vec<CrossLibScanRunView> = rows
        .into_iter()
        .map(|m| {
            let lib_id = m.library_id;
            let (lname, lslug) = library_meta
                .get(&lib_id)
                .cloned()
                .unwrap_or_else(|| (String::from("(deleted library)"), String::new()));
            CrossLibScanRunView {
                library_id: lib_id.to_string(),
                library_name: lname,
                library_slug: lslug,
                base: ScanRunView::from_model(m, &series_names, &issue_labels),
            }
        })
        .collect();

    Json(items).into_response()
}

/// Shared join resolution for cross-library scan-run handlers. Pulls
/// series names, issue labels, and library names in three grouped
/// queries so the response can render labels without N+1 fan-out.
pub(crate) async fn resolve_joins(
    app: &AppState,
    rows: &[scan_run::Model],
) -> (
    HashMap<Uuid, String>,
    HashMap<String, String>,
    HashMap<Uuid, (String, String)>,
) {
    let mut series_ids: HashSet<Uuid> = rows.iter().filter_map(|r| r.series_id).collect();
    let issue_ids: HashSet<String> = rows.iter().filter_map(|r| r.issue_id.clone()).collect();
    let library_ids: HashSet<Uuid> = rows.iter().map(|r| r.library_id).collect();

    let mut issue_meta: HashMap<String, (Uuid, Option<String>, Option<String>)> = HashMap::new();
    if !issue_ids.is_empty()
        && let Ok(issue_rows) = entity::issue::Entity::find()
            .filter(entity::issue::Column::Id.is_in(issue_ids.iter().cloned().collect::<Vec<_>>()))
            .all(&app.db)
            .await
    {
        for i in issue_rows {
            series_ids.insert(i.series_id);
            issue_meta.insert(i.id.clone(), (i.series_id, i.number_raw, i.title));
        }
    }

    let series_names: HashMap<Uuid, String> = if series_ids.is_empty() {
        HashMap::new()
    } else {
        series::Entity::find()
            .filter(series::Column::Id.is_in(series_ids.iter().copied().collect::<Vec<_>>()))
            .all(&app.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.id, s.name))
            .collect()
    };

    let issue_labels: HashMap<String, String> = issue_meta
        .into_iter()
        .map(|(id, (sid, number, title))| {
            let series_name = series_names
                .get(&sid)
                .cloned()
                .unwrap_or_else(|| "Unknown series".to_owned());
            let label = match (number.as_deref(), title.as_deref()) {
                (Some(n), _) if !n.is_empty() => format!("{series_name} #{n}"),
                (_, Some(t)) if !t.is_empty() => format!("{series_name} — {t}"),
                _ => series_name,
            };
            (id, label)
        })
        .collect();

    let library_meta: HashMap<Uuid, (String, String)> = if library_ids.is_empty() {
        HashMap::new()
    } else {
        library::Entity::find()
            .filter(library::Column::Id.is_in(library_ids))
            .all(&app.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|l| (l.id, (l.name, l.slug)))
            .collect()
    };

    (series_names, issue_labels, library_meta)
}

/// Trim each library's `scan_runs` history to the most recent `keep` rows.
/// Returns the number of rows deleted across all libraries. Called by the
/// daily cron in [`crate::jobs::scheduler`].
///
/// Implementation notes:
///   - Uses a single SQL statement keyed off `ROW_NUMBER() OVER (PARTITION BY
///     library_id ORDER BY started_at DESC)` so the per-library "last N"
///     behavior is one round-trip rather than one-per-library.
pub async fn prune(db: &DatabaseConnection, keep: u64) -> anyhow::Result<u64> {
    // Postgres-only — the workspace already pins sea-orm to sqlx-postgres.
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            DELETE FROM scan_runs
            WHERE id IN (
                SELECT id FROM (
                    SELECT id, ROW_NUMBER() OVER (
                        PARTITION BY library_id ORDER BY started_at DESC
                    ) AS rn
                    FROM scan_runs
                ) AS sub
                WHERE sub.rn > $1
            )
        "#,
        [(keep as i64).into()],
    );
    let res = db.execute(stmt).await?;
    Ok(res.rows_affected())
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ScanCancelResp {
    pub id: String,
    pub state: String,
    pub ended_at: Option<String>,
    pub error: Option<String>,
}

/// `POST /libraries/{slug}/scan-runs/{scan_id}/cancel` — flip a stuck
/// scan_runs row to a terminal state. Idempotent against already-
/// terminal rows (returns 409). The worker is the usual authority on
/// state transitions; this endpoint is the manual escape hatch for
/// scans that have lost their worker (e.g., operator cleared the
/// queue mid-flight, server restart killed an in-flight job, or the
/// worker hangs and no progress events arrive).
///
/// Writes `state="cancelled"`, `ended_at=NOW`, `error="Cancelled by
/// admin"`. Emits `ScanEvent::Failed` so connected WebSocket clients
/// (the Live scan page) drop the run out of their "active" set
/// immediately. Race note: if a still-alive worker subsequently
/// reaches `finalize_run`, it will overwrite this row with its own
/// terminal state — that's fine and expected.
#[utoipa::path(
    operation_id = "scan_runs_cancel",    post,
    path = "/libraries/{slug}/scan-runs/{scan_id}/cancel",
    params(
        ("slug" = String, Path,),
        ("scan_id" = String, Path,),
    ),
    responses(
        (status = 200, body = ScanCancelResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "library or scan run not found"),
        (status = 409, description = "scan run already terminal"),
    )
)]
#[handler]
pub async fn cancel(
    State(app): State<AppState>,
    admin: RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath((slug, scan_id)): AxPath<(String, String)>,
) -> impl IntoResponse {
    let lib = match crate::api::libraries::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(_) => return error(StatusCode::NOT_FOUND, "not_found", "library not found"),
    };
    let scan_uuid = match Uuid::parse_str(&scan_id) {
        Ok(u) => u,
        Err(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "scan_id must be a UUID",
            );
        }
    };
    let row = match scan_run::Entity::find_by_id(scan_uuid).one(&app.db).await {
        Ok(Some(r)) => r,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "scan run not found"),
        Err(e) => {
            tracing::warn!(error = %e, "scan cancel: lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if row.library_id != lib.id {
        return error(StatusCode::NOT_FOUND, "not_found", "scan run not found");
    }
    if matches!(row.state.as_str(), "complete" | "failed" | "cancelled") {
        return error(
            StatusCode::CONFLICT,
            "already_terminal",
            &format!("scan run already in terminal state: {}", row.state),
        );
    }
    let now = chrono::Utc::now().fixed_offset();
    let cancel_msg = "Cancelled by admin".to_string();
    let mut am: scan_run::ActiveModel = row.into();
    am.state = Set("cancelled".into());
    am.ended_at = Set(Some(now));
    am.error = Set(Some(cancel_msg.clone()));
    let updated = match am.update(&app.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "scan cancel: row update failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    // Connected Live-scan clients listen for `scan.failed`; emit one
    // so the UI drops the run out of its "active" set and surfaces a
    // terminal status without a page reload.
    app.events.emit(ScanEvent::Failed {
        library_id: lib.id,
        scan_id: scan_uuid,
        error: cancel_msg.clone(),
        batch_id: updated.batch_id,
    });

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: admin.0.id,
            action: "admin.scan_run.cancel",
            target_type: Some("scan_run"),
            target_id: Some(scan_uuid.to_string()),
            payload: serde_json::json!({
                "library_id": lib.id.to_string(),
                "kind": updated.kind,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(ScanCancelResp {
        id: updated.id.to_string(),
        state: updated.state,
        ended_at: updated.ended_at.map(|t| t.to_rfc3339()),
        error: updated.error,
    })
    .into_response()
}
