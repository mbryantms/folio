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

use crate::audit::{self, AuditEntry};
use crate::metadata::coverage::{self, AcceptOutcome, ProviderAnalysis, SeriesFacts};
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
    let redis = &state.jobs.redis;
    if let Some(rec) = latest_for_series(redis, series_id).await
        && rec.is_active()
    {
        return Ok((rec, false));
    }
    let rec = JobRecord {
        job_id: Uuid::new_v4(),
        series_id,
        actor_id,
        state: CoverageJobState::Queued,
        auto_accept,
        requested_at: Utc::now(),
        started_at: None,
        finished_at: None,
        error: None,
        providers: Vec::new(),
        auto_accepted: Vec::new(),
    };
    save(redis, &rec).await?;
    let mut storage = state.jobs.provider_coverage_storage.clone();
    storage
        .push(ProviderCoverageJob { job_id: rec.job_id })
        .await
        .map_err(|e| anyhow::anyhow!("enqueue provider coverage: {e}"))?;
    Ok((rec, true))
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
    let facts = SeriesFacts::load(state, &series_row).await?;
    let started = std::time::Instant::now();
    rec.providers = coverage::analyze(state, &facts).await;
    tracing::info!(
        series_id = %rec.series_id,
        elapsed_ms = started.elapsed().as_millis() as u64,
        requests = ?rec.providers.iter().map(|p| (p.source.as_str(), p.requests)).collect::<Vec<_>>(),
        "provider coverage: analysis finished"
    );

    if rec.auto_accept {
        let mut ranges_created = 0usize;
        for analysis in &rec.providers {
            let view =
                coverage::build_view(analysis, &facts.local, &facts.ext_ids, &facts.ranges, None);
            if !view.auto_acceptable {
                continue;
            }
            let outcome =
                coverage::accept_provider(state, rec.series_id, analysis, None, false).await?;
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
