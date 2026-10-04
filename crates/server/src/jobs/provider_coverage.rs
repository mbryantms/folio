//! `ProviderCoverageJob` — runs a provider-independent coverage analysis
//! ([`crate::metadata::coverage`]) for one series in the background.
//!
//! Three providers × up to eight listed candidates × ComicVine's 1.1 s
//! request floor don't fit the 60 s JSON route timeout, so
//! `POST /api/series/{slug}/provider-coverage/analyze` records a job and
//! returns its id; the web app polls
//! `GET /api/series/{slug}/provider-coverage/analysis`.
//!
//! **State** lives in Redis, not a table: `coverage:job:<id>` holds the
//! [`JobRecord`] (state, timestamps, the per-provider candidates with their
//! issue lists) for [`RECORD_TTL_SECS`], and `coverage:series:<series_id>`
//! points at the latest job. The record is everything "Accept" / "Choose
//! series" need to recompute a proposal without a provider request; the
//! public view is rebuilt from it plus the current DB rows on every read,
//! so it never shows a stale "new" range after an accept.
//!
//! **Dedupe.** A series with a queued / running job (younger than
//! [`STALE_AFTER_SECS`]) gets that job back instead of a second one.
//!
//! **Auto-accept.** When the request asked for it, providers whose proposal
//! is high-confidence, changes something and conflicts with no user-set
//! data are accepted with `SetBy::Provider` and audited
//! (`admin.series.provider_coverage_accept`, `auto: true`).
//!
//! **After a series apply** ([`enqueue_after_series_apply`]). A successful
//! series metadata apply queues an analysis *seeded* with the provider
//! series it applied ([`CoverageSeed`]): only those providers are
//! analysed, each seed is its provider's main unless the user linked
//! another series, and a seeded provider skips its name searches when its
//! known ids already cover every local issue. Which applies do this is
//! `metadata.coverage_after_series_apply` (`off` | `manual_only` | `all`);
//! auto-accept additionally needs `metadata.coverage_auto_accept`.
//! Bulk / automatic applies are deduped per series (an active job absorbs
//! the seeds) and skipped while [`MAX_QUEUED_AFTER_APPLY`] jobs wait.
//! This replaced the post-manual-apply auto-split detector.

use crate::audit::{self, AuditEntry};
use crate::metadata::coverage::{
    self, AcceptOutcome, COVERAGE_SOURCES, CoverageSeed, ProviderAnalysis, SeriesFacts,
};
use crate::metadata::identifier::Source;
use crate::state::AppState;
use apalis::prelude::*;
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderCoverageJob {
    pub job_id: Uuid,
}

/// How long a job record (and the series → job pointer) is kept.
pub const RECORD_TTL_SECS: u64 = 24 * 3600;

/// A queued / running record older than this is treated as abandoned (a
/// crashed worker) and no longer dedupes new requests.
pub const STALE_AFTER_SECS: i64 = 15 * 60;

/// Bulk / automatic series applies don't queue another analysis while this
/// many coverage jobs are already waiting (a manual apply always does).
pub const MAX_QUEUED_AFTER_APPLY: usize = 50;

/// What queued an analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoverageTrigger {
    /// "Analyze coverage" on the series' Details tab.
    #[default]
    Analyze,
    /// A series match the user applied from "Match this series…".
    SeriesMatch,
    /// A bulk or automatic series apply (setting `all`).
    BulkSeriesMatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoverageJobState {
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub job_id: Uuid,
    pub series_id: Uuid,
    pub actor_id: Uuid,
    pub state: CoverageJobState,
    pub auto_accept: bool,
    pub requested_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    #[serde(default)]
    pub providers: Vec<ProviderAnalysis>,
    #[serde(default)]
    pub auto_accepted: Vec<AcceptOutcome>,
    #[serde(default)]
    pub trigger: CoverageTrigger,
    /// Provider series a series apply chose (post-apply jobs only). Only
    /// these providers are analysed; each seed is its provider's main.
    #[serde(default)]
    pub seeds: Vec<CoverageSeed>,
    /// Providers to analyse; empty = all of [`COVERAGE_SOURCES`].
    #[serde(default)]
    pub sources: Vec<Source>,
}

impl JobRecord {
    /// The seeded main for `source`, if this job ran after an apply.
    pub fn seed_main(&self, source: Source) -> Option<&str> {
        coverage::seed_for(&self.seeds, source).map(|s| s.provider_series_id.as_str())
    }

    /// Providers this job analyses, in display order.
    pub fn analysed_sources(&self) -> Vec<Source> {
        COVERAGE_SOURCES
            .into_iter()
            .filter(|s| self.sources.is_empty() || self.sources.contains(s))
            .collect()
    }
}

impl JobRecord {
    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            CoverageJobState::Queued | CoverageJobState::Running
        ) && (Utc::now() - self.requested_at).num_seconds() < STALE_AFTER_SECS
    }
}

fn record_key(job_id: Uuid) -> String {
    format!("coverage:job:{job_id}")
}

fn series_key(series_id: Uuid) -> String {
    format!("coverage:series:{series_id}")
}

pub async fn load(redis: &ConnectionManager, job_id: Uuid) -> Option<JobRecord> {
    let mut conn = redis.clone();
    let raw: Option<String> = conn.get(record_key(job_id)).await.ok().flatten();
    raw.and_then(|s| serde_json::from_str(&s).ok())
}

/// The latest job record for a series, if one is still kept.
pub async fn latest_for_series(redis: &ConnectionManager, series_id: Uuid) -> Option<JobRecord> {
    let mut conn = redis.clone();
    let id: Option<String> = conn.get(series_key(series_id)).await.ok().flatten();
    let id = Uuid::parse_str(&id?).ok()?;
    load(redis, id).await
}

pub async fn save(redis: &ConnectionManager, rec: &JobRecord) -> redis::RedisResult<()> {
    let raw = serde_json::to_string(rec).map_err(|e| {
        redis::RedisError::from((
            redis::ErrorKind::TypeError,
            "serialize coverage record",
            e.to_string(),
        ))
    })?;
    let mut conn = redis.clone();
    let _: () = conn
        .set_ex(record_key(rec.job_id), raw, RECORD_TTL_SECS)
        .await?;
    let _: () = conn
        .set_ex(
            series_key(rec.series_id),
            rec.job_id.to_string(),
            RECORD_TTL_SECS,
        )
        .await?;
    Ok(())
}

/// Record + push an analysis for `series_id`, or return the series' job
/// already in flight. The bool is `true` when a new job was queued.
pub async fn enqueue(
    state: &AppState,
    series_id: Uuid,
    actor_id: Uuid,
    auto_accept: bool,
) -> anyhow::Result<(JobRecord, bool)> {
    enqueue_request(
        state,
        EnqueueRequest {
            series_id,
            actor_id,
            auto_accept,
            trigger: CoverageTrigger::Analyze,
            seeds: Vec::new(),
        },
    )
    .await
}

/// What [`enqueue_request`] queues.
pub struct EnqueueRequest {
    pub series_id: Uuid,
    pub actor_id: Uuid,
    pub auto_accept: bool,
    pub trigger: CoverageTrigger,
    pub seeds: Vec<CoverageSeed>,
}

/// [`enqueue`] with a trigger and seeds. A series with an active job gets
/// that job back; when it hasn't started yet, the new seeds are merged
/// into it (a seed replaces an older one for the same provider), so a
/// batch never queues a series twice.
pub async fn enqueue_request(
    state: &AppState,
    req: EnqueueRequest,
) -> anyhow::Result<(JobRecord, bool)> {
    let redis = &state.jobs.redis;
    if let Some(mut rec) = latest_for_series(redis, req.series_id).await
        && rec.is_active()
    {
        if rec.state == CoverageJobState::Queued && !req.seeds.is_empty() {
            // An all-provider job stays all-provider; a seeded one widens
            // to the new seeds' providers.
            for seed in req.seeds {
                if !rec.sources.is_empty() && !rec.sources.contains(&seed.source) {
                    rec.sources.push(seed.source);
                }
                rec.seeds.retain(|s| s.source != seed.source);
                rec.seeds.push(seed);
            }
            save(redis, &rec).await?;
        }
        return Ok((rec, false));
    }
    let rec = JobRecord {
        job_id: Uuid::new_v4(),
        series_id: req.series_id,
        actor_id: req.actor_id,
        state: CoverageJobState::Queued,
        auto_accept: req.auto_accept,
        requested_at: Utc::now(),
        started_at: None,
        finished_at: None,
        error: None,
        providers: Vec::new(),
        auto_accepted: Vec::new(),
        trigger: req.trigger,
        sources: req.seeds.iter().map(|s| s.source).collect(),
        seeds: req.seeds,
    };
    save(redis, &rec).await?;
    let mut storage = state.jobs.provider_coverage_storage.clone();
    storage
        .push(ProviderCoverageJob { job_id: rec.job_id })
        .await
        .map_err(|e| anyhow::anyhow!("enqueue provider coverage: {e}"))?;
    Ok((rec, true))
}

/// The provider series a series metadata run applied (its candidates with
/// `applied_at`, newest per provider), as coverage seeds. Covers single
/// and composite applies.
pub async fn seeds_from_run(state: &AppState, run_id: Uuid) -> Vec<CoverageSeed> {
    use sea_orm::{ColumnTrait, QueryFilter, QueryOrder};
    let applied = entity::metadata_run_candidate::Entity::find()
        .filter(entity::metadata_run_candidate::Column::RunId.eq(run_id))
        .filter(entity::metadata_run_candidate::Column::AppliedAt.is_not_null())
        .order_by_desc(entity::metadata_run_candidate::Column::AppliedAt)
        .all(&state.db)
        .await
        .unwrap_or_default();
    let mut seeds: Vec<CoverageSeed> = Vec::new();
    for c in applied {
        let Some(source) = crate::metadata::apply::parse_source(&c.source) else {
            continue;
        };
        if !COVERAGE_SOURCES.contains(&source) || seeds.iter().any(|s| s.source == source) {
            continue;
        }
        seeds.push(CoverageSeed {
            source,
            provider_series_id: c.external_id.clone(),
            candidate: serde_json::from_value(c.candidate.clone()).ok(),
        });
    }
    seeds
}

/// After a successful series apply: queue a coverage analysis seeded with
/// the run's applied provider series, when
/// `metadata.coverage_after_series_apply` covers this kind of apply
/// (`manual` = the user's apply from "Match this series…"). Best-effort:
/// never fails the apply. Returns the job when one was queued or reused.
pub async fn enqueue_after_series_apply(
    state: &AppState,
    series_id: Uuid,
    run_id: Uuid,
    actor_id: Option<Uuid>,
    manual: bool,
) -> Option<(JobRecord, bool)> {
    let cfg = state.cfg();
    if !cfg.metadata_coverage_after_series_apply.runs_after(manual) {
        return None;
    }
    let seeds = seeds_from_run(state, run_id).await;
    if seeds.is_empty() {
        return None;
    }
    if !manual {
        let mut storage = state.jobs.provider_coverage_storage.clone();
        let waiting = storage.len().await.unwrap_or(0).max(0) as usize;
        if waiting >= MAX_QUEUED_AFTER_APPLY {
            tracing::info!(
                series_id = %series_id,
                waiting,
                "provider coverage: queue full; skipped the post-apply analysis"
            );
            return None;
        }
    }
    // Auto-accept is audited under the actor; an actor-less (scheduled)
    // apply only ever presents its result.
    let auto_accept = cfg.metadata_coverage_auto_accept && actor_id.is_some();
    let req = EnqueueRequest {
        series_id,
        actor_id: actor_id.unwrap_or(Uuid::nil()),
        auto_accept,
        trigger: if manual {
            CoverageTrigger::SeriesMatch
        } else {
            CoverageTrigger::BulkSeriesMatch
        },
        seeds,
    };
    match enqueue_request(state, req).await {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::warn!(series_id = %series_id, error = %e, "provider coverage: post-apply enqueue failed");
            None
        }
    }
}

pub async fn handle(job: ProviderCoverageJob, state: Data<AppState>) -> Result<(), Error> {
    let state: AppState = (*state).clone();
    if let Err(e) = process(&state, job.job_id).await {
        // Recorded on the job record; not retried (a retry would spend the
        // provider budget again for the same failure).
        tracing::error!(job_id = %job.job_id, error = %e, "provider coverage: job failed");
    }
    Ok(())
}

/// Run the analysis for a recorded job. Public so tests drive it without a
/// worker.
pub async fn process(state: &AppState, job_id: Uuid) -> anyhow::Result<()> {
    let redis = &state.jobs.redis;
    let Some(mut rec) = load(redis, job_id).await else {
        return Ok(());
    };
    if rec.state != CoverageJobState::Queued {
        return Ok(());
    }
    rec.state = CoverageJobState::Running;
    rec.started_at = Some(Utc::now());
    save(redis, &rec).await?;

    let result = run(state, &mut rec).await;
    rec.finished_at = Some(Utc::now());
    match &result {
        Ok(()) => rec.state = CoverageJobState::Done,
        Err(e) => {
            rec.state = CoverageJobState::Failed;
            rec.error = Some(e.to_string());
        }
    }
    save(redis, &rec).await?;
    result
}

async fn run(state: &AppState, rec: &mut JobRecord) -> anyhow::Result<()> {
    let Some(series_row) = entity::series::Entity::find_by_id(rec.series_id)
        .one(&state.db)
        .await?
    else {
        anyhow::bail!("series no longer exists");
    };
    let mut facts = SeriesFacts::load(state, &series_row).await?;
    facts.seeds = rec.seeds.clone();
    let started = std::time::Instant::now();
    rec.providers = coverage::analyze_sources(state, &facts, &rec.analysed_sources()).await;
    tracing::info!(
        series_id = %rec.series_id,
        elapsed_ms = started.elapsed().as_millis() as u64,
        requests = ?rec.providers.iter().map(|p| (p.source.as_str(), p.requests)).collect::<Vec<_>>(),
        "provider coverage: analysis finished"
    );

    if rec.auto_accept {
        let mut ranges_created = 0usize;
        for analysis in &rec.providers {
            let seed = rec.seed_main(analysis.source);
            let view = coverage::build_view(
                analysis,
                &facts.local,
                &facts.ext_ids,
                &facts.ranges,
                None,
                seed,
            );
            if !view.auto_acceptable {
                continue;
            }
            let outcome =
                coverage::accept_provider(state, rec.series_id, analysis, None, seed, false)
                    .await?;
            ranges_created += outcome.ranges_created.len();
            audit::record(
                &state.db,
                AuditEntry {
                    actor_id: rec.actor_id,
                    action: "admin.series.provider_coverage_accept",
                    target_type: Some("series"),
                    target_id: Some(rec.series_id.to_string()),
                    payload: serde_json::json!({
                        "auto": true,
                        "job_id": rec.job_id,
                        "trigger": rec.trigger,
                        "source": outcome.source,
                        "main_series_id": outcome.main_series_id,
                        "main_written": outcome.main_written,
                        "ranges_created": outcome.ranges_created.iter()
                            .map(|r| serde_json::json!({
                                "provider_series_id": r.provider_series_id,
                                "low": r.low,
                                "high": r.high,
                            }))
                            .collect::<Vec<_>>(),
                    }),
                    ip: None,
                    user_agent: None,
                },
            )
            .await;
            rec.auto_accepted.push(outcome);
        }
        if ranges_created > 0 {
            crate::jobs::relationship_suggest::enqueue(state, series_row.library_id).await;
        }
    }
    Ok(())
}
