//! `POST /scans/selection` — rescan a multi-select from any list surface.
//!
//! Each selected series gets a series-scoped scan and each selected issue an
//! issue-scoped scan, through the same coalescing path as the per-item
//! "Scan series" / "Scan issue" buttons
//! ([`crate::jobs::Jobs::coalesce_scoped_scan`]). An issue whose series is
//! also selected is left to that series' scan. Unlike the per-item buttons
//! (which default to a content-verify pass), the bulk rescan defaults to the
//! cheap size+mtime fast path: a selection can name a hundred series.

use axum::{
    Extension, Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{issue, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::{Deserialize, Serialize};
use server_macros::handler;
use std::collections::{HashMap, HashSet};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::api::extractors::Validated;
use crate::audit::{self, AuditEntry};
use crate::auth::RequireAdmin;
use crate::jobs::scan_series::JobKind;
use crate::library::access;
use crate::middleware::RequestContext;
use crate::state::AppState;

/// Most issues one bulk rescan may name.
pub const SCAN_SELECTION_MAX_ISSUES: usize = 500;
/// Most series one bulk rescan may name.
pub const SCAN_SELECTION_MAX_SERIES: usize = 100;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(scan_selection))
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct ScanSelectionReq {
    #[serde(default)]
    #[garde(length(max = SCAN_SELECTION_MAX_ISSUES))]
    pub issue_ids: Vec<String>,
    #[serde(default)]
    #[garde(length(max = SCAN_SELECTION_MAX_SERIES))]
    pub series_ids: Vec<Uuid>,
    /// Bypass the size+mtime fast path (content-verify every file).
    /// Defaults to `false`.
    #[serde(default)]
    #[garde(skip)]
    pub force: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ScanSelectionResp {
    /// Scans newly queued.
    pub queued: usize,
    /// Targets that joined a scan already queued or running — including
    /// selected issues covered by their selected series' scan.
    pub coalesced: usize,
    /// Unknown, inactive or removed targets (nothing queued).
    pub not_found: usize,
    /// Targets whose enqueue failed.
    pub failed: usize,
}

#[utoipa::path(
    operation_id = "scans_scan_selection",    post,
    path = "/scans/selection",
    request_body = ScanSelectionReq,
    responses(
        (status = 202, body = ScanSelectionResp),
        (status = 403, description = "admin only"),
        (status = 422, description = "empty selection, or over the cap"),
    )
)]
#[handler]
pub async fn scan_selection(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Validated(req): Validated<ScanSelectionReq>,
) -> Response {
    if req.issue_ids.is_empty() && req.series_ids.is_empty() {
        return crate::api::validation("select at least one issue or series");
    }

    let series_ids: Vec<Uuid> = {
        let mut seen = HashSet::new();
        req.series_ids
            .iter()
            .copied()
            .filter(|id| seen.insert(*id))
            .collect()
    };
    let issue_ids: Vec<String> = {
        let mut seen = HashSet::new();
        req.issue_ids
            .iter()
            .filter(|id| seen.insert(id.as_str()))
            .cloned()
            .collect()
    };

    // (id, library, folder) for live selected series.
    let series_rows: Vec<(Uuid, Uuid, Option<String>)> = if series_ids.is_empty() {
        Vec::new()
    } else {
        match access::active_series_select()
            .select_only()
            .column(series::Column::Id)
            .column(series::Column::LibraryId)
            .column(series::Column::FolderPath)
            .filter(series::Column::Id.is_in(series_ids.iter().copied()))
            .into_tuple()
            .all(&app.db)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "scan_selection: series lookup failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    };

    // (id, series, library) for live selected issues in live series.
    let issue_rows: Vec<(String, Uuid, Uuid)> = if issue_ids.is_empty() {
        Vec::new()
    } else {
        let rows: Vec<(String, Uuid, Uuid)> = match issue::Entity::find()
            .select_only()
            .column(issue::Column::Id)
            .column(issue::Column::SeriesId)
            .column(issue::Column::LibraryId)
            .filter(issue::Column::State.eq(issue::IssueState::Active))
            .filter(issue::Column::RemovedAt.is_null())
            .filter(issue::Column::Id.is_in(issue_ids.iter().cloned()))
            .into_tuple()
            .all(&app.db)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "scan_selection: issue lookup failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
        let parents: Vec<Uuid> = rows.iter().map(|(_, s, _)| *s).collect();
        let live: HashSet<Uuid> = match access::active_series_select()
            .select_only()
            .column(series::Column::Id)
            .filter(series::Column::Id.is_in(parents))
            .into_tuple::<Uuid>()
            .all(&app.db)
            .await
        {
            Ok(ids) => ids.into_iter().collect(),
            Err(e) => {
                tracing::error!(error = %e, "scan_selection: series lookup failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
        let by_id: HashMap<String, (Uuid, Uuid)> = rows
            .into_iter()
            .filter(|(_, s, _)| live.contains(s))
            .map(|(id, s, l)| (id, (s, l)))
            .collect();
        issue_ids
            .iter()
            .filter_map(|id| by_id.get(id).map(|(s, l)| (id.clone(), *s, *l)))
            .collect()
    };

    let mut resp = ScanSelectionResp {
        queued: 0,
        coalesced: 0,
        not_found: (series_ids.len() - series_rows.len()) + (issue_ids.len() - issue_rows.len()),
        failed: 0,
    };

    let selected_series: HashSet<Uuid> = series_rows.iter().map(|(id, _, _)| *id).collect();
    for (series_id, library_id, folder) in &series_rows {
        match app
            .jobs
            .coalesce_scoped_scan(
                *library_id,
                *series_id,
                folder.clone(),
                JobKind::Series,
                None,
                req.force,
            )
            .await
        {
            Ok(o) if o.was_coalesced() => resp.coalesced += 1,
            Ok(_) => resp.queued += 1,
            Err(e) => {
                tracing::warn!(series_id = %series_id, error = %e, "scan_selection: series enqueue failed");
                resp.failed += 1;
            }
        }
    }
    for (issue_id, series_id, library_id) in &issue_rows {
        if selected_series.contains(series_id) {
            resp.coalesced += 1;
            continue;
        }
        match app
            .jobs
            .coalesce_scoped_scan(
                *library_id,
                *series_id,
                None,
                JobKind::Issue,
                Some(issue_id.clone()),
                req.force,
            )
            .await
        {
            Ok(o) if o.was_coalesced() => resp.coalesced += 1,
            Ok(_) => resp.queued += 1,
            Err(e) => {
                tracing::warn!(issue_id = %issue_id, error = %e, "scan_selection: issue enqueue failed");
                resp.failed += 1;
            }
        }
    }

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "admin.scan.selection",
            target_type: None,
            target_id: None,
            payload: serde_json::json!({
                "series_ids": series_rows.iter().map(|(id, _, _)| id.to_string()).collect::<Vec<_>>(),
                "issue_ids": issue_rows.iter().map(|(id, _, _)| id.clone()).collect::<Vec<_>>(),
                "force": req.force,
                "queued": resp.queued,
                "coalesced": resp.coalesced,
                "not_found": resp.not_found,
                "failed": resp.failed,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    (StatusCode::ACCEPTED, Json(resp)).into_response()
}
