//! `GET /libraries/{id}/scan-runs` — paginated scan history (spec §8.2).
//!
//! Library Scanner v1, Milestone 11.
//!
//! Plus [`prune`] — the trim-to-last-50 helper used by the daily cron.

use axum::{
    Extension, Json, Router,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use entity::{scan_run, series};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection,
    DbBackend, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Statement,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use super::error;
use crate::audit::{self, AuditEntry};
use crate::auth::RequireAdmin;
use crate::library::events::ScanEvent;
use crate::middleware::RequestContext;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/libraries/{slug}/scan-runs", get(list))
        .route("/libraries/{slug}/scan-runs/{scan_id}/cancel", post(cancel))
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
    fn from_model(
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
    get,
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
                StatusCode::BAD_REQUEST,
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
    post,
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
                StatusCode::BAD_REQUEST,
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
