//! `GET /libraries/{id}/scan-runs` — paginated scan history (spec §8.2).
//!
//! Library Scanner v1, Milestone 11.
//!
//! Plus [`prune`] — the trim-to-last-50 helper used by the daily cron.

use axum::{
    Json, Router,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use entity::{scan_run, series};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Statement,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::auth::RequireAdmin;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/libraries/{slug}/scan-runs", get(list))
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
}

impl ScanRunView {
    fn from_model(m: scan_run::Model, series_names: &HashMap<Uuid, String>) -> Self {
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
    let series_ids: HashSet<Uuid> = rows.iter().filter_map(|r| r.series_id).collect();
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

    Json(
        rows.into_iter()
            .map(|m| ScanRunView::from_model(m, &series_names))
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

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
