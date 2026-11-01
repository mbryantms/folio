//! `RewriteIssueSidecarsJob` — apalis worker that swaps an issue's
//! `ComicInfo.xml` + `MetronInfo.xml` entries inside the archive and
//! re-ingests the result via a scoped rescan.
//!
//! Wired by the M3 refactor of `apply_issue` in
//! [`crate::metadata::apply`] — when a library has
//! `metadata_writeback_enabled=true`, the apply path composes both
//! XMLs via [`crate::metadata::sidecar_compose`], serializes them with
//! [`parsers::comicinfo::serialize`] / [`parsers::metroninfo::serialize`],
//! and enqueues this job. The previous DB-direct write path stays for
//! libraries that haven't flipped the toggle.
//!
//! ## Flow (mirrors plan M3 step list)
//!
//!   1. Try-claim per-issue rewrite mutex
//!      (`archive:rewrite:<issue_id>`, TTL = 120s).
//!   2. Open the source archive via [`archive::open`].
//!   3. Build a [`archive::cbz_write::RebuildPlan`] with
//!      `set_entry("ComicInfo.xml", …)` + `set_entry("MetronInfo.xml", …)`.
//!      Every page entry takes the default `Keep` path → stream-copied
//!      compressed bytes preserved verbatim.
//!   4. Atomic swap via
//!      [`crate::archive_rewrite::rewrite_atomic`] (writes `.cbz.tmp`,
//!      rotates `.bak` slots, renames over the original, fsyncs the
//!      parent directory). Output respects the per-library
//!      `archive_backup_retain_count`.
//!   5. Invalidate the zip-LRU entry for this issue so subsequent
//!      reader opens see the rewritten file.
//!   6. Update bookkeeping on the `issues` row:
//!      `last_rewrite_at`, `last_rewrite_kind='sidecar'`,
//!      `thumbnails_generated_at=NULL`, `thumbnail_version=0`.
//!      Clearing the thumbnail stamps tells the catch-up sweep to
//!      regenerate them on the next post-scan pass — since the cover
//!      page bytes are identical, the regenerated thumbs are
//!      byte-equal; we clear them anyway because the scanner's
//!      content-hash dedupe pinpoint requires it.
//!   7. Emit an audit row: `admin.issue.sidecar_writeback` with the
//!      run id, ordinal, and the `suppressed_user_pins` array M3
//!      collected from `enumerate_suppressed_pins`.
//!   8. Enqueue a scoped issue rescan so the scanner re-ingests the
//!      freshly-written XML and the DB cache reflects the new state.
//!      The scanner's `dedupe_by_content_hash` keeps the row id
//!      stable.
//!   9. Release the rewrite mutex.

use crate::archive_rewrite::{self, RewriteError, mutex};
use crate::audit::{self, AuditEntry};
use crate::jobs::JOB_MAX_ATTEMPTS;
use crate::jobs::busy_requeue::{self, Requeue};
use crate::library::event_log::{self, Action, Category, NewEvent, Severity};
use crate::state::AppState;
use apalis::prelude::*;
use archive::ArchiveLimits;
use archive::cbz::Cbz;
use archive::cbz_write::{RebuildPlan, RebuildSummary, rebuild};
use chrono::Utc;
use entity::{issue, metadata_run};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RewriteIssueSidecarsJob {
    pub issue_id: String,
    /// Pre-serialized ComicInfo.xml — composed by the apply worker
    /// via `compose_comicinfo` + `parsers::comicinfo::serialize`. We
    /// pass the bytes rather than the struct so the job stays cheap
    /// to enqueue (no full DB join inside this worker) and the audit
    /// row can include the exact bytes that landed in the archive.
    pub comic_info_xml: String,
    /// Pre-serialized MetronInfo.xml. Same pattern as above.
    pub metron_info_xml: String,
    /// Field-provenance keys whose composer output preferred the DB
    /// value over the provider's (Q4 UX surface). Forwarded into the
    /// audit row so retrospective drill-downs show which fields were
    /// preserved against the provider's offering.
    #[serde(default)]
    pub suppressed_user_pins: Vec<String>,
    pub actor_id: Option<Uuid>,
    pub actor_ip: Option<String>,
    pub actor_ua: Option<String>,
    /// `metadata_run.id` that triggered this rewrite; surfaces in
    /// audit + the Runs feed so an operator can correlate apply rows
    /// with the XML write that followed.
    pub triggering_run_id: Option<Uuid>,
    pub triggering_run_ordinal: Option<i32>,
    /// Set to `true` by the series-scope apply path
    /// ([`crate::metadata::apply::apply_series_via_sidecar`]). When
    /// true, the worker writes the XML but does **not** enqueue a
    /// per-issue rescan — the series caller has already scheduled a
    /// single series-scoped rescan after the loop completes.
    /// `#[serde(default)]` so jobs queued before M4 still deserialize
    /// with the legacy "always rescan" behaviour.
    #[serde(default)]
    pub skip_rescan: bool,
    /// Busy-mutex requeue counter (see [`requeue_busy`]). New enqueues
    /// leave it 0; the worker bumps it on each rewrite-lock collision.
    #[serde(default)]
    pub attempt: u32,
    /// Metadata-only DB writes the apply decided on but that must land
    /// **only after the XML is actually in the archive** — per-field
    /// provenance, variant covers, `last_metadata_sync_at` (WP-2.6 (f),
    /// audit DI-10). Pre-fix `apply_issue_via_sidecar` wrote them at
    /// enqueue time, so a rewrite that failed at open (CBR/CBT, malformed
    /// zip) left the DB attributing provider values that never reached
    /// the file. `None` for jobs from the drift-flush endpoint and
    /// pre-upgrade payloads.
    #[serde(default)]
    pub post_apply: Option<PostRewriteWrites>,
}

/// One `field_provenance` upsert deferred until the rewrite succeeds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProvenanceWrite {
    /// [`crate::metadata::MetadataField::key`].
    pub field: String,
    /// The provider that supplied the value (`SetBy::Provider`).
    pub source: crate::metadata::identifier::Source,
    pub source_external_id: Option<String>,
}

/// The apply-time decisions that become DB rows once the archive holds
/// the new XML. Carried on [`RewriteIssueSidecarsJob::post_apply`] and
/// applied by [`apply_post_rewrite_writes`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PostRewriteWrites {
    #[serde(default)]
    pub provenance: Vec<ProvenanceWrite>,
    /// Variant covers to persist via `writers::set_issue_variants`
    /// (presentational rows the XML can't carry). Empty = leave the
    /// existing set alone.
    #[serde(default)]
    pub variants: Vec<crate::metadata::provider::VariantCoverCandidate>,
    /// Provider the variant rows are attributed to.
    pub variants_source: Option<crate::metadata::identifier::Source>,
    /// Stamp `issue.last_metadata_sync_at = now` after the rewrite.
    #[serde(default)]
    pub bump_sync: bool,
    /// WP-7.8: provider reprints to persist via `writers::set_issue_reprints`
    /// (neither XML schema carries them). Empty = leave the existing set
    /// alone. The apply already ran the user-precedence decision.
    #[serde(default)]
    pub reprints: Vec<crate::metadata::provider::ReprintCandidate>,
    /// Provider the reprint rows (and their `reprints` provenance) are
    /// attributed to.
    #[serde(default)]
    pub reprints_source: Option<crate::metadata::identifier::Source>,
    #[serde(default)]
    pub reprints_source_external_id: Option<String>,
    /// Audit 2026-11 N-M3: the provider's primary cover when the apply ran
    /// with `apply_cover`. Fetched and written through
    /// `writers::apply_cover` (the DB-direct apply's writer, which computes
    /// the perceptual hashes) only after the rewrite succeeded; a failed or
    /// refused rewrite fetches nothing. `None` = leave the cover alone.
    #[serde(default)]
    pub primary_cover: Option<PrimaryCoverWrite>,
}

/// A provider primary cover deferred until the rewrite lands. Carries the
/// URL, not the bytes: the job payload lives in Redis, and fetching after
/// the rewrite means a failed rewrite never downloads anything.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrimaryCoverWrite {
    pub url: String,
    /// The cover provider's issue identifier: the writer derives
    /// `issue_cover.source_provider` / `source_external_id` from it, as on
    /// the DB-direct path.
    #[serde(default)]
    pub identifier: Option<crate::metadata::identifier::Identifier>,
    /// The apply's `cover_overwrite_policy`, enforced by the writer.
    pub policy: crate::metadata::writers::CoverOverwritePolicy,
    /// The apply's admin-only `override_user_edits`: replace a primary an
    /// admin chose ("Use as cover"). Without it the writer keeps the
    /// choice, even one made after the apply was queued.
    #[serde(default)]
    pub override_user_edits: bool,
}

/// How the handler treated a failed rewrite. Decides the apalis return
/// value and the severity of the library event, and whether the
/// triggering metadata run is marked failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureDisposition {
    /// A deliberate refusal — `sidecar_refusal` (CBR / CB7 without
    /// conversion, unknown extension), the library's master writeback
    /// toggle off, or the issue row gone. Retrying can't change the
    /// answer, so the handler returns `Ok` and the task completes.
    Refused,
    /// A transient failure (I/O, archive write, CBR conversion, DB) on an
    /// attempt with budget left: the handler returns `Err` and apalis
    /// reschedules the task.
    Retrying { attempt: usize },
    /// A transient failure on the last budgeted attempt
    /// ([`JOB_MAX_ATTEMPTS`]): apalis dead-letters the task after this
    /// `Err`, so the handler records the drop first (error-severity event,
    /// run marked failed).
    Final { attempts: usize },
}

impl FailureDisposition {
    fn for_error(err: &WritebackError, attempt: usize) -> Self {
        if err.is_refusal() {
            Self::Refused
        } else if attempt >= JOB_MAX_ATTEMPTS {
            Self::Final { attempts: attempt }
        } else {
            Self::Retrying { attempt }
        }
    }
}

/// `attempt` is apalis's own counter for this task (1-based, already
/// incremented for the running attempt); the worker registration passes it
/// through [`crate::jobs::with_retry_budget_attempt`]. Transient failures
/// return `Err` so that budget applies — pre-fix every failed rewrite
/// returned `Ok`, so a NAS hiccup or a locked file was never retried and
/// the apply's deferred writes were silently lost (audit N-M7).
pub async fn handle(
    job: RewriteIssueSidecarsJob,
    state: Data<AppState>,
    attempt: Attempt,
) -> Result<(), BoxDynError> {
    let state: AppState = (*state).clone();

    let mut redis = state.jobs.redis.clone();
    let token = match mutex::try_claim(&mut redis, &job.issue_id, mutex::SIDECAR_TTL_SECS).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            // Another rewrite of this issue is in flight (page edit or a
            // sibling sidecar job). Pre-fix this returned `Ok` with a log
            // line claiming "the caller will re-enqueue" — nothing did, so
            // the write was silently dropped (audit DI-13). Requeue with
            // backoff, same as the page editor.
            return requeue_busy(&state, &job).await;
        }
        Err(e) => {
            // Redis itself failed. Surface it to apalis so the job is
            // retried (`JOB_MAX_ATTEMPTS`) and dead-lettered if Redis stays
            // down — a requeue push would fail the same way.
            tracing::error!(
                issue_id = %job.issue_id,
                error = %e,
                "sidecar writeback: mutex claim failed; returning Err for apalis retry",
            );
            return Err(e.into());
        }
    };
    // Keep the lock alive for as long as the blocking rewrite runs (a
    // NAS-hosted omnibus can outlive the 120s TTL). Dropped before release.
    let heartbeat = mutex::Heartbeat::start(
        state.jobs.redis.clone(),
        job.issue_id.clone(),
        token.clone(),
        mutex::SIDECAR_TTL_SECS,
    );

    let outcome = rewrite_one_issue(
        &state,
        &job.issue_id,
        job.comic_info_xml.clone(),
        job.metron_info_xml.clone(),
    )
    .await;
    drop(heartbeat);
    let mut redis = state.jobs.redis.clone();
    mutex::release(&mut redis, &job.issue_id, &token).await;

    let result = match &outcome {
        Ok(r) => {
            audit_writeback(&state, &job, &outcome, None).await;
            r
        }
        Err(e) => {
            // Failed rewrite: no rescan (it would just re-ingest the
            // original file) and — crucially — none of the deferred
            // metadata writes. The DB keeps describing the file as it
            // actually is. What happens next depends on the error:
            let disposition = FailureDisposition::for_error(e, attempt.current());
            audit_writeback(&state, &job, &outcome, Some(disposition)).await;
            let message = format!("sidecar writeback for {}: {e}", job.issue_id);
            return match disposition {
                FailureDisposition::Refused => Ok(()),
                FailureDisposition::Retrying { attempt } => {
                    tracing::warn!(
                        issue_id = %job.issue_id,
                        attempt,
                        error = %e,
                        "sidecar writeback: rewrite failed; returning Err for apalis retry",
                    );
                    Err(message.into())
                }
                FailureDisposition::Final { attempts } => {
                    tracing::error!(
                        issue_id = %job.issue_id,
                        attempts,
                        error = %e,
                        "sidecar writeback: rewrite failed on the last attempt; giving up",
                    );
                    fail_triggering_run(
                        &state,
                        &job,
                        &format!("{RUN_ERROR_PREFIX} failed after {attempts} attempts: {e}"),
                    )
                    .await;
                    Err(message.into())
                }
            };
        }
    };

    // This may be the operator's Retry of a dead-lettered job (Background
    // work re-enqueues a fresh copy) after an earlier attempt marked the run
    // failed: the write has now landed, so the run is complete again.
    clear_triggering_run_failure(&state, &job).await;

    // Best-effort scan enqueue after success. The series-scope apply
    // path sets `skip_rescan=true` because it already enqueued a single
    // series-scoped rescan after the iteration. Errors here only log;
    // the rewrite already landed and operators can re-trigger manually.
    if !job.skip_rescan
        && let Err(e) =
            enqueue_scoped_rescan(&state, &result.library_id, &result.series_id, &job.issue_id)
                .await
    {
        tracing::error!(
            issue_id = %job.issue_id,
            error = %e,
            "sidecar writeback: scoped rescan enqueue failed",
        );
    }

    // The XML is in the archive and the rescan is queued: now record
    // what the apply decided (provenance, variants, sync stamp). The
    // provenance rows are stamped at the rewrite time so the rescan —
    // which may well run after this — sees them as carried by the XML
    // and ingests the values instead of protecting the stale columns.
    if let Some(post) = &job.post_apply {
        apply_post_rewrite_writes(&state, &job.issue_id, post, result.rewritten_at).await;
    }

    Ok(())
}

/// The rewrite mutex is shared with the page editor, so a sidecar job can
/// land while an edit of the same issue is in flight. Requeue with a
/// short pacing delay ([`busy_requeue`]) and give up loudly — library
/// event + the triggering run marked failed — only after the retry budget
/// comfortably outlasts the longest mutex TTL. A busy lock is not an
/// error, so the handler returns `Ok` after requeueing; only a failed push
/// becomes `Err` so apalis retries the original task instead of losing it.
async fn requeue_busy(state: &AppState, job: &RewriteIssueSidecarsJob) -> Result<(), BoxDynError> {
    use apalis::prelude::TaskSink;
    let storage = state.jobs.rewrite_issue_sidecars_storage.clone();
    let outcome =
        busy_requeue::requeue_busy("sidecar writeback", &job.issue_id, job.attempt, |attempt| {
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
        })
        .await?;
    let Requeue::Exhausted { attempts } = outcome else {
        return Ok(());
    };
    if let Ok(Some(row)) = issue::Entity::find_by_id(job.issue_id.clone())
        .one(&state.db)
        .await
    {
        event_log::record(
            &state.db,
            NewEvent::new(
                row.library_id,
                Category::Archive,
                Action::Errored,
                Severity::Error,
                format!(
                    "Sidecar writeback dropped for {}: rewrite lock busy",
                    row.slug
                ),
            )
            .entity("issue", row.id.clone(), Some(row.slug.clone()))
            .detail(serde_json::json!({
                "attempts": attempts,
                "triggering_run_id": job.triggering_run_id,
            })),
        )
        .await;
    }
    fail_triggering_run(
        state,
        job,
        &format!("{RUN_ERROR_PREFIX} dropped: archive lock still busy after {attempts} requeues"),
    )
    .await;
    Ok(())
}

/// Every `error_summary` this job writes starts with this, so a later
/// success can tell its own failure apart from a search failure.
const RUN_ERROR_PREFIX: &str = "sidecar rewrite";

/// Mark the metadata run that queued this rewrite `failed` (status +
/// `error_summary`) and tell any client waiting on it. Without this the
/// batch view kept showing a dropped write as a complete, applied run
/// (audit N-M7). No-op for jobs with no run (drift flush, manual edits).
async fn fail_triggering_run(state: &AppState, job: &RewriteIssueSidecarsJob, error: &str) {
    let Some(run_id) = job.triggering_run_id else {
        return;
    };
    if let Err(e) = crate::metadata::orchestrator::fail_run(&state.db, run_id, error).await {
        tracing::warn!(
            run_id = %run_id,
            issue_id = %job.issue_id,
            error = %e,
            "sidecar writeback: could not mark the triggering run failed",
        );
    }
    crate::metadata::apply::emit_failed_for_run(state, run_id, error).await;
}

/// Undo [`fail_triggering_run`] once a later attempt of the same rewrite
/// succeeds. Only a run whose `error_summary` this job wrote
/// ([`RUN_ERROR_PREFIX`]) is touched — a search failure stays failed.
async fn clear_triggering_run_failure(state: &AppState, job: &RewriteIssueSidecarsJob) {
    use crate::metadata::orchestrator::status;
    let Some(run_id) = job.triggering_run_id else {
        return;
    };
    let row = match metadata_run::Entity::find_by_id(run_id)
        .one(&state.db)
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(run_id = %run_id, error = %e, "sidecar writeback: run lookup failed");
            return;
        }
    };
    let ours = row
        .error_summary
        .as_deref()
        .is_some_and(|s| s.starts_with(RUN_ERROR_PREFIX));
    if row.status != status::FAILED || !ours {
        return;
    }
    let mut am: metadata_run::ActiveModel = row.into();
    am.status = Set(status::COMPLETED.to_owned());
    am.error_summary = Set(None);
    am.finished_at = Set(Some(Utc::now().fixed_offset()));
    if let Err(e) = am.update(&state.db).await {
        tracing::warn!(
            run_id = %run_id,
            issue_id = %job.issue_id,
            error = %e,
            "sidecar writeback: could not clear the run's earlier failure",
        );
    } else {
        tracing::info!(run_id = %run_id, issue_id = %job.issue_id, "sidecar writeback: retried rewrite landed; run completed again");
    }
}

/// Land the apply's deferred metadata-only writes now that the XML is in
/// the archive: per-field provenance (the XML can't say "ComicVine set
/// this on date X"), variant-cover rows (neither schema carries them) and
/// the `last_metadata_sync_at` stamp. Best-effort — a failure here never
/// fails the job; each is logged. Exposed for the inline series path and
/// the integration tests.
///
/// `rewritten_at` is the archive's new `last_sidecar_rewrite_at`. The
/// provenance rows are recorded **at** that instant: in a writeback
/// library the scanner lets a provider-tier field follow the file only
/// when its provenance is no newer than the last sidecar rewrite (the
/// XML carries it). Stamping them `now()` — a few ms after the rewrite —
/// made every value this apply wrote look like DB-only drift, so the
/// scoped rescan protected the old column and the new value never left
/// the archive (owner bug: Chew #14's picked description).
pub async fn apply_post_rewrite_writes(
    state: &AppState,
    issue_id: &str,
    post: &PostRewriteWrites,
    rewritten_at: chrono::DateTime<chrono::FixedOffset>,
) {
    use crate::metadata::writers::{self, SetBy};
    for p in &post.provenance {
        let Ok(field) = <crate::metadata::MetadataField as std::str::FromStr>::from_str(&p.field)
        else {
            continue;
        };
        if let Err(e) = writers::write_field_provenance_at(
            &state.db,
            "issue",
            issue_id,
            field,
            SetBy::Provider(p.source),
            p.source_external_id.clone(),
            rewritten_at,
        )
        .await
        {
            tracing::warn!(issue_id, field = %p.field, error = %e, "sidecar writeback: field_provenance write failed");
        }
    }
    if let (false, Some(source)) = (post.variants.is_empty(), post.variants_source)
        && let Err(e) = writers::set_issue_variants(
            &state.db,
            &state.cfg().data_path,
            issue_id,
            &post.variants,
            SetBy::Provider(source),
        )
        .await
    {
        tracing::warn!(issue_id, error = %e, "sidecar writeback: variant covers write failed");
    }
    if let (false, Some(source)) = (post.reprints.is_empty(), post.reprints_source) {
        let res = async {
            let specs = writers::reprint_specs(&state.db, issue_id, &post.reprints).await?;
            writers::set_issue_reprints(
                &state.db,
                issue_id,
                specs,
                SetBy::Provider(source),
                post.reprints_source_external_id.clone(),
            )
            .await
        }
        .await;
        if let Err(e) = res {
            tracing::warn!(issue_id, error = %e, "sidecar writeback: reprints write failed");
        }
    }
    if let Some(cover) = &post.primary_cover {
        write_primary_cover(state, issue_id, cover).await;
    }
    if post.bump_sync {
        let am = issue::ActiveModel {
            id: Set(issue_id.to_owned()),
            last_metadata_sync_at: Set(Some(Utc::now().fixed_offset())),
            updated_at: Set(Utc::now().fixed_offset()),
            ..Default::default()
        };
        if let Err(e) = am.update(&state.db).await {
            tracing::warn!(issue_id, error = %e, "sidecar writeback: last_metadata_sync_at stamp failed");
        }
    }
}

/// The deferred half of the sidecar apply's `apply_cover`: fetch through
/// the shared provider-cover path (`writers::fetch_cover_bytes` — https
/// only, SSRF-vetted, magic-sniffed; every provider's `fetch_cover` is that
/// call) and store it with `writers::apply_cover` under the apply's
/// overwrite policy, exactly as the DB-direct `write_issue_fields` does.
/// Best-effort like the other deferred writes: the XML already landed, so a
/// fetch or write failure is logged, not retried.
async fn write_primary_cover(state: &AppState, issue_id: &str, cover: &PrimaryCoverWrite) {
    use crate::metadata::writers::{self, CoverFetchError, CoverOverwritePolicy, CoverWrite};
    use sea_orm::{ColumnTrait, PaginatorTrait, QueryFilter};
    // The policy would refuse the write anyway: don't download for nothing.
    if cover.policy != CoverOverwritePolicy::Always {
        let has_primary = entity::issue_cover::Entity::find()
            .filter(entity::issue_cover::Column::IssueId.eq(issue_id))
            .filter(entity::issue_cover::Column::Kind.eq("primary"))
            .filter(entity::issue_cover::Column::Ordinal.eq(0))
            .filter(entity::issue_cover::Column::IsActive.eq(true))
            .count(&state.db)
            .await;
        match has_primary {
            Ok(0) => {}
            Ok(_) => {
                tracing::debug!(
                    issue_id,
                    "sidecar writeback: primary cover kept (overwrite policy)"
                );
                return;
            }
            Err(e) => {
                tracing::warn!(issue_id, error = %e, "sidecar writeback: primary cover lookup failed");
                return;
            }
        }
    }
    let bytes = match writers::fetch_cover_bytes(&cover.url).await {
        Ok(bytes) => bytes,
        // A CDN behind a bot challenge (GCD): a clean skip, not a failure.
        Err(CoverFetchError::Blocked { host }) => {
            tracing::info!(
                issue_id,
                host,
                "sidecar writeback: primary cover unavailable (host refuses server-side downloads)"
            );
            return;
        }
        Err(e) => {
            tracing::warn!(issue_id, url = %cover.url, error = %e, "sidecar writeback: primary cover fetch failed");
            return;
        }
    };
    let write = CoverWrite {
        issue_id,
        kind: "primary",
        ordinal: 0,
        identifier: cover.identifier.as_ref(),
        source_url: Some(&cover.url),
        variant_label: None,
        variant_artist_person_id: None,
        bytes: &bytes,
        width: None,
        height: None,
        override_user_pin: cover.override_user_edits,
    };
    match writers::apply_cover(&state.db, &state.cfg().data_path, write, cover.policy).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            tracing::debug!(
                issue_id,
                "sidecar writeback: primary cover kept (overwrite policy or a chosen cover)"
            );
        }
        Err(e) => {
            tracing::warn!(issue_id, error = %e, "sidecar writeback: primary cover write failed");
        }
    }
}

/// Inner result captured for audit + post-job rescan trigger.
pub(crate) struct RewriteResult {
    pub library_id: Uuid,
    pub series_id: Uuid,
    /// The archive path *after* the rewrite — the new `.cbz` when a CBR
    /// was converted first, else the source path.
    pub archive_path: PathBuf,
    #[allow(dead_code)]
    pub summary: RebuildSummary,
    pub backup_path: Option<PathBuf>,
    /// The `last_sidecar_rewrite_at` this rewrite stamped.
    pub rewritten_at: chrono::DateTime<chrono::FixedOffset>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum WritebackError {
    #[error("issue {0} not found")]
    IssueGone(String),
    #[error("library {0} writeback disabled (allow_archive_writeback=false)")]
    WritebackDisabled(Uuid),
    /// The archive can't take a sidecar rewrite: an unknown extension, or a
    /// CBR / CB7 (no writer for either) in a library that hasn't allowed
    /// conversion to CBZ (`auto_convert_cbr_on_scan` /
    /// `auto_convert_cb7_on_scan` false).
    #[error("{0}")]
    UnsupportedFormat(String),
    #[error("cbr conversion: {0}")]
    Convert(#[from] crate::library::scanner::cbr_convert::CbrConvertError),
    #[error("rewrite: {0}")]
    Rewrite(#[from] RewriteError),
    #[error("db: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("archive: {0}")]
    Archive(#[from] archive::ArchiveError),
}

impl WritebackError {
    /// A refusal is an answer, not a failure: the archive can't take a
    /// sidecar (format gate), the library forbids archive writes, or the
    /// issue no longer exists. Retrying returns the same answer, so the
    /// job completes instead of consuming the apalis budget. Everything
    /// else — conversion, the rewrite itself, DB round-trips — may be a
    /// transient NAS / disk / connection fault and is retried.
    pub(crate) fn is_refusal(&self) -> bool {
        matches!(
            self,
            Self::IssueGone(_) | Self::WritebackDisabled(_) | Self::UnsupportedFormat(_)
        )
    }
}

/// Why the sidecar path can't take `file_path` in `lib`, or `None` when it
/// can. The apply dispatch consults this **before** choosing the XML-first
/// path so a refused archive falls back to the DB-direct apply with the
/// reason in `ApplyOutcome.sidecar_skip_reasons`, instead of enqueueing a
/// job that fails at open after the run was already marked applied
/// (WP-2.6 (f), audit DI-10). The same rule is re-checked inside
/// [`rewrite_one_issue`] for the series fan-out and drift-flush paths.
pub fn sidecar_refusal(lib: &entity::library::Model, file_path: &str) -> Option<String> {
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "cbz" | "cbt" => None,
        "cbr" if lib.auto_convert_cbr_on_scan => None,
        "cbr" => Some(
            "archive is CBR and the library does not allow CBR→CBZ conversion \
             (enable auto_convert_cbr_on_scan to write sidecars into RAR archives)"
                .to_owned(),
        ),
        "cb7" if lib.auto_convert_cb7_on_scan => None,
        "cb7" => Some(
            "archive is CB7 and the library does not allow CB7→CBZ conversion \
             (enable auto_convert_cb7_on_scan to write sidecars into 7z archives)"
                .to_owned(),
        ),
        other => Some(format!(
            "unsupported archive format for sidecar writeback: .{other} \
             (CBZ/CBT, or CBR/CB7 with conversion enabled)"
        )),
    }
}

/// Re-open the freshly-rebuilt archive at `tmp` and confirm it's a sound
/// replacement before the atomic swap: it must re-open cleanly, every
/// preserved source entry must still be present, and both sidecars must
/// have landed. `source_names` is the caller's snapshot of the entries the
/// rebuild must keep verbatim — the real pages, already filtered of the
/// sidecar/trash entries `rebuild` intentionally drops (see the snapshot in
/// [`rewrite_one_issue`]). Runs inside the `rewrite_atomic` closure, so any
/// failure aborts the rewrite with the original file untouched. This is what
/// makes `archive_backup_retain_count = 0` (no `.bak`) safe — a corrupt
/// or lossy rewrite never replaces a good original.
fn validate_rewrite(
    tmp: &std::path::Path,
    source_names: &[String],
    limits: ArchiveLimits,
) -> Result<(), RewriteError> {
    let new = Cbz::open(tmp, limits).map_err(|e| {
        RewriteError::ValidationFailed(format!("rewritten archive won't re-open: {e}"))
    })?;
    let new_names: std::collections::HashSet<&str> =
        new.entries().iter().map(|e| e.name.as_str()).collect();
    // Every preserved source entry (the pages) must survive verbatim. Sidecar
    // + trash entries were filtered out of `source_names` by the caller — the
    // rebuild drops those and re-adds the canonical sidecars, checked below.
    for name in source_names {
        if !new_names.contains(name.as_str()) {
            return Err(RewriteError::ValidationFailed(format!(
                "rewritten archive dropped entry {name:?}"
            )));
        }
    }
    // Both sidecars must be present (covers the case where the source had
    // neither and the rebuild was supposed to add them).
    let has = |needle: &str| new_names.iter().any(|n| n.eq_ignore_ascii_case(needle));
    if !has("ComicInfo.xml") {
        return Err(RewriteError::ValidationFailed(
            "ComicInfo.xml missing from rewritten archive".to_owned(),
        ));
    }
    if !has("MetronInfo.xml") {
        return Err(RewriteError::ValidationFailed(
            "MetronInfo.xml missing from rewritten archive".to_owned(),
        ));
    }
    Ok(())
}

/// Core write loop — opens the source archive, swaps in fresh ComicInfo
/// and MetronInfo entries, atomic-renames over the original, invalidates
/// the LRU, clears thumbnail stamps, and bumps `last_rewrite_*`.
///
/// Caller-provided invariants:
///   - The per-issue archive-rewrite mutex MUST be held when this is
///     called. The apalis [`handle`] above claims it; the series-inline
///     path in [`crate::metadata::apply::apply_series_via_sidecar`]
///     claims it around each iteration.
///   - The library must have `allow_archive_writeback=true`. This is
///     re-checked here as defense in depth.
///   - Caller does NOT enqueue a rescan; the apalis [`handle`] does
///     it (gated by `RewriteIssueSidecarsJob::skip_rescan`). The
///     series-inline path enqueues a single series-scope rescan after
///     the iteration completes.
pub(crate) async fn rewrite_one_issue(
    state: &AppState,
    issue_id: &str,
    comic_info_xml: String,
    metron_info_xml: String,
) -> Result<RewriteResult, WritebackError> {
    // Reload the issue row each time the worker fires so a concurrent
    // edit / move that landed between enqueue and now is reflected.
    let Some(row) = issue::Entity::find_by_id(issue_id).one(&state.db).await? else {
        return Err(WritebackError::IssueGone(issue_id.to_owned()));
    };

    // Defense-in-depth: the PATCH handler already refuses to set
    // metadata_writeback_enabled when allow_archive_writeback is off,
    // but a hand-edited DB row could violate the invariant. Refuse to
    // touch bytes when the master toggle is off.
    let lib = entity::library::Entity::find_by_id(row.library_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| WritebackError::IssueGone(format!("library missing for {}", row.id)))?;
    if !lib.allow_archive_writeback {
        return Err(WritebackError::WritebackDisabled(lib.id));
    }

    // Format gate — CBZ/CBT rewrite in place; CBR converts to a sibling
    // `.cbz` first when the library allows it; anything else is refused
    // here with a clear reason instead of failing at `Cbz::open` below
    // (WP-2.6 (f), audit DI-10).
    if let Some(reason) = sidecar_refusal(&lib, &row.file_path) {
        return Err(WritebackError::UnsupportedFormat(reason));
    }

    let source_path = PathBuf::from(&row.file_path);
    let cfg = state.cfg();
    let limits = cfg.archive_limits();
    let arch_limits = ArchiveLimits {
        max_entries: limits.max_entries,
        max_total_bytes: limits.max_total_bytes,
        max_entry_bytes: limits.max_entry_bytes,
        max_compression_ratio: limits.max_compression_ratio,
        max_nesting_depth: limits.max_nesting_depth,
        subprocess_wall_timeout: limits.subprocess_wall_timeout,
        subprocess_rss_bytes: limits.subprocess_rss_bytes,
    };
    let source_ext = source_path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let is_cbr = source_ext == "cbr";
    let is_cb7 = source_ext == "cb7";

    // CBR / CB7: RAR and 7z have no writer, so convert to CBZ first (same
    // converter the scanner's `auto_convert_cbr_on_scan` /
    // `auto_convert_cb7_on_scan` uses — the library opted into that
    // conversion, which `sidecar_refusal` verified). The original is kept as
    // `<name>.cbr.bak` / `<name>.cb7.bak`; the row is repointed at the `.cbz`
    // before the sidecar rewrite runs against it, so a failure in the second
    // step still leaves a readable, correctly-pointed archive.
    let archive_path = if is_cbr || is_cb7 {
        let src = source_path.clone();
        let converted = tokio::task::spawn_blocking(move || {
            if is_cb7 {
                crate::library::scanner::cbr_convert::convert_cb7_to_cbz(&src, arch_limits)
            } else {
                crate::library::scanner::cbr_convert::convert_cbr_to_cbz(&src, arch_limits)
            }
        })
        .await
        .map_err(|join_err| {
            WritebackError::Db(sea_orm::DbErr::Custom(format!("join: {join_err}")))
        })??;
        state.zip_lru.invalidate(&row.id);
        let am = issue::ActiveModel {
            id: Set(row.id.clone()),
            file_path: Set(converted.to_string_lossy().into_owned()),
            updated_at: Set(Utc::now().fixed_offset()),
            ..Default::default()
        };
        am.update(&state.db).await?;
        // First CBR conversion in a library stamps `cbr_convert_confirmed_at`
        // so the page editor stops prompting for the format change (CB7 has
        // no page-editor path, so it leaves the gate alone).
        if is_cbr && lib.cbr_convert_confirmed_at.is_none() {
            let lib_am = entity::library::ActiveModel {
                id: Set(lib.id),
                cbr_convert_confirmed_at: Set(Some(Utc::now().fixed_offset())),
                updated_at: Set(Utc::now().fixed_offset()),
                ..Default::default()
            };
            if let Err(e) = lib_am.update(&state.db).await {
                tracing::warn!(library_id = %lib.id, error = %e, "sidecar writeback: cbr_convert_confirmed_at stamp failed");
            }
        }
        tracing::info!(
            issue_id = %row.id,
            from = %source_path.display(),
            to = %converted.display(),
            ext = %source_ext,
            "sidecar writeback: converted to CBZ before rewrite",
        );
        converted
    } else {
        source_path
    };

    let is_cbt = archive_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cbt"));

    // `comic_info_xml` / `metron_info_xml` are already owned (function
    // takes them by value) — the spawn_blocking move closure consumes
    // them across the boundary, no extra clone needed.
    let retain_count = lib.archive_backup_retain_count;
    let src_path = archive_path.clone();

    let result = tokio::task::spawn_blocking(
        move || -> Result<(RebuildSummary, Option<PathBuf>), WritebackError> {
            let outcome = archive_rewrite::rewrite_atomic(&src_path, retain_count, |tmp| {
                if is_cbt {
                    rewrite_cbt_into(&src_path, tmp, comic_info_xml, metron_info_xml, arch_limits)
                } else {
                    rewrite_cbz_into(&src_path, tmp, comic_info_xml, metron_info_xml, arch_limits)
                }
            })?;
            // We don't propagate the per-call RebuildSummary out (the
            // atomic-rewrite closure already swallowed it); reconstruct a
            // minimal summary for the audit payload from the post-rewrite
            // archive on disk if the audit row needs counts. v1 keeps it
            // empty.
            Ok((RebuildSummary::default(), outcome.backup))
        },
    )
    .await
    .map_err(|join_err| {
        WritebackError::Db(sea_orm::DbErr::Custom(format!("join: {join_err}")))
    })??;

    let (summary, backup) = result;

    // Invalidate the zip-LRU entry so the next reader sees the new file.
    state.zip_lru.invalidate(&row.id);

    // Bookkeeping. Clear thumbnail stamps so the post-scan pipeline
    // re-derives them on the upcoming rescan. `last_sidecar_rewrite_at`
    // is the drift-detection stamp (only this path sets it);
    // `last_rewrite_at` is the UI's "last touched" stamp shared with
    // page edits.
    let now = Utc::now().fixed_offset();
    let am = issue::ActiveModel {
        id: Set(row.id.clone()),
        last_rewrite_at: Set(Some(now)),
        last_rewrite_kind: Set(Some("sidecar".to_owned())),
        last_sidecar_rewrite_at: Set(Some(now)),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        updated_at: Set(now),
        ..Default::default()
    };
    am.update(&state.db).await?;

    Ok(RewriteResult {
        library_id: row.library_id,
        series_id: row.series_id,
        archive_path,
        summary,
        backup_path: backup,
        rewritten_at: now,
    })
}

/// CBZ sidecar rewrite body, run inside the `rewrite_atomic` closure:
/// stream-copy every kept entry, swap in the fresh sidecars, then
/// validate the staged archive before the swap.
fn rewrite_cbz_into(
    src_path: &std::path::Path,
    tmp: &std::path::Path,
    comic_info_xml: String,
    metron_info_xml: String,
    arch_limits: ArchiveLimits,
) -> Result<(), RewriteError> {
    // Open the source inside the closure so the Cbz handle is dropped
    // before the rename swaps the file out from under it.
    let mut src = Cbz::open(src_path, arch_limits).map_err(RewriteError::ArchiveErr)?;
    // Snapshot the source entries the rebuild is contractually required
    // to preserve verbatim: the pages AND every foreign non-page entry
    // (`CoMet.xml`, notes, `.json` — WP-2.6 (b)). Only junk (dotfiles,
    // `Thumbs.db`, `__MACOSX`) and the two Folio-managed sidecars are
    // excluded: `rebuild` drops those and re-adds the freshly composed
    // root ComicInfo/MetronInfo, so a nested or duplicate sidecar
    // legitimately won't survive. Excluding them here keeps the
    // post-write validation from a false "dropped entry" abort (the two
    // sidecars' presence is checked separately in `validate_rewrite`).
    let source_names: Vec<String> = src
        .entries()
        .iter()
        .filter(|e| !archive::cbz::is_rewrite_skipped(&e.name))
        .map(|e| e.name.clone())
        .collect();
    let mut plan = RebuildPlan::new();
    plan.set_entry("ComicInfo.xml", comic_info_xml.into_bytes());
    plan.set_entry("MetronInfo.xml", metron_info_xml.into_bytes());
    let _summary = rebuild(&mut src, plan, tmp, arch_limits).map_err(RewriteError::ArchiveErr)?;
    // Drop the source handle before validation re-opens files.
    drop(src);
    // Validate-before-swap: confirm the freshly-built archive is a sound
    // replacement BEFORE rewrite_atomic renames it over the original. A
    // failure here aborts the rewrite with the original untouched — the
    // safety net that lets retain_count=0 (no `.bak`) run without risking
    // image-byte loss to a writer bug.
    validate_rewrite(tmp, &source_names, arch_limits)
}

/// CBT sidecar rewrite body (WP-2.6 (f)): tar has no stream-copy path, so
/// every kept entry is read and written back under its **original name**
/// (`cbt_write::write_entries`) — pages keep their names, so the reader's
/// natural sort and every page ordinal are unchanged. Kept = pages in
/// source order + every preserved extra (`rewrite_policy::preserved_extras`
/// minus the Folio pair) + the two fresh sidecars. Validated by re-opening
/// the staged tar before the swap, same contract as the CBZ path.
fn rewrite_cbt_into(
    src_path: &std::path::Path,
    tmp: &std::path::Path,
    comic_info_xml: String,
    metron_info_xml: String,
    arch_limits: ArchiveLimits,
) -> Result<(), RewriteError> {
    use archive::comic_archive::ComicArchive;
    let mut src =
        archive::cbt::Cbt::open(src_path, arch_limits).map_err(RewriteError::ArchiveErr)?;
    let page_names: Vec<String> = src.pages().iter().map(|e| e.name.clone()).collect();
    let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(page_names.len() + 4);
    for name in &page_names {
        let bytes = src
            .read_entry_bytes(name)
            .map_err(RewriteError::ArchiveErr)?;
        entries.push((name.clone(), bytes));
    }
    let extras = archive::rewrite_policy::preserved_extras(&mut src, false)
        .map_err(RewriteError::ArchiveErr)?;
    let mut must_survive: Vec<String> = page_names;
    for (name, bytes, _level) in extras {
        must_survive.push(name.clone());
        entries.push((name, bytes));
    }
    entries.push(("ComicInfo.xml".to_owned(), comic_info_xml.into_bytes()));
    entries.push(("MetronInfo.xml".to_owned(), metron_info_xml.into_bytes()));
    drop(src);
    archive::cbt_write::write_entries(entries, tmp, arch_limits)
        .map_err(RewriteError::ArchiveErr)?;

    let new = archive::cbt::Cbt::open(tmp, arch_limits).map_err(|e| {
        RewriteError::ValidationFailed(format!("rewritten archive won't re-open: {e}"))
    })?;
    let new_names: std::collections::HashSet<&str> =
        new.entries().iter().map(|e| e.name.as_str()).collect();
    for name in &must_survive {
        if !new_names.contains(name.as_str()) {
            return Err(RewriteError::ValidationFailed(format!(
                "rewritten archive dropped entry {name:?}"
            )));
        }
    }
    for sidecar in ["ComicInfo.xml", "MetronInfo.xml"] {
        if !new_names.iter().any(|n| n.eq_ignore_ascii_case(sidecar)) {
            return Err(RewriteError::ValidationFailed(format!(
                "{sidecar} missing from rewritten archive"
            )));
        }
    }
    Ok(())
}

async fn enqueue_scoped_rescan(
    state: &AppState,
    library_id: &Uuid,
    series_id: &Uuid,
    issue_id: &str,
) -> anyhow::Result<()> {
    use crate::jobs::scan_series;
    state
        .jobs
        .coalesce_scoped_scan(
            *library_id,
            *series_id,
            None,
            scan_series::JobKind::Issue,
            Some(issue_id.to_owned()),
            true, // force — the file's bytes changed
        )
        .await?;
    Ok(())
}

/// `disposition` is `Some` exactly when `outcome` is `Err`.
async fn audit_writeback(
    state: &AppState,
    job: &RewriteIssueSidecarsJob,
    outcome: &Result<RewriteResult, WritebackError>,
    disposition: Option<FailureDisposition>,
) {
    // Library stream (observability-split M3b): durable manifest row for the
    // sidecar rewrite, independent of the audit trail below.
    record_writeback_manifest(state, job, outcome, disposition).await;

    let payload = match outcome {
        Ok(r) => serde_json::json!({
            "issue_id": job.issue_id,
            "archive_path": r.archive_path.to_string_lossy(),
            "backup_path": r.backup_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            "suppressed_user_pins": job.suppressed_user_pins,
            "triggering_run_id": job.triggering_run_id,
            "triggering_run_ordinal": job.triggering_run_ordinal,
            "entries_written": r.summary.entries_written,
        }),
        Err(e) => serde_json::json!({
            "issue_id": job.issue_id,
            "error": e.to_string(),
            "disposition": disposition.map(|d| format!("{d:?}")),
            "triggering_run_id": job.triggering_run_id,
            "triggering_run_ordinal": job.triggering_run_ordinal,
        }),
    };

    let Some(actor_id) = job.actor_id else {
        tracing::info!(
            issue_id = %job.issue_id,
            ?payload,
            "sidecar writeback: anonymous run; no audit row",
        );
        return;
    };

    audit::record(
        &state.db,
        AuditEntry {
            actor_id,
            action: "admin.issue.sidecar_writeback",
            target_type: Some("issue"),
            target_id: Some(job.issue_id.clone()),
            payload,
            ip: job.actor_ip.clone(),
            user_agent: job.actor_ua.clone(),
        },
    )
    .await;
}

/// Emit an `archive` library-event for a sidecar writeback. `triggering_run_id`
/// is a metadata-run id (not a scan run), so it goes in `detail` rather than
/// the `scan_run_id` FK column; the event carries no scan link.
///
/// A failure's severity follows its [`FailureDisposition`]: an attempt
/// apalis will retry is a `warning`; a refusal and the last budgeted
/// attempt — the two cases where the write is really gone — are `error`.
async fn record_writeback_manifest(
    state: &AppState,
    job: &RewriteIssueSidecarsJob,
    outcome: &Result<RewriteResult, WritebackError>,
    disposition: Option<FailureDisposition>,
) {
    match outcome {
        Ok(r) => {
            let series = series_name(state, r.series_id).await;
            event_log::record(
                &state.db,
                NewEvent::new(
                    r.library_id,
                    Category::Archive,
                    Action::Updated,
                    Severity::Info,
                    format!(
                        "Sidecar metadata written back ({} entries)",
                        r.summary.entries_written
                    ),
                )
                .entity("issue", job.issue_id.clone(), None)
                .detail(serde_json::json!({
                    "entries_written": r.summary.entries_written,
                    "series_id": r.series_id,
                    "series": series,
                    "path": r.archive_path.to_string_lossy(),
                    "triggering_run_id": job.triggering_run_id,
                })),
            )
            .await;
        }
        Err(e) => {
            let Ok(Some(row)) = entity::issue::Entity::find_by_id(job.issue_id.clone())
                .one(&state.db)
                .await
            else {
                return;
            };
            let series = series_name(state, row.series_id).await;
            let (severity, summary) = match disposition {
                Some(FailureDisposition::Retrying { attempt }) => (
                    Severity::Warning,
                    format!(
                        "Sidecar writeback failed for {}; retrying (attempt {attempt} of {JOB_MAX_ATTEMPTS})",
                        row.slug
                    ),
                ),
                Some(FailureDisposition::Final { attempts }) => (
                    Severity::Error,
                    format!(
                        "Sidecar writeback failed for {} after {attempts} attempts",
                        row.slug
                    ),
                ),
                Some(FailureDisposition::Refused) | None => (
                    Severity::Error,
                    format!("Sidecar writeback refused for {}", row.slug),
                ),
            };
            event_log::record(
                &state.db,
                NewEvent::new(
                    row.library_id,
                    Category::Archive,
                    Action::Errored,
                    severity,
                    summary,
                )
                .entity("issue", row.id.clone(), Some(row.slug.clone()))
                .detail(serde_json::json!({
                    "error": e.to_string(),
                    "disposition": disposition.map(|d| format!("{d:?}")),
                    "series": series,
                    "path": row.file_path,
                    "triggering_run_id": job.triggering_run_id,
                })),
            )
            .await;
        }
    }
}

/// Best-effort series-name lookup for manifest enrichment.
async fn series_name(state: &AppState, series_id: Uuid) -> Option<String> {
    entity::series::Entity::find_by_id(series_id)
        .one(&state.db)
        .await
        .ok()
        .flatten()
        .map(|s| s.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Write a minimal stored-entry CBZ with the given entry names.
    /// Image-named entries get a real JPEG signature so the archive
    /// crate's open-time content sniff keeps them as pages.
    fn write_cbz(path: &std::path::Path, names: &[&str]) {
        let f = std::fs::File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for n in names {
            zw.start_file(*n, opts).unwrap();
            if archive::image_sniff::has_image_extension(n) {
                zw.write_all(b"\xFF\xD8\xFFx").unwrap();
            } else {
                zw.write_all(b"x").unwrap();
            }
        }
        zw.finish().unwrap();
    }

    #[test]
    fn validate_rewrite_accepts_preserved_entries_plus_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("new.cbz.tmp");
        write_cbz(
            &tmp,
            &[
                "page-001.png",
                "page-002.png",
                "ComicInfo.xml",
                "MetronInfo.xml",
            ],
        );
        let source = vec!["page-001.png".to_owned(), "page-002.png".to_owned()];
        assert!(validate_rewrite(&tmp, &source, ArchiveLimits::default()).is_ok());
    }

    #[test]
    fn validate_rewrite_rejects_dropped_entry() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("new.cbz.tmp");
        // page-002.png went missing in the rebuild — must be caught so the
        // swap is aborted and the original (only copy of those bytes when
        // retain_count = 0) is preserved.
        write_cbz(&tmp, &["page-001.png", "ComicInfo.xml", "MetronInfo.xml"]);
        let source = vec!["page-001.png".to_owned(), "page-002.png".to_owned()];
        let res = validate_rewrite(&tmp, &source, ArchiveLimits::default());
        assert!(
            matches!(res, Err(RewriteError::ValidationFailed(_))),
            "{res:?}"
        );
    }

    #[test]
    fn validate_rewrite_rejects_missing_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("new.cbz.tmp");
        // MetronInfo.xml absent from the rewrite.
        write_cbz(&tmp, &["page-001.png", "ComicInfo.xml"]);
        let source = vec!["page-001.png".to_owned()];
        let res = validate_rewrite(&tmp, &source, ArchiveLimits::default());
        assert!(
            matches!(res, Err(RewriteError::ValidationFailed(_))),
            "{res:?}"
        );
    }

    #[test]
    fn validate_rewrite_rejects_unopenable_archive() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("new.cbz.tmp");
        std::fs::write(&tmp, b"not a zip").unwrap();
        let res = validate_rewrite(&tmp, &[], ArchiveLimits::default());
        assert!(
            matches!(res, Err(RewriteError::ValidationFailed(_))),
            "{res:?}"
        );
    }

    /// Regression: an archive whose pages live under a subfolder and which
    /// carries a stale `Sub/ComicInfo.xml` *in addition to* a root one (the
    /// shape of a real "All Star Superman 002" CBZ). `rebuild` drops every
    /// sidecar and re-adds the canonical root ComicInfo/MetronInfo, so the
    /// nested copy legitimately won't survive — the rewrite used to abort
    /// repeatedly with `rewrite validation failed: rewritten archive dropped
    /// entry "Sub/ComicInfo.xml"`, leaving orphan `.tmp` files and never
    /// applying metadata. The full rebuild → validate path must now succeed,
    /// with the real pages preserved.
    #[test]
    fn rewrite_allows_dropped_nested_or_duplicate_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let src_path = dir.path().join("src.cbz");
        write_cbz(
            &src_path,
            &[
                "Sub Folder/page-001.jpg",
                "Sub Folder/page-002.jpg",
                "Sub Folder/ComicInfo.xml", // stale nested sidecar (gets dropped)
                "ComicInfo.xml",            // root sidecar (replaced)
            ],
        );

        let limits = ArchiveLimits::default();
        let mut src = Cbz::open(&src_path, limits).unwrap();
        // Snapshot the must-survive set exactly as `rewrite_one_issue` does.
        let source_names: Vec<String> = src
            .entries()
            .iter()
            .filter(|e| !archive::cbz::is_rewrite_skipped(&e.name))
            .map(|e| e.name.clone())
            .collect();
        // Only the two real pages are required to survive — neither sidecar.
        assert_eq!(
            source_names.len(),
            2,
            "sidecars must be filtered out: {source_names:?}"
        );

        let tmp = dir.path().join("out.cbz.tmp");
        let mut plan = RebuildPlan::new();
        plan.set_entry("ComicInfo.xml", b"<ComicInfo/>".to_vec());
        plan.set_entry("MetronInfo.xml", b"<MetronInfo/>".to_vec());
        rebuild(&mut src, plan, &tmp, limits).unwrap();
        drop(src);

        validate_rewrite(&tmp, &source_names, limits)
            .expect("a dropped nested/duplicate sidecar must not fail validation");

        // The pages survived; the canonical sidecars landed; the nested
        // duplicate is gone.
        let out = Cbz::open(&tmp, limits).unwrap();
        let names: std::collections::HashSet<&str> =
            out.entries().iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains("Sub Folder/page-001.jpg"));
        assert!(names.contains("Sub Folder/page-002.jpg"));
        assert!(names.contains("ComicInfo.xml"));
        assert!(names.contains("MetronInfo.xml"));
        assert!(
            !names.contains("Sub Folder/ComicInfo.xml"),
            "stale nested sidecar should be dropped, not preserved"
        );
    }

    /// WP-2.6 (b) / audit DI-11: a sidecar rewrite must carry every
    /// foreign non-page entry through — `CoMet.xml`, `notes.txt`, an
    /// embedded `.json` — and the validator must *require* them to
    /// survive (they're in the must-keep snapshot now, not filtered out).
    /// Junk is still dropped; a nested image is a page and streams
    /// through like any other.
    #[test]
    fn rewrite_preserves_foreign_sidecars_and_nested_pages() {
        let dir = tempfile::tempdir().unwrap();
        let src_path = dir.path().join("src.cbz");
        write_cbz(
            &src_path,
            &[
                "page-001.jpg",
                "extras/cover-alt.jpg",
                "CoMet.xml",
                "notes.txt",
                "meta.json",
                "Thumbs.db",
                "ComicInfo.xml",
            ],
        );

        let limits = ArchiveLimits::default();
        let mut src = Cbz::open(&src_path, limits).unwrap();
        let source_names: Vec<String> = src
            .entries()
            .iter()
            .filter(|e| !archive::cbz::is_rewrite_skipped(&e.name))
            .map(|e| e.name.clone())
            .collect();
        for must_keep in [
            "CoMet.xml",
            "notes.txt",
            "meta.json",
            "extras/cover-alt.jpg",
        ] {
            assert!(
                source_names.iter().any(|n| n == must_keep),
                "{must_keep} must be in the must-survive snapshot: {source_names:?}"
            );
        }
        assert!(!source_names.iter().any(|n| n == "Thumbs.db"));
        assert!(!source_names.iter().any(|n| n == "ComicInfo.xml"));

        let tmp = dir.path().join("out.cbz.tmp");
        let mut plan = RebuildPlan::new();
        plan.set_entry("ComicInfo.xml", b"<ComicInfo/>".to_vec());
        plan.set_entry("MetronInfo.xml", b"<MetronInfo/>".to_vec());
        rebuild(&mut src, plan, &tmp, limits).unwrap();
        drop(src);

        validate_rewrite(&tmp, &source_names, limits).expect("foreign sidecars survive");

        let mut out = Cbz::open(&tmp, limits).unwrap();
        let names: std::collections::HashSet<String> =
            out.entries().iter().map(|e| e.name.clone()).collect();
        for kept in [
            "CoMet.xml",
            "notes.txt",
            "meta.json",
            "extras/cover-alt.jpg",
        ] {
            assert!(names.contains(kept), "{kept} lost on rewrite: {names:?}");
        }
        assert!(!names.contains("Thumbs.db"), "junk must be dropped");
        // Byte-for-byte: `write_cbz` writes `x` for non-image entries.
        assert_eq!(out.read_entry_bytes_by_name("CoMet.xml").unwrap(), b"x");
        // The nested image is still a page after the rewrite.
        let pages: Vec<&str> = out.pages().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(pages, vec!["extras/cover-alt.jpg", "page-001.jpg"]);
    }
}
