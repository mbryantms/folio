//! Apalis-backed metadata-apply workers (metadata-providers-1.0 M4).
//!
//! Two job types — `ApplySeriesJob` + `ApplyIssueJob` — both gated
//! by a per-entity Redis mutex so concurrent applies on the same
//! series/issue serialize, never racing. The mutex is short-lived
//! (90s) because the worker holds it across the provider detail
//! fetch + the DB writes; if the worker crashes mid-apply, the key
//! TTLs out so the next click isn't permanently locked.
//!
//! The actual write logic lives in [`crate::metadata::apply`] — these
//! handlers are thin: load AppState, build args, call apply_*,
//! audit, release.
//!
//! Audit: every successful apply emits `admin.{series|issue}.metadata_apply`
//! (or `_force` when `override_user_edits=true`). The payload carries
//! the run_id + ordinal + provider + the full ApplyOutcome so the
//! Runs feed (M6) can render per-item drill-downs without re-fetching.

use crate::audit::{self, AuditEntry};
use crate::jobs::busy_requeue::{self, Requeue};
use crate::library::event_log::{self, Action, Category, NewEvent, Severity};
use crate::metadata::apply::{self, ApplyArgs, ApplyError, ApplyMode, ApplyOutcome};
use crate::metadata::composite::{self, AutoCompositeArgs};
use crate::metadata::identifier::Source;
use crate::metadata::writers::CoverOverwritePolicy;
use crate::state::AppState;
use apalis::prelude::*;
use redis::AsyncCommands;
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MUTEX_TTL_SECS: u64 = 90;

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverPolicy {
    Never,
    WhenMissing,
    Always,
}

impl From<CoverPolicy> for CoverOverwritePolicy {
    fn from(c: CoverPolicy) -> Self {
        match c {
            CoverPolicy::Never => CoverOverwritePolicy::Never,
            CoverPolicy::WhenMissing => CoverOverwritePolicy::WhenMissing,
            CoverPolicy::Always => CoverOverwritePolicy::Always,
        }
    }
}

/// Bulk "Fill missing / Replace all" spec carried on an apply job. When
/// present, the handler runs the multi-provider [`composite::apply_composite_auto`]
/// merge instead of a single-candidate apply — the per-field winners are
/// derived from the merge policy at apply time (no human selection).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositeSpec {
    /// Candidate `ordinal`s to merge. Empty → best-per-provider default.
    pub included: Vec<i32>,
    /// Cover-only source override (bulk prefers ComicVine).
    #[serde(default)]
    pub preferred_cover_provider: Option<Source>,
}

// ───────── coalescing / mutex ─────────

fn series_mutex_key(series_id: Uuid) -> String {
    format!("metadata:apply:series:{series_id}")
}
fn issue_mutex_key(issue_id: &str) -> String {
    format!("metadata:apply:issue:{issue_id}")
}

/// Try to claim the apply mutex for a series. Returns true when we
/// won the race; false when another worker holds it (caller should
/// requeue or skip).
pub async fn try_claim_series_mutex(
    state: &AppState,
    series_id: Uuid,
) -> Result<bool, redis::RedisError> {
    let mut conn = state.jobs.redis.clone();
    let set: Option<String> = redis::cmd("SET")
        .arg(series_mutex_key(series_id))
        .arg(uuid::Uuid::now_v7().to_string())
        .arg("NX")
        .arg("EX")
        .arg(MUTEX_TTL_SECS)
        .query_async(&mut conn)
        .await?;
    Ok(set.is_some())
}

pub async fn try_claim_issue_mutex(
    state: &AppState,
    issue_id: &str,
) -> Result<bool, redis::RedisError> {
    let mut conn = state.jobs.redis.clone();
    let set: Option<String> = redis::cmd("SET")
        .arg(issue_mutex_key(issue_id))
        .arg(uuid::Uuid::now_v7().to_string())
        .arg("NX")
        .arg("EX")
        .arg(MUTEX_TTL_SECS)
        .query_async(&mut conn)
        .await?;
    Ok(set.is_some())
}

/// Release the series apply mutex. `pub` so integration tests can play the
/// other claimant (hold, then release) around a worker call.
pub async fn release_series_mutex(state: &AppState, series_id: Uuid) {
    let mut conn = state.jobs.redis.clone();
    let _: Result<(), _> = conn.del::<_, ()>(series_mutex_key(series_id)).await;
}
pub async fn release_issue_mutex(state: &AppState, issue_id: &str) {
    let mut conn = state.jobs.redis.clone();
    let _: Result<(), _> = conn.del::<_, ()>(issue_mutex_key(issue_id)).await;
}

// ───────── busy-mutex requeue ─────────

/// The identity an apply job needs for its busy-lock bookkeeping.
struct BusyJob {
    run_id: Uuid,
    attempt: u32,
    /// `"series"` | `"issue"` — the event's entity type and the audit kind.
    kind: &'static str,
    target_id: String,
}

/// Found the per-entity apply mutex held: push a copy of the job back with
/// its `attempt` bumped ([`busy_requeue`], 5 s pace, 40 requeues), and once
/// that budget is spent mark the run `failed` with a clear
/// `error_summary`, write a `metadata.errored` library event and tell any
/// waiting client — so the batch review shows the dropped child instead of
/// a silently complete batch.
///
/// The apply mutex is claimed only by apply workers (one per queue), so a
/// busy claim means a second replica applying the same entity, an inline
/// apply, or a key a crashed worker left behind (TTL 90 s); the 200 s
/// budget outlasts all three.
///
/// Provider quota is never re-spent by a requeue: the job carries only
/// `run_id` + `ordinal`, which name the candidate the search already
/// stored on `metadata_run_candidate`; the detail fetch happens inside
/// `apply_*` (cache first), which this path never reaches.
async fn requeue_busy<P, Fut>(state: &AppState, job: BusyJob, push: P) -> Result<(), BoxDynError>
where
    P: FnOnce(u32) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let BusyJob {
        run_id,
        attempt,
        kind,
        target_id,
    } = job;
    let outcome = busy_requeue::requeue_busy("metadata apply", &target_id, attempt, push).await?;
    let Requeue::Exhausted { attempts } = outcome else {
        return Ok(());
    };
    let error =
        format!("metadata apply dropped: {kind} apply lock still held after {attempts} requeues");
    fail_run(state, run_id, &error).await;
    record_busy_dropped(state, kind, &target_id, run_id, attempts, &error).await;
    Ok(())
}

/// Mark the run `failed` with `error` and tell any client waiting on it.
/// Used for a spent busy budget and for an `ApplyError` from the apply
/// itself — pre-fix the latter only emitted the WS event, so the run stayed
/// `completed` with nothing applied and nothing persistent said why.
async fn fail_run(state: &AppState, run_id: Uuid, error: &str) {
    if let Err(e) = crate::metadata::orchestrator::fail_run(&state.db, run_id, error).await {
        tracing::warn!(run_id = %run_id, error = %e, "metadata_apply: could not mark the run failed");
    }
    apply::emit_failed_for_run(state, run_id, error).await;
}

/// `metadata.errored` library event for an apply dropped on a busy lock.
/// Resolves the library from the target row; skipped when it is gone.
async fn record_busy_dropped(
    state: &AppState,
    kind: &'static str,
    target_id: &str,
    run_id: Uuid,
    attempts: u32,
    error: &str,
) {
    let (library_id, label, path) = match kind {
        "series" => {
            let Ok(id) = target_id.parse::<Uuid>() else {
                return;
            };
            match entity::series::Entity::find_by_id(id).one(&state.db).await {
                Ok(Some(s)) => (s.library_id, s.name, s.folder_path),
                _ => return,
            }
        }
        _ => match entity::issue::Entity::find_by_id(target_id.to_owned())
            .one(&state.db)
            .await
        {
            Ok(Some(i)) => (i.library_id, i.slug, Some(i.file_path)),
            _ => return,
        },
    };
    event_log::record(
        &state.db,
        NewEvent::new(
            library_id,
            Category::Metadata,
            Action::Errored,
            Severity::Error,
            format!("Metadata apply dropped for {kind} {label}: lock busy"),
        )
        .entity(kind, target_id.to_owned(), Some(label))
        .detail(serde_json::json!({
            "error": error,
            "attempts": attempts,
            "run_id": run_id,
            "path": path,
        })),
    )
    .await;
}

// ───────── series job ─────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplySeriesJob {
    pub run_id: Uuid,
    pub ordinal: i32,
    pub series_id: Uuid,
    pub mode: ApplyMode,
    pub apply_cover: bool,
    pub cover_overwrite_policy: CoverPolicy,
    pub override_user_edits: bool,
    pub actor_id: Option<Uuid>,
    pub actor_ip: Option<String>,
    pub actor_ua: Option<String>,
    /// M5 per-field opt-in from the preview pane. `#[serde(default)]`
    /// so pre-M5 queued jobs still deserialize.
    #[serde(default)]
    pub selected_fields: Option<std::collections::HashSet<String>>,
    /// M5 per-source override for external_ids conflicts.
    #[serde(default)]
    pub override_external_id_sources: std::collections::HashSet<String>,
    /// Matching-accuracy-1.0 M12: flag the auto-apply path so the
    /// audit step uses the distinct `metadata_auto_apply` action.
    /// `#[serde(default)]` so pre-M12 queued jobs still deserialize.
    #[serde(default)]
    pub is_auto: bool,
    /// Bulk multi-provider merge spec. `Some` → run `apply_composite_auto`
    /// (the "Fill missing / Replace all" path); `None` → single-candidate
    /// apply on `ordinal`. `#[serde(default)]` for queued-job back-compat.
    #[serde(default)]
    pub composite: Option<CompositeSpec>,
    /// Queued by a bulk "Apply" from a batch review (not the user's own
    /// pick in "Match this series…"). With `is_auto`, decides whether the
    /// post-apply coverage analysis runs under
    /// `metadata.coverage_after_series_apply = manual_only`.
    /// `#[serde(default)]` for queued-job back-compat.
    #[serde(default)]
    pub bulk: bool,
    /// Busy-mutex requeue counter ([`crate::jobs::busy_requeue`]). New
    /// enqueues leave it 0; the worker bumps it each time it finds the
    /// series' apply mutex held and pushes itself back. `#[serde(default)]`
    /// so queued pre-upgrade jobs still deserialize.
    #[serde(default)]
    pub attempt: u32,
}

pub async fn handle_series(job: ApplySeriesJob, state: Data<AppState>) -> Result<(), BoxDynError> {
    let state: AppState = (*state).clone();

    // Cancelled before the apply ran (queue cleared / batch cancelled):
    // nothing to write.
    if crate::metadata::orchestrator::run_is_cancelled(&state.db, job.run_id).await {
        tracing::info!(run_id = %job.run_id, "metadata_apply: run cancelled; skipping");
        return Ok(());
    }
    let claimed = match try_claim_series_mutex(&state, job.series_id).await {
        Ok(b) => b,
        Err(e) => {
            // Redis itself failed: hand the task back to apalis for a retry
            // (`JOB_MAX_ATTEMPTS`, then the dead set on Background work)
            // rather than quietly leaving the row unapplied.
            tracing::error!(error = %e, "metadata_apply series: mutex claim failed; returning Err for apalis retry");
            return Err(e.into());
        }
    };
    if !claimed {
        // Another apply holds the series (or a crashed worker's key has
        // not expired). Pre-fix this returned `Ok` with a log line saying
        // "caller will retry" — no caller did, so a batch child whose
        // series was busy was dropped with its run left looking complete
        // (audit N-M7). Requeue with backoff instead.
        let storage = state.jobs.metadata_apply_series_storage.clone();
        return requeue_busy(
            &state,
            BusyJob {
                run_id: job.run_id,
                attempt: job.attempt,
                kind: "series",
                target_id: job.series_id.to_string(),
            },
            |attempt| {
                let mut next = job.clone();
                next.attempt = attempt;
                let mut storage = storage.clone();
                async move {
                    storage
                        .push(next)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                }
            },
        )
        .await;
    }

    let ApplySeriesJob {
        run_id,
        ordinal,
        series_id,
        mode,
        apply_cover,
        cover_overwrite_policy,
        override_user_edits,
        actor_id,
        actor_ip,
        actor_ua,
        selected_fields,
        override_external_id_sources,
        is_auto,
        composite,
        bulk,
        attempt: _,
    } = job;

    let outcome = match composite {
        Some(spec) => {
            composite::apply_composite_auto(
                &state,
                AutoCompositeArgs {
                    run_id,
                    included: spec.included,
                    mode,
                    apply_cover,
                    cover_overwrite_policy: cover_overwrite_policy.into(),
                    override_user_edits,
                    override_external_id_sources,
                    preferred_cover_provider: spec.preferred_cover_provider,
                    actor_id,
                },
            )
            .await
        }
        None => {
            let args = ApplyArgs {
                run_id,
                ordinal,
                mode,
                apply_cover,
                cover_overwrite_policy: cover_overwrite_policy.into(),
                override_user_edits,
                actor_id,
                selected_fields,
                override_external_id_sources,
            };
            apply::apply_series(&state, args).await
        }
    };
    audit_apply(
        &state,
        "series",
        series_id.to_string(),
        run_id,
        ordinal,
        actor_id,
        actor_ip.as_deref(),
        actor_ua.as_deref(),
        override_user_edits,
        is_auto,
        &outcome,
    )
    .await;
    release_series_mutex(&state, series_id).await;

    if let Err(e) = &outcome {
        fail_run(&state, run_id, &e.to_string()).await;
    }
    if outcome.is_ok() {
        // Persist the matched provider series ids onto the series'
        // external_ids. The DB-direct apply writes these inline; the
        // writeback path doesn't (its rescan only ingests series-level
        // ids at series *creation*), so without this the Metron/CV
        // linkage never shows in the External IDs card / header.
        persist_applied_series_external_ids(&state, series_id, run_id).await;

        // After a series match, queue a provider-coverage analysis
        // seeded with the applied provider series (replaces the M7
        // post-manual-apply auto-split detector). The result waits on the
        // series' Details tab for the admin's Accept unless
        // `metadata.coverage_auto_accept` allows the job's high-confidence
        // rule. `metadata.coverage_after_series_apply` picks which applies
        // do this. Best-effort — never fails the apply.
        crate::jobs::provider_coverage::enqueue_after_series_apply(
            &state,
            series_id,
            run_id,
            actor_id,
            !is_auto && !bulk,
        )
        .await;
    }
    Ok(())
}

/// Write the matched provider series ids (from the run's applied
/// candidates) onto the series' `external_ids`, so the series-level
/// linkage is visible immediately regardless of apply path. Idempotent
/// and honours user-precedence via `set_external_id`.
async fn persist_applied_series_external_ids(
    state: &AppState,
    series_id: uuid::Uuid,
    run_id: uuid::Uuid,
) {
    use crate::metadata::identifier::Identifier;
    use crate::metadata::writers::{SetBy, set_external_id_promoting};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let applied = entity::metadata_run_candidate::Entity::find()
        .filter(entity::metadata_run_candidate::Column::RunId.eq(run_id))
        .filter(entity::metadata_run_candidate::Column::AppliedAt.is_not_null())
        .all(&state.db)
        .await
        .unwrap_or_default();
    let series_id_str = series_id.to_string();
    let mut promoted_pairs = 0;
    for c in applied {
        let Some(source) = crate::metadata::apply::parse_source(&c.source) else {
            continue;
        };
        let identifier = Identifier::with_canonical_url(source, c.external_id.clone(), "series");
        match set_external_id_promoting(
            &state.db,
            "series",
            &series_id_str,
            &identifier,
            SetBy::Provider(source),
        )
        .await
        {
            Ok((_, n)) => promoted_pairs += n,
            Err(e) => {
                tracing::warn!(
                    series_id = %series_id,
                    source = source.as_str(),
                    error = %e,
                    "failed to persist applied series external id"
                );
            }
        }
    }
    // WP-8.2: the id just written can promote another series' external
    // link to a series relationship (`promote_for_provider_id`). This runs
    // after the apply's own cache invalidation, so drop the similar-series
    // cache again when it did.
    if promoted_pairs > 0 {
        state.similarity.invalidate_all();
    }
}

// ───────── issue job ─────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyIssueJob {
    pub run_id: Uuid,
    pub ordinal: i32,
    pub issue_id: String,
    pub mode: ApplyMode,
    pub apply_cover: bool,
    pub cover_overwrite_policy: CoverPolicy,
    pub override_user_edits: bool,
    pub actor_id: Option<Uuid>,
    pub actor_ip: Option<String>,
    pub actor_ua: Option<String>,
    /// M5 per-field opt-in from the preview pane. `#[serde(default)]`
    /// so pre-M5 queued jobs still deserialize.
    #[serde(default)]
    pub selected_fields: Option<std::collections::HashSet<String>>,
    /// M5 per-source override for external_ids conflicts.
    #[serde(default)]
    pub override_external_id_sources: std::collections::HashSet<String>,
    /// Matching-accuracy-1.0 M12: flag the auto-apply path so the
    /// audit step uses the distinct `metadata_auto_apply` action.
    #[serde(default)]
    pub is_auto: bool,
    /// Bulk multi-provider merge spec. `Some` → run `apply_composite_auto`;
    /// `None` → single-candidate apply on `ordinal`.
    #[serde(default)]
    pub composite: Option<CompositeSpec>,
    /// Busy-mutex requeue counter ([`crate::jobs::busy_requeue`]); see
    /// [`ApplySeriesJob::attempt`].
    #[serde(default)]
    pub attempt: u32,
}

pub async fn handle_issue(job: ApplyIssueJob, state: Data<AppState>) -> Result<(), BoxDynError> {
    let state: AppState = (*state).clone();

    // Cancelled before the apply ran (queue cleared / batch cancelled):
    // nothing to write.
    if crate::metadata::orchestrator::run_is_cancelled(&state.db, job.run_id).await {
        tracing::info!(run_id = %job.run_id, "metadata_apply: run cancelled; skipping");
        return Ok(());
    }
    let claimed = match try_claim_issue_mutex(&state, &job.issue_id).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "metadata_apply issue: mutex claim failed; returning Err for apalis retry");
            return Err(e.into());
        }
    };
    if !claimed {
        // Same requeue-with-backoff as the series job; see there.
        let storage = state.jobs.metadata_apply_issue_storage.clone();
        return requeue_busy(
            &state,
            BusyJob {
                run_id: job.run_id,
                attempt: job.attempt,
                kind: "issue",
                target_id: job.issue_id.clone(),
            },
            |attempt| {
                let mut next = job.clone();
                next.attempt = attempt;
                let mut storage = storage.clone();
                async move {
                    storage
                        .push(next)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                }
            },
        )
        .await;
    }

    let ApplyIssueJob {
        run_id,
        ordinal,
        issue_id,
        mode,
        apply_cover,
        cover_overwrite_policy,
        override_user_edits,
        actor_id,
        actor_ip,
        actor_ua,
        selected_fields,
        override_external_id_sources,
        is_auto,
        composite,
        attempt: _,
    } = job;

    let outcome = match composite {
        Some(spec) => {
            composite::apply_composite_auto(
                &state,
                AutoCompositeArgs {
                    run_id,
                    included: spec.included,
                    mode,
                    apply_cover,
                    cover_overwrite_policy: cover_overwrite_policy.into(),
                    override_user_edits,
                    override_external_id_sources,
                    preferred_cover_provider: spec.preferred_cover_provider,
                    actor_id,
                },
            )
            .await
        }
        None => {
            let args = ApplyArgs {
                run_id,
                ordinal,
                mode,
                apply_cover,
                cover_overwrite_policy: cover_overwrite_policy.into(),
                override_user_edits,
                actor_id,
                selected_fields,
                override_external_id_sources,
            };
            apply::apply_issue(&state, args).await
        }
    };
    audit_apply(
        &state,
        "issue",
        issue_id.clone(),
        run_id,
        ordinal,
        actor_id,
        actor_ip.as_deref(),
        actor_ua.as_deref(),
        override_user_edits,
        is_auto,
        &outcome,
    )
    .await;
    release_issue_mutex(&state, &issue_id).await;
    if let Err(e) = &outcome {
        fail_run(&state, run_id, &e.to_string()).await;
    }
    Ok(())
}

// ───────── audit ─────────

#[allow(clippy::too_many_arguments)]
pub(crate) async fn audit_apply(
    state: &AppState,
    kind: &str,
    target_id: String,
    run_id: Uuid,
    ordinal: i32,
    actor_id: Option<Uuid>,
    actor_ip: Option<&str>,
    actor_ua: Option<&str>,
    override_user_edits: bool,
    is_auto: bool,
    outcome: &Result<ApplyOutcome, ApplyError>,
) {
    // Library stream (observability-split M3b): a durable, operational
    // manifest row for the apply — independent of the audit trail below
    // (which is the server-domain who-did-what record and is skipped for
    // anonymous cron runs). Fires for every apply, actor or not.
    record_apply_manifest(state, kind, &target_id, outcome).await;

    // matching-accuracy-1.0 M12: `metadata_auto_apply` is the distinct
    // action emitted by the cron / bulk-fetch auto-apply path. Manual
    // applies stay on `metadata_apply` / `metadata_apply_force` so
    // operators can grep + filter cleanly.
    let action_owned = match (is_auto, override_user_edits) {
        (true, _) => format!("admin.{kind}.metadata_auto_apply"),
        (false, true) => format!("admin.{kind}.metadata_apply_force"),
        (false, false) => format!("admin.{kind}.metadata_apply"),
    };
    let payload = match outcome {
        Ok(o) => serde_json::json!({
            "run_id": run_id,
            "ordinal": ordinal,
            "outcome": o,
            "is_auto": is_auto,
        }),
        Err(e) => serde_json::json!({
            "run_id": run_id,
            "ordinal": ordinal,
            "error": e.to_string(),
            "is_auto": is_auto,
        }),
    };
    let Some(actor_id) = actor_id else {
        // Anonymous run — weekly cron has no triggered_by. `audit::record`
        // requires an actor UUID, so we structured-log instead. The
        // `event=metadata_auto_apply` field is the grep handle operators
        // use to find these.
        tracing::info!(
            event = "metadata_auto_apply",
            run_id = %run_id,
            ordinal,
            kind,
            target_id,
            is_auto,
            ?payload,
            "metadata_apply: anonymous run; structured-log only"
        );
        return;
    };
    // SAFETY: leak the per-job string so the &'static expected by
    // AuditEntry is satisfied. Audit action names are a small
    // bounded set ({series|issue} × {apply | apply_force | auto_apply})
    // so the leak is bounded at 6 strings per process lifetime.
    let action: &'static str = Box::leak(action_owned.into_boxed_str());
    let target_type: &'static str = match kind {
        "series" => "series",
        "issue" => "issue",
        _ => "unknown",
    };
    audit::record(
        &state.db,
        AuditEntry {
            actor_id,
            action,
            target_type: Some(target_type),
            target_id: Some(target_id),
            payload,
            ip: actor_ip.map(str::to_owned),
            user_agent: actor_ua.map(str::to_owned),
        },
    )
    .await;
}

/// Emit a `metadata` library-event for a series/issue apply. Resolves the
/// `library_id` (+ a label) from the target row — apply jobs run outside any
/// scan, so there's no `scan_run_id` to inherit. Best-effort: if the target
/// row can't be resolved the event is skipped (the apply itself already
/// happened).
async fn record_apply_manifest(
    state: &AppState,
    kind: &str,
    target_id: &str,
    outcome: &Result<ApplyOutcome, ApplyError>,
) {
    // Resolve library_id, a label, and the "where to look" context
    // (`path` + `series`) so the Library-activity row is actionable — same
    // enrichment the thumbnail-failure events carry.
    let (library_id, entity_type, label, path, series_name) = match kind {
        "series" => {
            let Ok(id) = target_id.parse::<Uuid>() else {
                return;
            };
            match entity::series::Entity::find_by_id(id).one(&state.db).await {
                Ok(Some(s)) => (
                    s.library_id,
                    "series",
                    s.name.clone(),
                    s.folder_path.clone(),
                    Some(s.name),
                ),
                _ => return,
            }
        }
        "issue" => match entity::issue::Entity::find_by_id(target_id.to_owned())
            .one(&state.db)
            .await
        {
            Ok(Some(i)) => {
                let series_name = entity::series::Entity::find_by_id(i.series_id)
                    .one(&state.db)
                    .await
                    .ok()
                    .flatten()
                    .map(|s| s.name);
                (
                    i.library_id,
                    "issue",
                    i.slug,
                    Some(i.file_path),
                    series_name,
                )
            }
            _ => return,
        },
        _ => return,
    };
    let (action, severity, summary) = match outcome {
        Ok(_) => (
            Action::Applied,
            Severity::Info,
            format!("Metadata applied to {kind} {label}"),
        ),
        Err(_) => (
            Action::Errored,
            Severity::Warning,
            format!("Metadata apply failed for {kind} {label}"),
        ),
    };
    let mut detail = match outcome {
        Ok(o) => serde_json::json!({ "outcome": o }),
        Err(e) => serde_json::json!({ "error": e.to_string() }),
    };
    if let Some(obj) = detail.as_object_mut() {
        if let Some(p) = path {
            obj.insert("path".into(), serde_json::json!(p));
        }
        if let Some(s) = series_name {
            obj.insert("series".into(), serde_json::json!(s));
        }
    }
    event_log::record(
        &state.db,
        NewEvent::new(library_id, Category::Metadata, action, severity, summary)
            .entity(entity_type, target_id.to_owned(), Some(label))
            .detail(detail),
    )
    .await;
}

/// Synchronous entrypoint that bypasses the apalis worker — drives
/// the same per-entity mutex + apply logic + release as `handle_*`,
/// but returns the outcome directly. Used by integration tests, and
/// available to future callers (e.g. a CLI driver or a sync bulk
/// import path) that don't want to round-trip through Redis.
pub async fn apply_series_inline(
    state: &AppState,
    series_id: Uuid,
    args: ApplyArgs,
) -> Result<ApplyOutcome, ApplyError> {
    let _ = try_claim_series_mutex(state, series_id).await;
    let result = apply::apply_series(state, args).await;
    release_series_mutex(state, series_id).await;
    result
}

pub async fn apply_issue_inline(
    state: &AppState,
    issue_id: &str,
    args: ApplyArgs,
) -> Result<ApplyOutcome, ApplyError> {
    let _ = try_claim_issue_mutex(state, issue_id).await;
    let result = apply::apply_issue(state, args).await;
    release_issue_mutex(state, issue_id).await;
    result
}
