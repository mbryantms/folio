//! Provider-independent series coverage — analyse which provider series
//! hold which local issues across ComicVine, Metron and GCD, then accept a
//! provider's proposal (main series id + range rows).
//!
//! - `POST /series/{slug}/provider-coverage/analyze` queues the background
//!   analysis ([`crate::jobs::provider_coverage`]) and returns its job id
//!   (`202`). A series with an analysis in flight gets that job back.
//! - `GET /series/{slug}/provider-coverage/analysis` returns the latest
//!   job's state and, once done, the coverage grid. The grid is rebuilt
//!   from the stored candidates + the current DB rows on every read.
//! - `POST /series/{slug}/provider-coverage/accept` writes one provider's
//!   proposal, optionally with a chosen main series ("Choose series").
//!
//! Admin-only; the mutating routes are audited
//! (`admin.series.provider_coverage_analyze` / `…_accept`). The engine is
//! [`crate::metadata::coverage`].

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::error;
use crate::auth::RequireAdmin;
use crate::jobs::provider_coverage::{self, CoverageJobState};
use crate::metadata::coverage::{
    self, AcceptOutcome, CoverageLocalIssue, ProviderCoverageView, SeriesFacts,
};
use crate::metadata::identifier::Source;
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(analyze))
        .routes(routes!(coverage_analysis))
        .routes(routes!(accept))
}

#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct AnalyzeCoverageReq {
    /// Accept high-confidence, conflict-free proposals automatically
    /// (series id as provider-set, ranges as automated rows).
    #[serde(default)]
    pub auto_accept: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CoverageJobResp {
    pub job_id: String,
    pub state: CoverageJobState,
    /// `false` when an analysis for this series was already in flight and
    /// its job is returned instead.
    pub queued: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CoverageAnalysisResp {
    pub job_id: String,
    pub series_id: String,
    pub state: CoverageJobState,
    pub auto_accept: bool,
    pub requested_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    /// The grid's rows (empty until the job is done).
    pub local_issues: Vec<CoverageLocalIssue>,
    /// One column per provider (empty until the job is done).
    pub providers: Vec<ProviderCoverageView>,
    /// Proposals accepted automatically by this job.
    pub auto_accepted: Vec<AcceptOutcome>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AcceptCoverageReq {
    /// `comicvine` | `metron` | `gcd`.
    pub source: String,
    /// Use this candidate as the main series instead of the largest
    /// coverer ("Choose series"). Must be one of the analysis' candidates.
    #[serde(default)]
    pub main_series_id: Option<String>,
}

#[utoipa::path(
    operation_id = "provider_coverage_analyze", post,
    path = "/series/{slug}/provider-coverage/analyze",
    params(("slug" = String, Path)),
    request_body = AnalyzeCoverageReq,
    responses(
        (status = 202, body = CoverageJobResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn analyze(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    Json(req): Json<AnalyzeCoverageReq>,
) -> Response {
    let s = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let (rec, queued) =
        match provider_coverage::enqueue(&app, s.id, actor.id, req.auto_accept).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(series_id = %s.id, error = %e, "provider coverage enqueue failed");
                return error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "metadata.coverage_enqueue_failed",
                    "couldn't queue the coverage analysis",
                );
            }
        };
    crate::record_admin_action!(
        db = &app.db,
        ctx = ctx,
        actor = actor.id,
        action = "admin.series.provider_coverage_analyze",
        target = ("series", s.id.to_string()),
        payload = serde_json::json!({
            "job_id": rec.job_id,
            "queued": queued,
            "auto_accept": rec.auto_accept,
        }),
    );
    (
        StatusCode::ACCEPTED,
        Json(CoverageJobResp {
            job_id: rec.job_id.to_string(),
            state: rec.state,
            queued,
        }),
    )
        .into_response()
}

#[utoipa::path(
    operation_id = "provider_coverage_analysis", get,
    path = "/series/{slug}/provider-coverage/analysis",
    params(("slug" = String, Path)),
    responses(
        (status = 200, body = CoverageAnalysisResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found / never analysed"),
    )
)]
#[handler]
pub async fn coverage_analysis(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Path(slug): Path<String>,
) -> Response {
    let s = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let Some(rec) = provider_coverage::latest_for_series(&app.jobs.redis, s.id).await else {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.coverage_not_found",
            "this series hasn't been analysed yet",
        );
    };
    let mut resp = CoverageAnalysisResp {
        job_id: rec.job_id.to_string(),
        series_id: rec.series_id.to_string(),
        state: rec.state,
        auto_accept: rec.auto_accept,
        requested_at: rec.requested_at,
        started_at: rec.started_at,
        finished_at: rec.finished_at,
        error: rec.error.clone(),
        local_issues: Vec::new(),
        providers: Vec::new(),
        auto_accepted: rec.auto_accepted.clone(),
    };
    if rec.state == CoverageJobState::Done {
        let facts = match SeriesFacts::load(&app, &s).await {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(error = %e, "provider coverage: load series facts failed");
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "couldn't load the series",
                );
            }
        };
        resp.local_issues = coverage::local_view(&facts.local);
        resp.providers = rec
            .providers
            .iter()
            .map(|p| coverage::build_view(p, &facts.local, &facts.ext_ids, &facts.ranges, None))
            .collect();
        // GCD series found only through a link carry no name in the
        // analysis; the issue-list cache usually has it.
        for p in &mut resp.providers {
            let Ok(src) = Source::from_str(&p.source) else {
                continue;
            };
            for c in &mut p.candidates {
                if c.name.is_none()
                    && let Some((n, y)) =
                        coverage::cached_series_label(&app.jobs.redis, src, &c.provider_series_id)
                            .await
                {
                    c.name = n;
                    c.year = c.year.or(y);
                }
            }
        }
    }
    Json(resp).into_response()
}

#[utoipa::path(
    operation_id = "provider_coverage_accept", post,
    path = "/series/{slug}/provider-coverage/accept",
    params(("slug" = String, Path)),
    request_body = AcceptCoverageReq,
    responses(
        (status = 200, body = AcceptOutcome),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found / never analysed"),
        (status = 409, description = "the analysis hasn't finished"),
        (status = 422, description = "unknown source / series not a candidate"),
    )
)]
#[handler]
pub async fn accept(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    Json(req): Json<AcceptCoverageReq>,
) -> Response {
    let s = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let Ok(source) = Source::from_str(&req.source) else {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "metadata.invalid_source",
            "unknown source",
        );
    };
    let Some(rec) = provider_coverage::latest_for_series(&app.jobs.redis, s.id).await else {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.coverage_not_found",
            "this series hasn't been analysed yet",
        );
    };
    if rec.state != CoverageJobState::Done {
        return error(
            StatusCode::CONFLICT,
            "metadata.coverage_not_ready",
            "the coverage analysis hasn't finished",
        );
    }
    let Some(analysis) = rec.providers.iter().find(|p| p.source == source) else {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "metadata.coverage_source_missing",
            "that provider wasn't part of the analysis",
        );
    };
    let main = req
        .main_series_id
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty());
    if let Some(m) = main
        && !analysis
            .candidates
            .iter()
            .any(|c| c.provider_series_id == m)
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "metadata.coverage_not_a_candidate",
            "that series isn't one of the analysed candidates",
        );
    }
    if analysis.candidates.is_empty() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "metadata.coverage_nothing_to_accept",
            "no provider series covers this series",
        );
    }

    let outcome = match coverage::accept_provider(&app, s.id, analysis, main, true).await {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(error = %e, "provider coverage accept failed");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "couldn't write the coverage",
            );
        }
    };
    if !outcome.ranges_created.is_empty() {
        crate::jobs::relationship_suggest::enqueue(&app, s.library_id).await;
    }
    crate::record_admin_action!(
        db = &app.db,
        ctx = ctx,
        actor = actor.id,
        action = "admin.series.provider_coverage_accept",
        target = ("series", s.id.to_string()),
        payload = serde_json::json!({
            "auto": false,
            "job_id": rec.job_id,
            "source": source.as_str(),
            "chosen_main": main,
            "main_series_id": outcome.main_series_id,
            "main_written": outcome.main_written,
            "ranges_created": outcome.ranges_created.iter()
                .map(|r| serde_json::json!({
                    "provider_series_id": r.provider_series_id,
                    "low": r.low,
                    "high": r.high,
                }))
                .collect::<Vec<_>>(),
            "ranges_skipped": outcome.ranges_skipped.len(),
        }),
    );
    Json(outcome).into_response()
}
