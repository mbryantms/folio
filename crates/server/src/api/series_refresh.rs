//! Guided "Refresh this series…" flow — one read-only status endpoint the
//! web stepper resumes from (coverage tie-ins PR 3).
//!
//! The flow itself only drives existing endpoints, in order:
//!
//! 1. **Series match** — `POST /series/{slug}/metadata/search` →
//!    candidates (with coverage hints) → `POST …/metadata/apply`, or
//!    "keep current match" when the series already has provider ids.
//! 2. **Coverage** — the analysis the apply queued (seeded, see
//!    `jobs::provider_coverage::enqueue_after_series_apply`) or
//!    `POST …/provider-coverage/analyze`; accept per provider.
//! 3. **Per-issue fetch** — `POST …/metadata/batch?scope=all|incomplete`
//!    (direct lookups through series coverage, see
//!    `metadata::direct_lookup`).
//! 4. **Review** — `GET /metadata/batch/{id}` + `POST …/apply`.
//!
//! `GET /series/{slug}/metadata/refresh-status` aggregates the server
//! state those steps leave behind — the latest series run and apply, the
//! coverage job record (Redis, 24 h), and the series' latest metadata
//! batch — so closing the dialog mid-flow and reopening it lands on the
//! step it got to without a new table. It also estimates the per-issue
//! step's provider calls (issues answered by a direct lookup vs a search)
//! for both batch scopes. Nothing here writes or calls a provider.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Duration, Utc};
use sea_orm::{ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Statement};
use serde::Serialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use super::metadata_search::{SeriesBatchScope, series_batch_selection};
use crate::auth::RequireAdmin;
use crate::jobs::provider_coverage::{self, CoverageJobState, CoverageTrigger};
use crate::metadata::coverage::COVERAGE_SOURCES;
use crate::metadata::identifier::Source;
use crate::metadata::orchestrator;
use crate::metadata::range_map;
use crate::state::AppState;
use server_macros::handler;

/// How far back the flow looks for its own state: the coverage record's
/// lifetime, so the three steps share one window.
pub const RESUME_WINDOW_SECS: i64 = provider_coverage::RECORD_TTL_SECS as i64;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(refresh_status))
}

/// The guided flow's steps, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefreshStep {
    /// Pick / apply a series match (or keep the current one).
    Match,
    /// Run or reuse the coverage analysis; accept per provider.
    Coverage,
    /// Start (or watch) the per-issue batch.
    Fetch,
    /// The batch finished: accept its results.
    Review,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesRefreshStatusResp {
    pub series_id: String,
    /// Where a reopened dialog resumes (see the module docs for the rule).
    pub resume_step: RefreshStep,
    pub series_match: SeriesMatchState,
    /// The latest coverage job, when its record is still kept (24 h).
    pub coverage: Option<RefreshCoverageJob>,
    /// `metadata.coverage_after_series_apply` (`off` | `manual_only` |
    /// `all`): whether a match applied in step 1 queues the analysis
    /// itself, or step 2 has to run it.
    pub coverage_after_series_apply: String,
    /// The series' latest metadata batch inside the resume window.
    pub batch: Option<RefreshBatch>,
    /// Provider-call estimate for the per-issue step, one entry per batch
    /// scope (`all`, then `incomplete`).
    pub fetch_estimate: Vec<FetchScopeEstimate>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesMatchState {
    /// The series' provider ids (`external_ids`) — a non-empty list is a
    /// confirmed match the user may keep.
    pub links: Vec<SeriesProviderLink>,
    /// Latest series search run (any age), for the match step's status.
    pub latest_run: Option<RefreshRun>,
    /// When a series candidate was last applied, if inside the window.
    pub applied_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesProviderLink {
    pub source: String,
    pub external_id: String,
    /// `user` | `provider:<source>` | … (`external_ids.set_by`).
    pub set_by: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RefreshRun {
    pub run_id: Uuid,
    pub status: String,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RefreshCoverageJob {
    pub job_id: String,
    pub state: CoverageJobState,
    pub trigger: CoverageTrigger,
    pub requested_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Providers the job analyses (a seeded job: only the matched ones).
    pub sources: Vec<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RefreshBatch {
    pub batch_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub items_total: i32,
    /// Children still queued / searching / parked on quota.
    pub unfinished: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FetchScopeEstimate {
    pub scope: SeriesBatchScope,
    /// Issues the batch would search (at most the 200-per-run cap).
    pub issues: i64,
    /// Issues the scope covers before the cap and the recent-fetch skip.
    pub eligible: i64,
    /// `all` only: eligible issues skipped because they were searched in
    /// the last 24 hours (a re-trigger walks on to the next chunk).
    pub recently_fetched: i64,
    /// Eligible issues this batch would not reach; a second run with the
    /// same scope takes them.
    pub remainder: i64,
    /// Per enabled provider (ComicVine, Metron, GCD order).
    pub providers: Vec<ProviderFetchEstimate>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProviderFetchEstimate {
    pub source: String,
    /// Issues with a provider series (a covering range or the series id):
    /// looked up directly — one detail request, no search — unless the
    /// listing doesn't hold them (then they fall back to a search).
    pub direct: i64,
    /// Issues without one: a provider search each (1–2 requests).
    pub search: i64,
}

#[utoipa::path(
    operation_id = "metadata_series_refresh_status", get,
    path = "/series/{slug}/metadata/refresh-status",
    params(("slug" = String, Path)),
    responses(
        (status = 200, body = SeriesRefreshStatusResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn refresh_status(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Path(slug): Path<String>,
) -> Response {
    let s = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    match build_status(&app, &s).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => {
            tracing::error!(series_id = %s.id, error = %e, "series refresh status failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

async fn build_status(
    app: &AppState,
    s: &entity::series::Model,
) -> Result<SeriesRefreshStatusResp, sea_orm::DbErr> {
    let window_start = Utc::now() - Duration::seconds(RESUME_WINDOW_SECS);
    let sid = s.id.to_string();

    let links: Vec<SeriesProviderLink> = entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq("series"))
        .filter(entity::external_id::Column::EntityId.eq(sid.clone()))
        .all(&app.db)
        .await?
        .into_iter()
        .map(|e| SeriesProviderLink {
            source: e.source,
            external_id: e.external_id,
            set_by: e.set_by,
        })
        .collect();

    let latest_run = app
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, status, started_at FROM metadata_run \
             WHERE scope = 'series' AND scope_entity_id = $1 \
             ORDER BY started_at DESC LIMIT 1",
            [sid.clone().into()],
        ))
        .await?
        .map(|r| -> Result<RefreshRun, sea_orm::DbErr> {
            Ok(RefreshRun {
                run_id: r.try_get("", "id")?,
                status: r.try_get("", "status")?,
                started_at: r
                    .try_get::<DateTime<chrono::FixedOffset>>("", "started_at")?
                    .with_timezone(&Utc),
            })
        })
        .transpose()?;

    let applied_at: Option<DateTime<Utc>> = app
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT max(c.applied_at) AS applied_at FROM metadata_run_candidate c \
             JOIN metadata_run r ON r.id = c.run_id \
             WHERE r.scope = 'series' AND r.scope_entity_id = $1 \
               AND c.applied_at IS NOT NULL",
            [sid.clone().into()],
        ))
        .await?
        .and_then(|r| {
            r.try_get::<Option<DateTime<chrono::FixedOffset>>>("", "applied_at")
                .ok()
                .flatten()
        })
        .map(|t| t.with_timezone(&Utc))
        .filter(|t| *t >= window_start);

    let coverage = provider_coverage::latest_for_series(&app.jobs.redis, s.id)
        .await
        .filter(|rec| rec.requested_at >= window_start)
        .map(|rec| RefreshCoverageJob {
            job_id: rec.job_id.to_string(),
            state: rec.state,
            trigger: rec.trigger,
            requested_at: rec.requested_at,
            finished_at: rec.finished_at,
            sources: rec
                .analysed_sources()
                .into_iter()
                .map(|s| s.as_str().to_owned())
                .collect(),
        });

    let batch = latest_series_batch(app, s, window_start).await?;

    let resume_step = resume_step(
        applied_at,
        coverage.as_ref().map(|c| c.requested_at),
        batch.as_ref().map(|b| (b.created_at, b.unfinished)),
    );

    let mut fetch_estimate = Vec::with_capacity(2);
    for scope in [SeriesBatchScope::All, SeriesBatchScope::Incomplete] {
        fetch_estimate.push(estimate_scope(app, s.id, scope).await?);
    }

    Ok(SeriesRefreshStatusResp {
        series_id: sid,
        resume_step,
        series_match: SeriesMatchState {
            links,
            latest_run,
            applied_at,
        },
        coverage,
        coverage_after_series_apply: app
            .cfg()
            .metadata_coverage_after_series_apply
            .as_str()
            .to_owned(),
        batch,
        fetch_estimate,
    })
}

/// Where the flow resumes, from the timestamps of what it left behind
/// (all inside the window):
///
/// - a batch newer than the last apply and the coverage job → `fetch`
///   while children are unfinished, else `review`;
/// - otherwise a coverage job or an applied match → `coverage`;
/// - otherwise `match`.
pub fn resume_step(
    applied_at: Option<DateTime<Utc>>,
    coverage_requested_at: Option<DateTime<Utc>>,
    batch: Option<(DateTime<Utc>, i64)>,
) -> RefreshStep {
    let latest_before_batch = applied_at.max(coverage_requested_at);
    if let Some((created_at, unfinished)) = batch
        && latest_before_batch.is_none_or(|t| created_at >= t)
    {
        return if unfinished > 0 {
            RefreshStep::Fetch
        } else {
            RefreshStep::Review
        };
    }
    if latest_before_batch.is_some() {
        RefreshStep::Coverage
    } else {
        RefreshStep::Match
    }
}

/// The latest `series_issues` batch (series "Fetch metadata" or a grid
/// selection) whose children are this series' issues.
async fn latest_series_batch(
    app: &AppState,
    s: &entity::series::Model,
    window_start: DateTime<Utc>,
) -> Result<Option<RefreshBatch>, sea_orm::DbErr> {
    const SQL: &str = r#"
SELECT b.id, b.created_at, b.items_total,
       (SELECT count(*) FROM metadata_run r
         WHERE r.batch_id = b.id AND r.status NOT IN ('completed', 'failed')) AS unfinished
FROM metadata_batch b
WHERE b.scope = 'series_issues' AND b.library_id = $2 AND b.created_at >= $3
  AND EXISTS (
    SELECT 1 FROM metadata_run r JOIN issues i ON i.id = r.scope_entity_id
    WHERE r.batch_id = b.id AND r.scope = 'issue' AND i.series_id = $1)
ORDER BY b.created_at DESC
LIMIT 1
"#;
    let row = app
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            SQL,
            [
                s.id.into(),
                s.library_id.into(),
                window_start.fixed_offset().into(),
            ],
        ))
        .await?;
    row.map(|r| {
        Ok(RefreshBatch {
            batch_id: r.try_get("", "id")?,
            created_at: r
                .try_get::<DateTime<chrono::FixedOffset>>("", "created_at")?
                .with_timezone(&Utc),
            items_total: r.try_get("", "items_total")?,
            unfinished: r.try_get("", "unfinished")?,
        })
    })
    .transpose()
}

/// Direct-lookup vs search counts for one batch scope, per enabled
/// coverage provider. Mirrors the batch child's routing
/// (`orchestrator::run_issue_search_with`): the target is
/// `range_map::fold_targets` for the issue's canonical number, an annual
/// only uses a range target, and a provider that can't list a series'
/// issues always searches.
async fn estimate_scope(
    app: &AppState,
    series_id: Uuid,
    scope: SeriesBatchScope,
) -> Result<FetchScopeEstimate, sea_orm::DbErr> {
    use entity::{issue, series_provider_range};
    use sea_orm::QuerySelect;

    let selection = series_batch_selection(app, series_id, scope).await?;
    let ids = selection.ids.clone();
    let numbers: Vec<Option<String>> = if ids.is_empty() {
        Vec::new()
    } else {
        issue::Entity::find()
            .select_only()
            .column(issue::Column::NumberRaw)
            .filter(issue::Column::Id.is_in(ids.clone()))
            .into_tuple::<Option<String>>()
            .all(&app.db)
            .await?
    };
    let ranges = series_provider_range::Entity::find()
        .filter(series_provider_range::Column::SeriesId.eq(series_id))
        .all(&app.db)
        .await?;
    let series_ids = entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq("series"))
        .filter(entity::external_id::Column::EntityId.eq(series_id.to_string()))
        .all(&app.db)
        .await?;

    let providers = orchestrator::build_providers(&app.cfg(), app.jobs.redis.clone());
    let enabled: Vec<(Source, bool)> = COVERAGE_SOURCES
        .into_iter()
        .filter_map(|src| {
            providers
                .iter()
                .find(|p| p.id() == src)
                .map(|p| (src, p.lists_series_issues()))
        })
        .collect();

    let mut direct = vec![0i64; enabled.len()];
    for raw in numbers.iter().flatten() {
        if raw.trim().is_empty() {
            continue;
        }
        let canonical = crate::metadata::matcher::canonical_issue_number(raw);
        let annual = crate::metadata::title_norm::strip_annual_prefix(raw).is_some();
        let targets = range_map::fold_targets(&ranges, &series_ids, &canonical);
        for (i, (src, lists)) in enabled.iter().enumerate() {
            if *lists
                && targets
                    .iter()
                    .any(|t| t.source == *src && (!annual || t.via_range))
            {
                direct[i] += 1;
            }
        }
    }
    let total = ids.len() as i64;
    Ok(FetchScopeEstimate {
        scope,
        issues: total,
        eligible: selection.eligible as i64,
        recently_fetched: selection.recently_fetched as i64,
        remainder: selection.remainder() as i64,
        providers: enabled
            .iter()
            .zip(direct)
            .map(|((src, _), d)| ProviderFetchEstimate {
                source: src.as_str().to_owned(),
                direct: d,
                search: total - d,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(min: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(1_800_000_000 + min * 60, 0).unwrap()
    }

    #[test]
    fn nothing_yet_resumes_at_match() {
        assert_eq!(resume_step(None, None, None), RefreshStep::Match);
    }

    #[test]
    fn an_apply_or_a_coverage_job_resumes_at_coverage() {
        assert_eq!(resume_step(Some(t(0)), None, None), RefreshStep::Coverage);
        assert_eq!(resume_step(None, Some(t(0)), None), RefreshStep::Coverage);
        assert_eq!(
            resume_step(Some(t(0)), Some(t(1)), None),
            RefreshStep::Coverage
        );
    }

    #[test]
    fn a_newer_batch_resumes_at_fetch_or_review() {
        assert_eq!(
            resume_step(Some(t(0)), Some(t(1)), Some((t(2), 5))),
            RefreshStep::Fetch
        );
        assert_eq!(
            resume_step(Some(t(0)), Some(t(1)), Some((t(2), 0))),
            RefreshStep::Review
        );
        assert_eq!(
            resume_step(None, None, Some((t(2), 0))),
            RefreshStep::Review
        );
    }

    #[test]
    fn a_batch_older_than_the_latest_match_is_ignored() {
        assert_eq!(
            resume_step(Some(t(5)), None, Some((t(2), 0))),
            RefreshStep::Coverage
        );
        assert_eq!(
            resume_step(None, Some(t(5)), Some((t(2), 0))),
            RefreshStep::Coverage
        );
    }
}
