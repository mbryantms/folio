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
use crate::metadata::apply::{self, ApplyArgs, ApplyError, ApplyMode, ApplyOutcome};
use crate::metadata::writers::CoverOverwritePolicy;
use crate::state::AppState;
use apalis::prelude::*;
use redis::AsyncCommands;
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

async fn release_series_mutex(state: &AppState, series_id: Uuid) {
    let mut conn = state.jobs.redis.clone();
    let _: Result<(), _> = conn.del::<_, ()>(series_mutex_key(series_id)).await;
}
async fn release_issue_mutex(state: &AppState, issue_id: &str) {
    let mut conn = state.jobs.redis.clone();
    let _: Result<(), _> = conn.del::<_, ()>(issue_mutex_key(issue_id)).await;
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
}

pub async fn handle_series(job: ApplySeriesJob, state: Data<AppState>) -> Result<(), Error> {
    let state: AppState = (*state).clone();
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
    } = job;

    let claimed = match try_claim_series_mutex(&state, series_id).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "metadata_apply series: mutex claim failed");
            return Ok(()); // soft-fail; the row stays unapplied + the user can retry
        }
    };
    if !claimed {
        tracing::info!(series_id = %series_id, "metadata_apply series: mutex busy; skipping (caller will retry)");
        return Ok(());
    }

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

    let outcome = apply::apply_series(&state, args).await;
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
    Ok(())
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
}

pub async fn handle_issue(job: ApplyIssueJob, state: Data<AppState>) -> Result<(), Error> {
    let state: AppState = (*state).clone();
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
    } = job;

    let claimed = match try_claim_issue_mutex(&state, &issue_id).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "metadata_apply issue: mutex claim failed");
            return Ok(());
        }
    };
    if !claimed {
        tracing::info!(issue_id, "metadata_apply issue: mutex busy; skipping");
        return Ok(());
    }

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

    let outcome = apply::apply_issue(&state, args).await;
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
