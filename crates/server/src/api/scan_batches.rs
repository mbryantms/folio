//! `GET /admin/scan-batches` + `/admin/scan-batches/{id}` — observability-split
//! M7. Read surface over the `scan_batch` grouping created by "Scan all" (M6).
//!
//! The list powers the Scan-all dashboard's recent-batches rail; the detail
//! powers a single batch's roll-up: per-library member runs, aggregated
//! `ScanStats` totals, and a count of durable `library_events` to drill into
//! (the per-event list is the Library activity log filtered by `batch_id`,
//! Slice 3).
//!
//! Both are read-only admin GETs → allowlisted in the audit-check tool.

use axum::{
    Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, FixedOffset};
use entity::{library_event, scan_batch, scan_run};
use sea_orm::{
    ColumnTrait, Condition, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
};
use serde::{Deserialize, Serialize};
use shared::pagination::{CursorPage, decode_cursor, encode_cursor};
use std::collections::HashMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use super::scan_runs::{CrossLibScanRunView, ScanRunView, resolve_joins};
use crate::auth::RequireAdmin;
use crate::library::scanner::ScanStats;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(scan_batches_list))
        .routes(routes!(scan_batch_detail))
}

/// Per-state tally of a batch's member runs — drives the dashboard progress
/// bar without the client refetching the run list.
#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct BatchRunTally {
    pub queued: u64,
    pub running: u64,
    pub complete: u64,
    pub failed: u64,
    pub cancelled: u64,
}

impl BatchRunTally {
    fn bump(&mut self, state: &str) {
        match state {
            "queued" => self.queued += 1,
            "running" => self.running += 1,
            "complete" => self.complete += 1,
            "failed" => self.failed += 1,
            "cancelled" => self.cancelled += 1,
            _ => {}
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ScanBatchView {
    pub id: String,
    /// Trigger discriminator — today always `scan_all`.
    pub kind: String,
    pub actor_id: Option<String>,
    pub force: bool,
    pub started_at: String,
    pub ended_at: Option<String>,
    /// Number of runs that adopted the batch (newly-enqueued libraries).
    pub library_count: i32,
    /// `running` | `complete` | `partial_failed` | `failed`.
    pub state: String,
    /// Live per-state breakdown of the member runs.
    pub runs: BatchRunTally,
}

impl ScanBatchView {
    fn from_model(m: scan_batch::Model, runs: BatchRunTally) -> Self {
        Self {
            id: m.id.to_string(),
            kind: m.kind,
            actor_id: m.actor_id.map(|a| a.to_string()),
            force: m.force,
            started_at: m.started_at.to_rfc3339(),
            ended_at: m.ended_at.map(|t| t.to_rfc3339()),
            library_count: m.library_count,
            state: m.state,
            runs,
        }
    }
}

/// Aggregated `ScanStats` counters summed across a batch's member runs — the
/// "what did this Scan all do overall" roll-up.
#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct BatchTotals {
    pub files_seen: u64,
    pub files_added: u64,
    pub files_updated: u64,
    pub issues_removed: u64,
    pub issues_restored: u64,
    pub series_created: u64,
    pub files_malformed: u64,
    pub files_duplicate: u64,
}

impl BatchTotals {
    fn add(&mut self, s: &ScanStats) {
        self.files_seen += s.files_seen;
        self.files_added += s.files_added;
        self.files_updated += s.files_updated;
        self.issues_removed += s.issues_removed;
        self.issues_restored += s.issues_restored;
        self.series_created += s.series_created;
        self.files_malformed += s.files_malformed;
        self.files_duplicate += s.files_duplicate;
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ScanBatchDetailView {
    #[serde(flatten)]
    pub batch: ScanBatchView,
    /// Every member run, newest first, with library context.
    pub member_runs: Vec<CrossLibScanRunView>,
    /// Aggregated counters across the member runs.
    pub totals: BatchTotals,
    /// Number of durable `library_events` recorded under this batch. The web
    /// detail links to the Library activity log filtered by `batch_id` to
    /// drill into the itemized manifest.
    pub event_count: u64,
}

/// Build per-batch [`BatchRunTally`]s for a set of batch ids in one query.
async fn tally_runs(app: &AppState, batch_ids: &[Uuid]) -> HashMap<Uuid, BatchRunTally> {
    let mut out: HashMap<Uuid, BatchRunTally> = HashMap::new();
    if batch_ids.is_empty() {
        return out;
    }
    let runs = scan_run::Entity::find()
        .filter(scan_run::Column::BatchId.is_in(batch_ids.to_vec()))
        .all(&app.db)
        .await
        .unwrap_or_default();
    for r in runs {
        if let Some(b) = r.batch_id {
            out.entry(b).or_default().bump(&r.state);
        }
    }
    out
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// `running` | `complete` | `partial_failed` | `failed`. Unknown 422.
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[utoipa::path(
    operation_id = "admin_scan_batches_list",
    get,
    path = "/admin/scan-batches",
    params(
        ("state" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
        ("cursor" = Option<String>, Query,),
    ),
    responses(
        (status = 200, body = shared::pagination::CursorPage<ScanBatchView>),
        (status = 403, description = "admin only"),
        (status = 422, description = "invalid filter value"),
    )
)]
#[handler]
pub async fn scan_batches_list(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    let state_filter = match q.state.as_deref() {
        None | Some("") | Some("all") => None,
        Some(s @ ("running" | "complete" | "partial_failed" | "failed")) => Some(s.to_owned()),
        Some(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.state",
                "state must be one of: running, complete, partial_failed, failed, all",
            );
        }
    };

    let cursor: Option<(DateTime<FixedOffset>, Uuid)> = match q.cursor.as_deref() {
        None => None,
        Some(c) => match decode_cursor::<(DateTime<FixedOffset>, Uuid)>(c) {
            Ok(parsed) => Some(parsed),
            Err(_) => return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor"),
        },
    };

    let mut query = scan_batch::Entity::find();
    if let Some(state) = state_filter.as_deref() {
        query = query.filter(scan_batch::Column::State.eq(state));
    }
    if let Some((c_at, c_id)) = cursor {
        query = query.filter(
            Condition::any()
                .add(scan_batch::Column::StartedAt.lt(c_at))
                .add(
                    Condition::all()
                        .add(scan_batch::Column::StartedAt.eq(c_at))
                        .add(scan_batch::Column::Id.lt(c_id)),
                ),
        );
    }
    query = query
        .order_by_desc(scan_batch::Column::StartedAt)
        .order_by_desc(scan_batch::Column::Id)
        .limit(limit + 1);

    let rows = match query.all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "admin list scan_batches failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let next_cursor = if rows.len() as u64 > limit {
        rows.get((limit - 1) as usize)
            .and_then(|r| encode_cursor(&(r.started_at, r.id)).ok())
    } else {
        None
    };
    let page: Vec<scan_batch::Model> = rows.into_iter().take(limit as usize).collect();

    let batch_ids: Vec<Uuid> = page.iter().map(|b| b.id).collect();
    let mut tallies = tally_runs(&app, &batch_ids).await;
    let items: Vec<ScanBatchView> = page
        .into_iter()
        .map(|m| {
            let tally = tallies.remove(&m.id).unwrap_or_default();
            ScanBatchView::from_model(m, tally)
        })
        .collect();

    Json(CursorPage::<ScanBatchView>::paginated(
        items,
        next_cursor,
        None,
    ))
    .into_response()
}

#[utoipa::path(
    operation_id = "admin_scan_batches_detail",
    get,
    path = "/admin/scan-batches/{id}",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = ScanBatchDetailView),
        (status = 403, description = "admin only"),
        (status = 404, description = "batch not found"),
    )
)]
#[handler]
pub async fn scan_batch_detail(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(id): AxPath<String>,
) -> impl IntoResponse {
    let batch_id = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "batch id must be a UUID",
            );
        }
    };

    let batch = match scan_batch::Entity::find_by_id(batch_id).one(&app.db).await {
        Ok(Some(b)) => b,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "scan batch not found"),
        Err(e) => {
            tracing::error!(error = %e, %batch_id, "load scan_batch failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let runs = match scan_run::Entity::find()
        .filter(scan_run::Column::BatchId.eq(batch_id))
        .order_by_desc(scan_run::Column::StartedAt)
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, %batch_id, "load batch member runs failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    // Tally + aggregate in one pass over the member runs.
    let mut tally = BatchRunTally::default();
    let mut totals = BatchTotals::default();
    for r in &runs {
        tally.bump(&r.state);
        if let Ok(stats) = serde_json::from_value::<ScanStats>(r.stats.clone()) {
            totals.add(&stats);
        }
    }

    let (series_names, issue_labels, library_meta) = resolve_joins(&app, &runs).await;
    let member_runs: Vec<CrossLibScanRunView> = runs
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

    let event_count = library_event::Entity::find()
        .filter(library_event::Column::BatchId.eq(batch_id))
        .count(&app.db)
        .await
        .unwrap_or(0);

    Json(ScanBatchDetailView {
        batch: ScanBatchView::from_model(batch, tally),
        member_runs,
        totals,
        event_count,
    })
    .into_response()
}
