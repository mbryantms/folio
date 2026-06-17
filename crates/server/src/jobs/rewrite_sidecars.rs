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
use crate::library::event_log::{self, Action, Category, NewEvent, Severity};
use crate::state::AppState;
use apalis::prelude::*;
use archive::ArchiveLimits;
use archive::cbz::Cbz;
use archive::cbz_write::{RebuildPlan, RebuildSummary, rebuild};
use chrono::Utc;
use entity::issue;
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
}

pub async fn handle(job: RewriteIssueSidecarsJob, state: Data<AppState>) -> Result<(), Error> {
    let state: AppState = (*state).clone();

    let mut redis = state.jobs.redis.clone();
    let token = match mutex::try_claim(&mut redis, &job.issue_id, mutex::SIDECAR_TTL_SECS).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            tracing::info!(
                issue_id = %job.issue_id,
                "sidecar writeback: mutex busy; skipping (caller will re-enqueue if needed)",
            );
            return Ok(());
        }
        Err(e) => {
            tracing::error!(
                issue_id = %job.issue_id,
                error = %e,
                "sidecar writeback: mutex claim failed",
            );
            return Ok(()); // soft fail; caller can retry
        }
    };

    let outcome = rewrite_one_issue(
        &state,
        &job.issue_id,
        job.comic_info_xml.clone(),
        job.metron_info_xml.clone(),
    )
    .await;
    let mut redis = state.jobs.redis.clone();
    mutex::release(&mut redis, &job.issue_id, &token).await;

    audit_writeback(&state, &job, &outcome).await;

    // Best-effort scan enqueue after success — gated on the outcome
    // so failed rewrites don't trigger a rescan that would just
    // re-ingest the original file. The series-scope apply path sets
    // `skip_rescan=true` because it already enqueued a single
    // series-scoped rescan after the iteration. Errors here only log;
    // the rewrite already landed and operators can re-trigger
    // manually.
    if !job.skip_rescan
        && let Ok(ref result) = outcome
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

    Ok(())
}

/// Inner result captured for audit + post-job rescan trigger.
pub(crate) struct RewriteResult {
    pub library_id: Uuid,
    pub series_id: Uuid,
    pub archive_path: PathBuf,
    #[allow(dead_code)]
    pub summary: RebuildSummary,
    pub backup_path: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum WritebackError {
    #[error("issue {0} not found")]
    IssueGone(String),
    #[error("library {0} writeback disabled (allow_archive_writeback=false)")]
    WritebackDisabled(Uuid),
    #[error("rewrite: {0}")]
    Rewrite(#[from] RewriteError),
    #[error("db: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("archive: {0}")]
    Archive(#[from] archive::ArchiveError),
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

    let archive_path = PathBuf::from(&row.file_path);
    let cfg = state.cfg();
    let limits = cfg.archive_limits();

    // `comic_info_xml` / `metron_info_xml` are already owned (function
    // takes them by value) — the spawn_blocking move closure consumes
    // them across the boundary, no extra clone needed.
    let retain_count = lib.archive_backup_retain_count;
    let src_path = archive_path.clone();

    let result = tokio::task::spawn_blocking(
        move || -> Result<(RebuildSummary, Option<PathBuf>), WritebackError> {
            let arch_limits = ArchiveLimits {
                max_entries: limits.max_entries,
                max_total_bytes: limits.max_total_bytes,
                max_entry_bytes: limits.max_entry_bytes,
                max_compression_ratio: limits.max_compression_ratio,
                max_nesting_depth: limits.max_nesting_depth,
                subprocess_wall_timeout: limits.subprocess_wall_timeout,
                subprocess_rss_bytes: limits.subprocess_rss_bytes,
            };
            let outcome = archive_rewrite::rewrite_atomic(&src_path, retain_count, |tmp| {
                // Open the source inside the closure so the Cbz handle is
                // dropped before the rename swaps the file out from under
                // it.
                let mut src =
                    Cbz::open(&src_path, arch_limits).map_err(RewriteError::ArchiveErr)?;
                // Snapshot the source entries the rebuild is contractually
                // required to preserve verbatim — i.e. the real pages. The
                // reader surfaces sidecars + trash (`.xml`/`.json`/`.txt`,
                // dotfiles, `Thumbs.db`, `__MACOSX`) through `entries()` too,
                // but `rebuild` intentionally drops every such entry and
                // re-adds the canonical ComicInfo/MetronInfo, so a nested or
                // duplicate sidecar legitimately won't survive. Excluding them
                // here keeps the post-write validation from a false "dropped
                // entry" abort on those (the two sidecars' presence is checked
                // separately in `validate_rewrite`).
                let source_names: Vec<String> = src
                    .entries()
                    .iter()
                    .filter(|e| !archive::cbz::is_rewrite_skipped(&e.name))
                    .map(|e| e.name.clone())
                    .collect();
                let mut plan = RebuildPlan::new();
                plan.set_entry("ComicInfo.xml", comic_info_xml.into_bytes());
                plan.set_entry("MetronInfo.xml", metron_info_xml.into_bytes());
                // `rebuild` returns RebuildSummary on success; we need it
                // outside the closure. Stash it in a captured slot via the
                // outer `Result` channel — but `rewrite_atomic`'s closure
                // returns Result<(), RewriteError>, so we use a side
                // channel.
                let _summary =
                    rebuild(&mut src, plan, tmp, arch_limits).map_err(RewriteError::ArchiveErr)?;
                // Drop the source handle before validation re-opens files.
                drop(src);
                // Validate-before-swap: confirm the freshly-built archive is a
                // sound replacement BEFORE rewrite_atomic renames it over the
                // original. A failure here aborts the rewrite with the
                // original untouched — the safety net that lets retain_count=0
                // (no `.bak`) run without risking image-byte loss to a writer
                // bug.
                validate_rewrite(tmp, &source_names, arch_limits)?;
                Ok(())
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
    // re-derives them on the upcoming rescan.
    let am = issue::ActiveModel {
        id: Set(row.id.clone()),
        last_rewrite_at: Set(Some(Utc::now().fixed_offset())),
        last_rewrite_kind: Set(Some("sidecar".to_owned())),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        updated_at: Set(Utc::now().fixed_offset()),
        ..Default::default()
    };
    am.update(&state.db).await?;

    Ok(RewriteResult {
        library_id: row.library_id,
        series_id: row.series_id,
        archive_path,
        summary,
        backup_path: backup,
    })
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

async fn audit_writeback(
    state: &AppState,
    job: &RewriteIssueSidecarsJob,
    outcome: &Result<RewriteResult, WritebackError>,
) {
    // Library stream (observability-split M3b): durable manifest row for the
    // sidecar rewrite, independent of the audit trail below.
    record_writeback_manifest(state, job, outcome).await;

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
async fn record_writeback_manifest(
    state: &AppState,
    job: &RewriteIssueSidecarsJob,
    outcome: &Result<RewriteResult, WritebackError>,
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
            event_log::record(
                &state.db,
                NewEvent::new(
                    row.library_id,
                    Category::Archive,
                    Action::Errored,
                    Severity::Error,
                    format!("Sidecar writeback failed for {}", row.slug),
                )
                .entity("issue", row.id.clone(), Some(row.slug.clone()))
                .detail(serde_json::json!({
                    "error": e.to_string(),
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
    fn write_cbz(path: &std::path::Path, names: &[&str]) {
        let f = std::fs::File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for n in names {
            zw.start_file(*n, opts).unwrap();
            zw.write_all(b"x").unwrap();
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
}
