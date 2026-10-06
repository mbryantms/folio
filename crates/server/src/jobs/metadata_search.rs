//! Apalis-backed metadata-search workers (metadata-providers-1.0 M3).
//!
//! Two job types — `SearchSeriesJob` + `SearchIssueJob` — both backed
//! by per-entity Redis coalescing so repeat clicks while a search is
//! already in flight return the existing `run_id` instead of doubling
//! the provider budget. The handler delegates the actual fan-out and
//! ranking to [`crate::metadata::orchestrator`], which is the single
//! audited surface for everything (the future bulk-refresh job will
//! call the same entry points).
//!
//! Worker concurrency is intentionally bounded to 1 per job type —
//! the per-provider Redis token bucket already enforces budget, and
//! the velocity cap on the ComicVine client serializes through a
//! per-instance mutex. Running multiple search workers concurrently
//! gains nothing on the happy path and risks burst-deny on bucket
//! exhaustion.

use crate::metadata::direct_lookup::DirectMode;
use crate::metadata::identifier::Source;
use crate::metadata::matcher::{IssueQueryFacts, SeriesQueryFacts};
use crate::metadata::orchestrator::{self, SearchOpts, StoredQuery};
use crate::metadata::range_map::EffectiveTarget;
use crate::state::AppState;
use apalis::prelude::*;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ───────── coalescing key shapes ─────────

fn series_inflight_key(series_id: Uuid) -> String {
    format!("metadata:search:series:{series_id}")
}

fn issue_inflight_key(issue_id: &str) -> String {
    format!("metadata:search:issue:{issue_id}")
}

/// Set the in-flight marker for a series search and return the run id
/// that wins the coalescing race. If a previous run is still active,
/// the existing run id wins and the caller skips the enqueue.
pub async fn reserve_series_slot(
    state: &AppState,
    series_id: Uuid,
    new_run_id: Uuid,
) -> Result<Uuid, redis::RedisError> {
    let mut conn = state.jobs.redis.clone();
    let key = series_inflight_key(series_id);
    // SET NX EX 600 — the 10-minute TTL is a generous upper bound for
    // a search across two providers (each capped at 30s timeout); if
    // the worker crashes mid-search the key auto-expires so the next
    // click isn't permanently locked out.
    let set: Option<String> = redis::cmd("SET")
        .arg(&key)
        .arg(new_run_id.to_string())
        .arg("NX")
        .arg("EX")
        .arg(600)
        .query_async(&mut conn)
        .await?;
    if set.is_some() {
        return Ok(new_run_id);
    }
    let existing: Option<String> = conn.get(&key).await?;
    let id = existing
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or(new_run_id);
    Ok(id)
}

pub async fn reserve_issue_slot(
    state: &AppState,
    issue_id: &str,
    new_run_id: Uuid,
) -> Result<Uuid, redis::RedisError> {
    let mut conn = state.jobs.redis.clone();
    let key = issue_inflight_key(issue_id);
    let set: Option<String> = redis::cmd("SET")
        .arg(&key)
        .arg(new_run_id.to_string())
        .arg("NX")
        .arg("EX")
        .arg(600)
        .query_async(&mut conn)
        .await?;
    if set.is_some() {
        return Ok(new_run_id);
    }
    let existing: Option<String> = conn.get(&key).await?;
    let id = existing
        .and_then(|s| Uuid::parse_str(&s).ok())
        .unwrap_or(new_run_id);
    Ok(id)
}

async fn release_series_slot(state: &AppState, series_id: Uuid) {
    let mut conn = state.jobs.redis.clone();
    let _: Result<(), _> = conn.del::<_, ()>(series_inflight_key(series_id)).await;
}

async fn release_issue_slot(state: &AppState, issue_id: &str) {
    let mut conn = state.jobs.redis.clone();
    let _: Result<(), _> = conn.del::<_, ()>(issue_inflight_key(issue_id)).await;
}

// ───────── series job ─────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchSeriesJob {
    pub run_id: Uuid,
    pub series_id: Uuid,
    pub library_id: Option<Uuid>,
    pub facts: SeriesQueryFacts,
    /// WP-2.8: the user supplied `facts.year` as a query override. Pins
    /// the hard year gate — no cover-aware relaxation when the user
    /// asserted the year. `#[serde(default)]` so jobs queued before
    /// this field existed still deserialize.
    #[serde(default)]
    pub year_asserted: bool,
}

pub async fn handle_series(job: SearchSeriesJob, state: Data<AppState>) -> Result<(), Error> {
    let state: AppState = (*state).clone();
    // Stashed with the partial results if the run parks on quota, so the
    // resume re-runs this exact job on the owed providers.
    let job_payload = serde_json::to_value(&job).ok();
    let SearchSeriesJob {
        run_id,
        series_id,
        library_id,
        facts,
        year_asserted,
    } = job;
    tracing::info!(
        run_id = %run_id,
        series_id = %series_id,
        name = %facts.name,
        "metadata search: series job start"
    );
    let providers = orchestrator::build_providers(&state.cfg(), state.jobs.redis.clone());
    if providers.is_empty() {
        if let Err(e) = orchestrator::fail_run(&state.db, run_id, "no providers configured").await {
            tracing::error!(error = %e, "metadata search: fail_run write failed");
        }
        release_series_slot(&state, series_id).await;
        return Ok(());
    }
    let thresholds = thresholds(&state);
    let pre_filter = pre_filter_for_library(&state, library_id).await;
    let alt_cap = state.cfg().metadata_alternate_cover_fetch_cap;
    match orchestrator::run_series_search_with(
        &state.db,
        run_id,
        &providers,
        &facts,
        thresholds,
        &pre_filter,
        alt_cap,
        Some(series_id),
        SearchOpts {
            relax_year_gate: !year_asserted,
            job_payload,
            ..SearchOpts::default()
        },
    )
    .await
    {
        Ok(ranked) => {
            tracing::info!(
                run_id = %run_id,
                results = ranked.len(),
                "metadata search: series job complete"
            );
            // M12: auto-apply SingleGoodMatch on non-manual runs when
            // the library has the toggle on. Fires AFTER finalize_run
            // commits the candidates so any failure here doesn't
            // strand the search row.
            maybe_auto_apply_series(&state, run_id, &ranked, library_id, series_id).await;
        }
        Err(e) => {
            tracing::warn!(
                run_id = %run_id,
                error = %e,
                "metadata search: series job failed"
            );
        }
    }
    release_series_slot(&state, series_id).await;
    Ok(())
}

// ───────── issue job ─────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchIssueJob {
    pub run_id: Uuid,
    pub issue_id: String,
    pub library_id: Option<Uuid>,
    pub facts: IssueQueryFacts,
    /// Effective per-provider series target for this issue — a covering
    /// `series_provider_range` mapping folded over the parent series'
    /// `external_ids` default (see [`crate::metadata::range_map`]). Lets
    /// the per-provider issue search narrow to the right volume even
    /// when this issue diverges from the rest of the series.
    pub series_targets: Vec<EffectiveTarget>,
    /// WP-2.8: see [`SearchSeriesJob::year_asserted`].
    #[serde(default)]
    pub year_asserted: bool,
    /// Batch children: let each provider answer from its series' cached
    /// issue list (one cached detail fetch) instead of searching when the
    /// issue's provider series is known ([`crate::metadata::direct_lookup`]).
    /// `false` for the single-issue dialog search, which should show
    /// alternatives.
    #[serde(default)]
    pub direct_lookup: bool,
    /// The match dialog's search: add each provider's coverage-assigned
    /// issue (when its series is known) next to the search results, so
    /// compare mode can default to it ([`DirectMode::Additive`]).
    /// Ignored when `direct_lookup` is set.
    #[serde(default)]
    pub coverage_candidates: bool,
    /// Issue-level refresh: only these providers, direct lookups only — a
    /// miss is recorded, never searched ([`DirectMode::Only`]).
    #[serde(default)]
    pub direct_only: Option<Vec<Source>>,
}

impl SearchIssueJob {
    /// The direct-lookup mode this job runs with (`None` ⇒ search only).
    pub fn direct_mode(&self) -> Option<DirectMode> {
        if self.direct_only.is_some() {
            Some(DirectMode::Only)
        } else if self.direct_lookup {
            Some(DirectMode::Replace)
        } else if self.coverage_candidates {
            Some(DirectMode::Additive)
        } else {
            None
        }
    }
}

pub async fn handle_issue(job: SearchIssueJob, state: Data<AppState>) -> Result<(), Error> {
    let state: AppState = (*state).clone();
    let mode = job.direct_mode();
    let job_payload = serde_json::to_value(&job).ok();
    let SearchIssueJob {
        run_id,
        issue_id,
        library_id,
        facts,
        series_targets,
        year_asserted,
        direct_only,
        ..
    } = job;
    tracing::info!(
        run_id = %run_id,
        issue_id,
        series = %facts.series_name,
        number = %facts.issue_number,
        "metadata search: issue job start"
    );
    let mut providers = orchestrator::build_providers(&state.cfg(), state.jobs.redis.clone());
    if let Some(only) = &direct_only {
        providers.retain(|p| only.contains(&p.id()));
    }
    if providers.is_empty() {
        if let Err(e) = orchestrator::fail_run(&state.db, run_id, "no providers configured").await {
            tracing::error!(error = %e, "metadata search: fail_run write failed");
        }
        release_issue_slot(&state, &issue_id).await;
        return Ok(());
    }
    let thresholds = thresholds(&state);
    let alt_cap = state.cfg().metadata_alternate_cover_fetch_cap;
    let direct = match mode {
        Some(mode) => Some(crate::metadata::direct_lookup::DirectLookupCtx {
            redis: state.jobs.redis.clone(),
            cover_month: issue_cover_month(&state, &issue_id).await,
            mode,
        }),
        None => None,
    };
    match orchestrator::run_issue_search_with(
        &state.db,
        run_id,
        &providers,
        &facts,
        &series_targets,
        thresholds,
        alt_cap,
        Some(issue_id.as_str()),
        SearchOpts {
            relax_year_gate: !year_asserted,
            cover_hasher: None,
            direct,
            job_payload,
        },
    )
    .await
    {
        Ok(ranked) => {
            tracing::info!(
                run_id = %run_id,
                results = ranked.len(),
                "metadata search: issue job complete"
            );
            // M12: auto-apply SingleGoodMatch on non-manual runs when
            // the library has the toggle on.
            maybe_auto_apply_issue(&state, run_id, &ranked, library_id, &issue_id).await;
        }
        Err(e) => {
            tracing::warn!(
                run_id = %run_id,
                error = %e,
                "metadata search: issue job failed"
            );
        }
    }
    release_issue_slot(&state, &issue_id).await;
    Ok(())
}

// ───────── helpers ─────────

/// The issue's cover month (the year rides in the job's facts) — the
/// direct lookup's cover-date check. `None` on any miss: the lookup then
/// compares years only.
async fn issue_cover_month(state: &AppState, issue_id: &str) -> Option<i32> {
    use sea_orm::{EntityTrait, QuerySelect};
    entity::issue::Entity::find_by_id(issue_id.to_owned())
        .select_only()
        .column(entity::issue::Column::Month)
        .into_tuple::<Option<i32>>()
        .one(&state.db)
        .await
        .ok()
        .flatten()
        .flatten()
}

/// Build the per-search [`PreFilter`] from the library row when one
/// is in scope. Cross-library bulk-refresh paths (no `library_id`)
/// get the default empty filter — the operator's blacklist is a
/// per-library policy.
async fn pre_filter_for_library(
    state: &AppState,
    library_id: Option<Uuid>,
) -> crate::metadata::orchestrator::PreFilter {
    use sea_orm::EntityTrait;
    let Some(id) = library_id else {
        return crate::metadata::orchestrator::PreFilter::default();
    };
    match entity::library::Entity::find_by_id(id).one(&state.db).await {
        Ok(Some(lib)) => crate::metadata::orchestrator::PreFilter::from_library(&lib),
        Ok(None) => crate::metadata::orchestrator::PreFilter::default(),
        Err(e) => {
            tracing::warn!(
                error = %e,
                library_id = %id,
                "metadata search: pre-filter library lookup failed; defaulting to empty",
            );
            crate::metadata::orchestrator::PreFilter::default()
        }
    }
}

/// Read the live HIGH + MEDIUM thresholds from the settings overlay.
/// matching-accuracy-1.0 M1: pre-M1 the function returned a hardcoded
/// 95 (which series text scoring can't reach) and the MEDIUM cutoff
/// was hardcoded at 70 inside `Confidence::from_score`. Both now flow
/// from the registry so operators can tune via `/admin/metadata`.
fn thresholds(state: &AppState) -> crate::metadata::matcher::Thresholds {
    let cfg = state.cfg();
    crate::metadata::matcher::Thresholds::new(
        cfg.metadata_auto_apply_threshold as f32,
        cfg.metadata_match_medium_threshold as f32,
    )
}

/// Synchronous variant used by tests that want to drive the
/// orchestrator without an apalis worker booted. Production callers
/// always go through the queue + worker.
#[cfg(test)]
pub async fn run_series_inline(
    state: &AppState,
    run_id: Uuid,
    series_id: Uuid,
    facts: SeriesQueryFacts,
) {
    let providers = orchestrator::build_providers(&state.cfg(), state.jobs.redis.clone());
    let _ = orchestrator::run_series_search(
        &state.db,
        run_id,
        &providers,
        &facts,
        thresholds(state),
        &crate::metadata::orchestrator::PreFilter::default(),
        state.cfg().metadata_alternate_cover_fetch_cap,
        Some(series_id),
    )
    .await;
    release_series_slot(state, series_id).await;
}

#[cfg(test)]
pub async fn run_issue_inline(
    state: &AppState,
    run_id: Uuid,
    issue_id: String,
    facts: IssueQueryFacts,
    series_targets: Vec<EffectiveTarget>,
) {
    let providers = orchestrator::build_providers(&state.cfg(), state.jobs.redis.clone());
    let _ = orchestrator::run_issue_search(
        &state.db,
        run_id,
        &providers,
        &facts,
        &series_targets,
        thresholds(state),
        state.cfg().metadata_alternate_cover_fetch_cap,
        Some(issue_id.as_str()),
    )
    .await;
    release_issue_slot(state, &issue_id).await;
}

/// Build a `StoredQuery` for the run row from the series facts —
/// kept here so the API handler doesn't need to know the `StoredQuery`
/// variant layout.
pub fn series_stored_query(facts: &SeriesQueryFacts) -> StoredQuery {
    StoredQuery::Series(facts.clone())
}

pub fn issue_stored_query(facts: &IssueQueryFacts) -> StoredQuery {
    StoredQuery::Issue(facts.clone())
}

// ───────── bulk-refresh enqueue helper (M7) ─────────

/// Outcome of a single [`enqueue_series_search`] call — surfaces
/// whether the per-entity coalesce gate kicked in so the caller can
/// distinguish a fresh enqueue from a reuse of an in-flight run.
#[derive(Debug, Clone)]
pub struct EnqueueOutcome {
    pub run_id: Uuid,
    /// `true` when the per-entity Redis slot was already held by an
    /// earlier in-flight run; the caller's run row was discarded
    /// and the winner's id is returned.
    pub coalesced: bool,
}

/// Start a metadata-search run + push the apalis job for one series.
/// Reuses the same orchestrator/coalesce/storage path the API
/// handler uses, so bulk-refresh fan-outs and per-series user
/// clicks land on the same per-entity gates and budget controls.
///
/// `triggered_by` is the actor uuid for user-driven flows; pass
/// `None` for cron/system fan-outs (the metadata_run row stores it
/// as nullable). `trigger_kind` should be one of the constants in
/// [`crate::metadata::orchestrator::trigger_kind`].
///
/// metadata-providers-1.0 M7.
pub async fn enqueue_series_search(
    state: &AppState,
    series_id: Uuid,
    triggered_by: Option<Uuid>,
    trigger_kind: &'static str,
    batch_id: Option<Uuid>,
) -> Result<EnqueueOutcome, anyhow::Error> {
    use entity::series;
    use sea_orm::EntityTrait;

    let Some(row) = series::Entity::find_by_id(series_id).one(&state.db).await? else {
        return Err(anyhow::anyhow!("series {series_id} not found"));
    };
    let facts = SeriesQueryFacts {
        name: row.name.clone(),
        year: row.year,
        publisher: row.publisher.clone(),
        volume: row.volume,
        format: row.series_type.clone(),
    };
    let providers = orchestrator::build_providers(&state.cfg(), state.jobs.redis.clone());
    if providers.is_empty() {
        return Err(anyhow::anyhow!("no metadata providers configured"));
    }
    let providers_listed: Vec<_> = providers.iter().map(|p| p.id()).collect();

    let new_run_id = orchestrator::start_run(
        &state.db,
        orchestrator::StartRunArgs {
            scope: orchestrator::scope::SERIES,
            scope_entity_id: Some(row.id.to_string()),
            library_id: Some(row.library_id),
            triggered_by,
            trigger_kind,
            providers: &providers_listed,
            query: series_stored_query(&facts),
            batch_id,
        },
    )
    .await?;

    let winner_run_id = reserve_series_slot(state, row.id, new_run_id).await?;
    if winner_run_id != new_run_id {
        // Existing run already in flight — discard the speculative
        // row + return the winner's id with coalesced=true.
        use sea_orm::EntityTrait;
        let _ = entity::metadata_run::Entity::delete_by_id(new_run_id)
            .exec(&state.db)
            .await;
        return Ok(EnqueueOutcome {
            run_id: winner_run_id,
            coalesced: true,
        });
    }

    let mut storage = state.jobs.metadata_search_series_storage.clone();
    if let Err(e) = storage
        .push(SearchSeriesJob {
            run_id: new_run_id,
            series_id: row.id,
            library_id: Some(row.library_id),
            facts,
            year_asserted: false,
        })
        .await
    {
        let _ = orchestrator::fail_run(&state.db, new_run_id, "queue push failed").await;
        return Err(anyhow::Error::from(e));
    }

    Ok(EnqueueOutcome {
        run_id: new_run_id,
        coalesced: false,
    })
}

/// Issue-scope sibling of [`enqueue_series_search`]. Loads the issue + its
/// parent series, builds the query facts + parent-series external ids, starts
/// a run (optionally under `batch_id`), and pushes a `SearchIssueJob` through
/// the same coalesce gate. Errors (skipped by the batch fan-out) when the
/// issue has no `number_raw` (unsearchable) or no providers are configured.
pub async fn enqueue_issue_search(
    state: &AppState,
    issue_id: &str,
    triggered_by: Option<Uuid>,
    trigger_kind: &'static str,
    batch_id: Option<Uuid>,
) -> Result<EnqueueOutcome, anyhow::Error> {
    enqueue_issue_search_with(state, issue_id, triggered_by, trigger_kind, batch_id, None).await
}

/// [`enqueue_issue_search`] with `direct_only`: the issue-level refresh
/// asks only these providers, by direct lookup only
/// ([`SearchIssueJob::direct_only`]).
pub async fn enqueue_issue_search_with(
    state: &AppState,
    issue_id: &str,
    triggered_by: Option<Uuid>,
    trigger_kind: &'static str,
    batch_id: Option<Uuid>,
    direct_only: Option<Vec<Source>>,
) -> Result<EnqueueOutcome, anyhow::Error> {
    use entity::{issue, series};
    use sea_orm::EntityTrait;

    let Some(i) = issue::Entity::find_by_id(issue_id.to_owned())
        .one(&state.db)
        .await?
    else {
        return Err(anyhow::anyhow!("issue {issue_id} not found"));
    };
    let Some(s) = series::Entity::find_by_id(i.series_id)
        .one(&state.db)
        .await?
    else {
        return Err(anyhow::anyhow!("series {} not found", i.series_id));
    };
    let Some(issue_number) = i.number_raw.clone().filter(|s| !s.trim().is_empty()) else {
        return Err(anyhow::anyhow!("issue {issue_id} has no number_raw"));
    };

    let facts = IssueQueryFacts {
        series_name: s.name.clone(),
        series_year: s.year,
        publisher: s.publisher.clone(),
        volume: s.volume,
        issue_number,
        issue_year: i.year,
        format: crate::metadata::matcher::local_issue_format_hint(
            crate::metadata::matcher::LocalIssueFormat {
                issue_format: i.format.as_deref(),
                special_type: i.special_type.as_deref(),
                series_type: s.series_type.as_deref(),
                manga: i.manga.as_deref(),
                series_name: &s.name,
                issue_number: i.number_raw.as_deref(),
            },
        ),
    };

    // Ids only — building the clients here would cost three TLS client
    // builds per issue of a batch fan-out (see `configured_provider_ids`).
    let providers_listed: Vec<_> = orchestrator::configured_provider_ids(&state.cfg())
        .into_iter()
        .filter(|s| direct_only.as_ref().is_none_or(|only| only.contains(s)))
        .collect();
    if providers_listed.is_empty() {
        return Err(anyhow::anyhow!("no metadata providers configured"));
    }

    // Effective per-provider series target for this issue: a covering
    // `series_provider_range` row folded over the parent series'
    // `external_ids` default. Routes a divergent (split / renumbered)
    // issue to the right provider series at search time.
    let canonical_number = crate::metadata::matcher::canonical_issue_number(&facts.issue_number);
    let series_targets =
        crate::metadata::range_map::resolve_for_issue(&state.db, s.id, &canonical_number)
            .await
            .unwrap_or_default();

    let new_run_id = orchestrator::start_run(
        &state.db,
        orchestrator::StartRunArgs {
            scope: orchestrator::scope::ISSUE,
            scope_entity_id: Some(i.id.clone()),
            library_id: Some(s.library_id),
            triggered_by,
            trigger_kind,
            providers: &providers_listed,
            query: issue_stored_query(&facts),
            batch_id,
        },
    )
    .await?;

    let winner_run_id = reserve_issue_slot(state, &i.id, new_run_id).await?;
    if winner_run_id != new_run_id {
        use sea_orm::EntityTrait;
        let _ = entity::metadata_run::Entity::delete_by_id(new_run_id)
            .exec(&state.db)
            .await;
        return Ok(EnqueueOutcome {
            run_id: winner_run_id,
            coalesced: true,
        });
    }

    let mut storage = state.jobs.metadata_search_issue_storage.clone();
    if let Err(e) = storage
        .push(SearchIssueJob {
            run_id: new_run_id,
            issue_id: i.id.clone(),
            library_id: Some(s.library_id),
            facts,
            series_targets,
            year_asserted: false,
            // Batch children (and their quota resumes) answer covered
            // issues by direct lookup; see `SearchIssueJob::direct_lookup`.
            direct_lookup: batch_id.is_some(),
            coverage_candidates: false,
            direct_only,
        })
        .await
    {
        let _ = orchestrator::fail_run(&state.db, new_run_id, "queue push failed").await;
        return Err(anyhow::Error::from(e));
    }

    Ok(EnqueueOutcome {
        run_id: new_run_id,
        coalesced: false,
    })
}

// ───────── M12: opt-in auto-apply on SingleGoodMatch ─────────

/// Auto-apply the top series candidate when:
/// - The library has `metadata_auto_apply_strong_matches = true`.
/// - The run was non-manual (weekly cron / bulk-fetch / scanner —
///   not a user clicking "Fetch metadata" in the dialog).
/// - The matcher classified the result as
///   [`crate::metadata::match_outcome::MatchOutcomeKind::SingleGood`].
///
/// Soft-fails on every error path (settings lookup, library
/// lookup, queue push). The search row stays valid; the operator
/// can manually apply from the dialog instead. Matching-accuracy-1.0 M12.
async fn maybe_auto_apply_series(
    state: &AppState,
    run_id: Uuid,
    ranked: &[crate::metadata::orchestrator::RankedCandidate],
    library_id: Option<Uuid>,
    series_id: Uuid,
) {
    use crate::metadata::match_outcome::MatchOutcomeKind;
    if MatchOutcomeKind::classify(ranked) != MatchOutcomeKind::SingleGood {
        return;
    }
    let Some(library_id) = library_id else {
        return;
    };
    let (allow_auto, trigger_kind, triggered_by) =
        match auto_apply_eligibility(state, run_id, library_id).await {
            Some(t) => t,
            None => return,
        };
    if !allow_auto {
        return;
    }
    if trigger_kind == crate::metadata::orchestrator::trigger_kind::MANUAL {
        return;
    }
    use apalis::prelude::Storage;
    let mut storage = state.jobs.metadata_apply_series_storage.clone();
    let push_result = storage
        .push(crate::jobs::metadata_apply::ApplySeriesJob {
            run_id,
            ordinal: 0,
            series_id,
            mode: crate::metadata::apply::ApplyMode::FillMissing,
            apply_cover: true,
            cover_overwrite_policy: crate::jobs::metadata_apply::CoverPolicy::WhenMissing,
            override_user_edits: false,
            actor_id: triggered_by,
            actor_ip: None,
            actor_ua: None,
            selected_fields: None,
            override_external_id_sources: Default::default(),
            is_auto: true,
            composite: None,
            bulk: false,
        })
        .await;
    match push_result {
        Ok(_) => tracing::info!(
            run_id = %run_id,
            series_id = %series_id,
            trigger_kind,
            "metadata auto-apply: SingleGoodMatch enqueued (series)",
        ),
        Err(e) => tracing::warn!(
            error = %e,
            run_id = %run_id,
            series_id = %series_id,
            "metadata auto-apply: queue push failed (series)",
        ),
    }
}

/// Issue-scope sibling of [`maybe_auto_apply_series`].
async fn maybe_auto_apply_issue(
    state: &AppState,
    run_id: Uuid,
    ranked: &[crate::metadata::orchestrator::RankedCandidate],
    library_id: Option<Uuid>,
    issue_id: &str,
) {
    use crate::metadata::match_outcome::MatchOutcomeKind;
    if MatchOutcomeKind::classify(ranked) != MatchOutcomeKind::SingleGood {
        return;
    }
    let Some(library_id) = library_id else {
        return;
    };
    let (allow_auto, trigger_kind, triggered_by) =
        match auto_apply_eligibility(state, run_id, library_id).await {
            Some(t) => t,
            None => return,
        };
    if !allow_auto {
        return;
    }
    if trigger_kind == crate::metadata::orchestrator::trigger_kind::MANUAL {
        return;
    }
    use apalis::prelude::Storage;
    let mut storage = state.jobs.metadata_apply_issue_storage.clone();
    let push_result = storage
        .push(crate::jobs::metadata_apply::ApplyIssueJob {
            run_id,
            ordinal: 0,
            issue_id: issue_id.to_owned(),
            mode: crate::metadata::apply::ApplyMode::FillMissing,
            apply_cover: true,
            cover_overwrite_policy: crate::jobs::metadata_apply::CoverPolicy::WhenMissing,
            override_user_edits: false,
            actor_id: triggered_by,
            actor_ip: None,
            actor_ua: None,
            selected_fields: None,
            override_external_id_sources: Default::default(),
            is_auto: true,
            composite: None,
        })
        .await;
    match push_result {
        Ok(_) => tracing::info!(
            run_id = %run_id,
            issue_id,
            trigger_kind,
            "metadata auto-apply: SingleGoodMatch enqueued (issue)",
        ),
        Err(e) => tracing::warn!(
            error = %e,
            run_id = %run_id,
            issue_id,
            "metadata auto-apply: queue push failed (issue)",
        ),
    }
}

/// Loads the library row + the metadata_run row and returns
/// `Some((allow_auto, trigger_kind, triggered_by))` when both are
/// readable. Returns `None` on any DB error so the caller short-
/// circuits — the auto-apply path soft-fails to "leave the search
/// row for manual review".
async fn auto_apply_eligibility(
    state: &AppState,
    run_id: Uuid,
    library_id: Uuid,
) -> Option<(bool, String, Option<Uuid>)> {
    use sea_orm::EntityTrait;
    let library = match entity::library::Entity::find_by_id(library_id)
        .one(&state.db)
        .await
    {
        Ok(Some(l)) => l,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(error = %e, %library_id, "auto-apply eligibility: library lookup failed");
            return None;
        }
    };
    let run = match entity::metadata_run::Entity::find_by_id(run_id)
        .one(&state.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(error = %e, %run_id, "auto-apply eligibility: run lookup failed");
            return None;
        }
    };
    // Provider-complete search: a strong match only auto-applies when
    // every provider the run was started with actually answered. A run
    // with a failed provider stays in Review, flagged, so the operator
    // decides — otherwise one provider's ids would land as if the match
    // were complete.
    let statuses = crate::metadata::provider_status::for_run(&run);
    let complete = crate::metadata::provider_status::all_answered(&statuses);
    if !complete && library.metadata_auto_apply_strong_matches {
        tracing::info!(
            %run_id,
            owed = ?crate::metadata::provider_status::owed_sources(&statuses)
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            "auto-apply: skipped — not every provider answered this run"
        );
    }
    Some((
        library.metadata_auto_apply_strong_matches && complete,
        run.trigger_kind,
        run.triggered_by,
    ))
}

// ───────── quota resume (provider-complete search) ─────────

/// How [`resume_parked_run`] ended.
#[derive(Debug)]
pub enum ResumeOutcome {
    /// The stashed job was re-queued on the parked run itself.
    Resumed,
    /// Already re-queued (no longer parked), or another run holds the
    /// entity's in-flight slot; try next tick.
    InFlight,
    /// Legacy parked row with no stashed job: a fresh run was enqueued
    /// (the old resume path) and the parked row dropped.
    Replaced(EnqueueOutcome),
}

/// Re-queue a run parked `awaiting_quota` on the **same** run id: push the
/// job stashed with its partial results (the exact original query), after
/// re-taking the entity's in-flight slot. The orchestrator then loads the
/// stash and only asks the providers still owed a query.
pub async fn resume_parked_run(
    state: &AppState,
    parked: &entity::metadata_run::Model,
) -> Result<ResumeOutcome, anyhow::Error> {
    use apalis::prelude::Storage;
    use sea_orm::EntityTrait;
    // Re-read the status: a second sweep tick (or a stale model) must not
    // queue the job twice — once re-queued the run is `queued`, not parked.
    let live = entity::metadata_run::Entity::find_by_id(parked.id)
        .one(&state.db)
        .await?;
    if live.as_ref().map(|r| r.status.as_str()) != Some(orchestrator::status::AWAITING_QUOTA) {
        return Ok(ResumeOutcome::InFlight);
    }

    let stashed_job =
        crate::metadata::provider_status::parse_partial(parked.partial_results.as_ref())
            .and_then(|p| p.job);
    let Some(job_json) = stashed_job else {
        // Pre-stash parked row: resume the old way (fresh run, same batch).
        let kind = match parked.trigger_kind.as_str() {
            "weekly_refresh" => orchestrator::trigger_kind::WEEKLY_REFRESH,
            "scanner" => orchestrator::trigger_kind::SCANNER,
            "bulk_action" => orchestrator::trigger_kind::BULK_ACTION,
            _ => orchestrator::trigger_kind::MANUAL,
        };
        let outcome = match parked.scope.as_str() {
            orchestrator::scope::SERIES => {
                let series_id = parked
                    .scope_entity_id
                    .as_deref()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .ok_or_else(|| anyhow::anyhow!("parked series run has no series id"))?;
                enqueue_series_search(state, series_id, parked.triggered_by, kind, parked.batch_id)
                    .await?
            }
            orchestrator::scope::ISSUE => {
                let issue_id = parked
                    .scope_entity_id
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("parked issue run has no issue id"))?;
                enqueue_issue_search(state, &issue_id, parked.triggered_by, kind, parked.batch_id)
                    .await?
            }
            other => return Err(anyhow::anyhow!("parked run has unexpected scope {other}")),
        };
        // Drop the parked row so it doesn't double-count in batch
        // aggregates — the fresh run carries the same batch_id. Guard
        // against the (rare) coalesce-onto-self case.
        if outcome.run_id != parked.id {
            let _ = entity::metadata_run::Entity::delete_by_id(parked.id)
                .exec(&state.db)
                .await;
        }
        return Ok(ResumeOutcome::Replaced(outcome));
    };

    match parked.scope.as_str() {
        orchestrator::scope::SERIES => {
            let job: SearchSeriesJob = serde_json::from_value(job_json)?;
            if reserve_series_slot(state, job.series_id, parked.id).await? != parked.id {
                return Ok(ResumeOutcome::InFlight);
            }
            let mut storage = state.jobs.metadata_search_series_storage.clone();
            let series_id = job.series_id;
            if let Err(e) = storage.push(job).await {
                release_series_slot(state, series_id).await;
                return Err(anyhow::Error::from(e));
            }
        }
        orchestrator::scope::ISSUE => {
            let job: SearchIssueJob = serde_json::from_value(job_json)?;
            if reserve_issue_slot(state, &job.issue_id, parked.id).await? != parked.id {
                return Ok(ResumeOutcome::InFlight);
            }
            let issue_id = job.issue_id.clone();
            let mut storage = state.jobs.metadata_search_issue_storage.clone();
            if let Err(e) = storage.push(job).await {
                release_issue_slot(state, &issue_id).await;
                return Err(anyhow::Error::from(e));
            }
        }
        other => return Err(anyhow::anyhow!("parked run has unexpected scope {other}")),
    }
    orchestrator::mark_resumed(&state.db, parked.id).await?;
    Ok(ResumeOutcome::Resumed)
}
