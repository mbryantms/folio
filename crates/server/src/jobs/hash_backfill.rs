//! `HashBackfillJob` — the second half of first-import lazy-hash mode
//! (roadmap WP-3.2, audit §3.1 "Import").
//!
//! A library with `trust_fingerprint_on_first_import = true` ingests its
//! first scan on size+mtime alone: each new row gets a path+size+mtime
//! fingerprint ([`lazy_fingerprint`]) as its id and as a placeholder
//! `content_hash`, with `hash_algorithm = 0` (pending). The scan finishes
//! without reading a single archive end to end; this job then drains the
//! pending rows one library at a time:
//!
//! 1. Stat the file. Missing, or size/mtime drifted from the row → leave it
//!    pending: the next scan either soft-deletes it (reconcile) or takes
//!    the update path, which always hashes and settles the row.
//! 2. BLAKE3 the bytes under the shared archive-work semaphore (so a
//!    backfill never starves a running scan or the reader), re-stat, and
//!    stamp `content_hash` + `hash_algorithm = 1` with an optimistic
//!    `WHERE hash_algorithm = 0 AND file_size = … AND file_mtime = …`
//!    guard so a concurrent rescan always wins.
//! 3. **Dedupe re-check.** Ingest-time dedupe was skipped for the pending
//!    row, so the job re-runs it: another settled row in the same library
//!    with the same content hash whose file is still on disk makes this a
//!    duplicate. The loser row is dropped (the survivor keeps its id —
//!    `issues.id` never changes), preferring to keep whichever copy
//!    carries reading progress. The job then enqueues a (non-force) scoped
//!    rescan of each affected series folder, which re-ingests the dropped
//!    path through the normal hashed path and surfaces the standard
//!    `DuplicateContent` health issue — the steady state a hashed first
//!    import would have produced. (A library rescan would not do: its
//!    folder-mtime skip never revisits an unchanged folder.)
//!
//!    The re-check honours the same policy as ingest-time dedupe (WP-3.3):
//!    - `library.dedupe_by_content = false` → **never** delete. Both rows
//!      are kept as separate issues (the Duplicates page groups them),
//!      exactly what an inline-hashed import of that library produces.
//!    - A row carrying an `issue_duplicate_decision` (the admin already
//!      ruled on it from the Duplicates page) is never the one deleted;
//!      when the would-be loser has a decision the pair is left alone.
//!
//! The issue id is **not** re-keyed when the hash lands (scanner
//! content-hash decoupling: `issues.id` stable, `content_hash` mutable), so
//! progress, markers, thumbnails and URLs made during the backfill survive
//! it, and retag detection after the backfill works exactly as for any
//! other row.
//!
//! Progress is visible through `GET /api/libraries/{slug}/hash-backfill`
//! (pending / total counts off the `issues_hash_pending_idx` partial index)
//! and a `library_events` row on completion. The worker runs at
//! `concurrency(1)`: re-enqueues queue behind the running drain and find
//! nothing left to do.
//!
//! [`lazy_fingerprint`]: crate::library::scanner::process::lazy_fingerprint

use crate::library::event_log::{self, Action, Category, NewEvent, Severity};
use crate::library::scanner::process::{
    HASH_ALGORITHM_BLAKE3, HASH_ALGORITHM_PENDING, file_fingerprint,
};
use crate::state::AppState;
use apalis::prelude::*;
use entity::issue;
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HashBackfillJob {
    pub library_id: Uuid,
}

/// Rows fetched per drain iteration. Small on purpose — each row is a
/// full-file read; the batch only bounds the id-cursor query.
const BATCH: u64 = 64;

/// Totals for one drain of a library's pending rows.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DrainOutcome {
    /// Rows whose real BLAKE3 landed.
    pub hashed: u64,
    /// Rows dropped by the post-hash dedupe re-check.
    pub duplicates_removed: u64,
    /// Rows left pending (file missing, changed since ingest, unreadable,
    /// or raced by a concurrent rescan). The next scan settles them.
    pub skipped: u64,
    /// Series whose folder held a dropped duplicate — each gets a scoped
    /// rescan so the dropped path surfaces as `DuplicateContent`.
    pub rescan_series: std::collections::BTreeSet<Uuid>,
}

#[derive(Debug, FromQueryResult)]
struct PendingRow {
    id: String,
    series_id: Uuid,
    file_path: String,
    file_size: i64,
    file_mtime: chrono::DateTime<chrono::FixedOffset>,
}

pub async fn handle(job: HashBackfillJob, state: Data<AppState>) -> Result<(), Error> {
    let state: AppState = (*state).clone();
    let library_id = job.library_id;
    let outcome = match drain_library(&state, library_id).await {
        Ok(o) => o,
        Err(e) => {
            tracing::error!(library_id = %library_id, error = %e, "hash backfill: drain failed");
            return Ok(());
        }
    };
    // Re-ingest the dropped paths through the ordinary hashed path so they
    // surface as `DuplicateContent` health issues (see module doc).
    for series_id in outcome.rescan_series {
        let folder = match entity::series::Entity::find_by_id(series_id)
            .one(&state.db)
            .await
        {
            Ok(Some(s)) => s.folder_path,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(series_id = %series_id, error = %e, "hash backfill: series lookup failed");
                continue;
            }
        };
        if let Err(e) = state
            .jobs
            .coalesce_scoped_scan(
                library_id,
                series_id,
                folder,
                crate::jobs::scan_series::JobKind::Series,
                None,
                false,
            )
            .await
        {
            tracing::warn!(series_id = %series_id, error = %e, "hash backfill: follow-up rescan enqueue failed");
        }
    }
    Ok(())
}

/// Hash every pending row of `library_id`. Public so integration tests can
/// drive the drain without a running apalis worker.
pub async fn drain_library(state: &AppState, library_id: Uuid) -> anyhow::Result<DrainOutcome> {
    let mut outcome = DrainOutcome::default();
    let Some(lib) = entity::library::Entity::find_by_id(library_id)
        .one(&state.db)
        .await?
    else {
        return Ok(outcome);
    };
    // Id cursor: rows the job decides to leave pending must not be
    // re-fetched forever.
    let mut cursor: Option<String> = None;
    loop {
        let mut q = issue::Entity::find()
            .select_only()
            .column(issue::Column::Id)
            .column(issue::Column::SeriesId)
            .column(issue::Column::FilePath)
            .column(issue::Column::FileSize)
            .column(issue::Column::FileMtime)
            .filter(issue::Column::LibraryId.eq(library_id))
            .filter(issue::Column::HashAlgorithm.eq(HASH_ALGORITHM_PENDING))
            .filter(issue::Column::RemovedAt.is_null())
            .order_by_asc(issue::Column::Id)
            .limit(BATCH);
        if let Some(c) = &cursor {
            q = q.filter(issue::Column::Id.gt(c.clone()));
        }
        let rows = q.into_model::<PendingRow>().all(&state.db).await?;
        let Some(last) = rows.last() else { break };
        cursor = Some(last.id.clone());
        for row in rows {
            match settle_row(state, &lib, &row).await {
                Ok(Settled::Hashed) => outcome.hashed += 1,
                Ok(Settled::Duplicate { dropped_series }) => {
                    outcome.hashed += 1;
                    outcome.duplicates_removed += 1;
                    outcome.rescan_series.insert(dropped_series);
                }
                Ok(Settled::Skipped) => outcome.skipped += 1,
                Err(e) => {
                    tracing::warn!(issue_id = %row.id, path = %row.file_path, error = %e, "hash backfill: row failed");
                    outcome.skipped += 1;
                }
            }
        }
    }

    tracing::info!(
        library_id = %library_id,
        hashed = outcome.hashed,
        duplicates_removed = outcome.duplicates_removed,
        skipped = outcome.skipped,
        "hash backfill: drain complete"
    );
    if outcome.hashed + outcome.skipped > 0 {
        event_log::record(
            &state.db,
            NewEvent::new(
                library_id,
                Category::File,
                Action::Completed,
                if outcome.skipped > 0 {
                    Severity::Warning
                } else {
                    Severity::Info
                },
                format!(
                    "Content hashes backfilled for {} file(s){}{}",
                    outcome.hashed,
                    if outcome.duplicates_removed > 0 {
                        format!("; {} duplicate(s) dropped", outcome.duplicates_removed)
                    } else {
                        String::new()
                    },
                    if outcome.skipped > 0 {
                        format!("; {} left for the next scan", outcome.skipped)
                    } else {
                        String::new()
                    },
                ),
            )
            .detail(serde_json::json!({
                "kind": "hash_backfill",
                "hashed": outcome.hashed,
                "duplicates_removed": outcome.duplicates_removed,
                "skipped": outcome.skipped,
            })),
        )
        .await;
    }
    Ok(outcome)
}

enum Settled {
    Hashed,
    Duplicate { dropped_series: Uuid },
    Skipped,
}

async fn settle_row(
    state: &AppState,
    lib: &entity::library::Model,
    row: &PendingRow,
) -> anyhow::Result<Settled> {
    let library_id = lib.id;
    let path = Path::new(&row.file_path).to_path_buf();
    let expected = (row.file_size, row.file_mtime.to_utc());
    // Cheap pre-check: don't pay a full read for a file the next scan
    // will re-ingest anyway.
    match file_fingerprint(&path) {
        Ok(fp) if fp == expected => {}
        _ => return Ok(Settled::Skipped),
    }

    let buffer_kb = state.cfg().scan_hash_buffer_kb;
    let hash = {
        let _permit = state
            .archive_work_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| anyhow::anyhow!("archive work semaphore closed: {e}"))?;
        let p = path.clone();
        tokio::task::spawn_blocking(move || {
            let hash = crate::library::hash::blake3_file_with_buffer(&p, buffer_kb)?;
            // Re-stat: bytes that changed mid-read hash to garbage.
            let after = file_fingerprint(&p)?;
            Ok::<_, anyhow::Error>((after == expected).then_some(hash))
        })
        .await
        .map_err(|e| anyhow::anyhow!("hash task failed: {e}"))??
    };
    let Some(hash) = hash else {
        return Ok(Settled::Skipped);
    };

    // Dedupe re-check (the ingest-time check was skipped for this row).
    // Library-scoped: a copy of the same file in another library is not a
    // duplicate of this one. Only settled rows can match — a pending row's
    // placeholder is never a real content hash.
    let prior = issue::Entity::find()
        .select_only()
        .column(issue::Column::Id)
        .column(issue::Column::SeriesId)
        .column(issue::Column::FilePath)
        .column(issue::Column::FileSize)
        .column(issue::Column::FileMtime)
        .filter(issue::Column::LibraryId.eq(library_id))
        .filter(issue::Column::ContentHash.eq(hash.clone()))
        .filter(issue::Column::Id.ne(row.id.clone()))
        .filter(issue::Column::RemovedAt.is_null())
        .order_by_asc(issue::Column::Id)
        .into_model::<PendingRow>()
        .one(&state.db)
        .await?;
    // `dedupe_by_content = false`: duplicates are kept as separate issues
    // (WP-3.3) — just settle the hash; the Duplicates page groups them.
    if let Some(prior) = prior
        && lib.dedupe_by_content
        && Path::new(&prior.file_path).exists()
    {
        // True duplicate. Keep the copy a reader already has progress on;
        // otherwise keep the row that was settled first (it is what an
        // inline-hashed import would have kept).
        // An admin verdict (`issue_duplicate_decision`) pins a row: it is
        // kept over an undecided copy, and never deleted.
        let row_decided = has_duplicate_decision(&state.db, &row.id).await?;
        let prior_decided = has_duplicate_decision(&state.db, &prior.id).await?;
        let keep_row = row_decided
            || (!prior_decided
                && has_progress(&state.db, &row.id).await?
                && !has_progress(&state.db, &prior.id).await?);
        let (keep, drop, dropped_series, drop_decided) = if keep_row {
            (
                row.id.as_str(),
                prior.id.as_str(),
                prior.series_id,
                prior_decided,
            )
        } else {
            (
                prior.id.as_str(),
                row.id.as_str(),
                row.series_id,
                row_decided,
            )
        };
        if drop_decided {
            // Both copies carry a verdict — leave the pair to the admin.
            return Ok(if stamp_hash(&state.db, row, &hash).await? {
                Settled::Hashed
            } else {
                Settled::Skipped
            });
        }
        let txn = state.db.begin().await?;
        if keep == row.id && !stamp_hash(&txn, row, &hash).await? {
            txn.rollback().await?;
            return Ok(Settled::Skipped);
        }
        drop_issue_row(&txn, drop).await?;
        txn.commit().await?;
        let data_dir = state.cfg().data_path.clone();
        let dropped = drop.to_owned();
        let _ = tokio::task::spawn_blocking(move || {
            crate::library::thumbnails::wipe_issue_thumbs(&data_dir, &dropped);
        })
        .await;
        tracing::info!(
            kept = %keep,
            dropped = %drop,
            path = %row.file_path,
            "hash backfill: duplicate content; dropped the redundant row",
        );
        return Ok(Settled::Duplicate { dropped_series });
    }
    // No live duplicate. A matching row whose file is gone is a stale
    // pre-move row; the next scan's reconcile soft-deletes it.

    if stamp_hash(&state.db, row, &hash).await? {
        Ok(Settled::Hashed)
    } else {
        Ok(Settled::Skipped)
    }
}

/// Settle one pending row. The guard makes a concurrent rescan (which
/// re-hashes changed files through the update path) always win. Returns
/// whether the row was updated.
async fn stamp_hash<C: ConnectionTrait>(
    db: &C,
    row: &PendingRow,
    hash: &str,
) -> anyhow::Result<bool> {
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            db.get_database_backend(),
            r"UPDATE issues
                 SET content_hash = $1, hash_algorithm = $2, updated_at = NOW()
               WHERE id = $3 AND hash_algorithm = $4
                 AND file_size = $5 AND file_mtime = $6",
            [
                hash.into(),
                HASH_ALGORITHM_BLAKE3.into(),
                row.id.clone().into(),
                HASH_ALGORITHM_PENDING.into(),
                row.file_size.into(),
                row.file_mtime.into(),
            ],
        ))
        .await?;
    Ok(res.rows_affected() == 1)
}

async fn has_duplicate_decision<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
) -> anyhow::Result<bool> {
    Ok(
        entity::issue_duplicate_decision::Entity::find_by_id(issue_id.to_owned())
            .count(db)
            .await?
            > 0,
    )
}

async fn has_progress<C: ConnectionTrait>(db: &C, issue_id: &str) -> anyhow::Result<bool> {
    Ok(entity::progress_record::Entity::find()
        .filter(entity::progress_record::Column::IssueId.eq(issue_id))
        .count(db)
        .await?
        > 0)
}

/// Hard-delete a duplicate row. FK'd tables (junctions, `issue_paths`,
/// progress, markers, …) cascade; the polymorphic `external_ids` /
/// `field_provenance` rows have no FK, so they go explicitly.
async fn drop_issue_row<C: ConnectionTrait>(db: &C, issue_id: &str) -> anyhow::Result<()> {
    for sql in [
        "DELETE FROM external_ids WHERE entity_type = 'issue' AND entity_id = $1",
        "DELETE FROM field_provenance WHERE entity_type = 'issue' AND entity_id = $1",
        "DELETE FROM issues WHERE id = $1",
    ] {
        db.execute_raw(Statement::from_sql_and_values(
            db.get_database_backend(),
            sql,
            [issue_id.into()],
        ))
        .await?;
    }
    Ok(())
}

/// `(pending, total)` live-issue counts for a library — the progress the
/// settings page renders. `pending` rides the partial index.
pub async fn progress<C: ConnectionTrait>(db: &C, library_id: Uuid) -> anyhow::Result<(u64, u64)> {
    let base = || {
        issue::Entity::find()
            .filter(issue::Column::LibraryId.eq(library_id))
            .filter(issue::Column::RemovedAt.is_null())
    };
    let pending = base()
        .filter(issue::Column::HashAlgorithm.eq(HASH_ALGORITHM_PENDING))
        .count(db)
        .await?;
    let total = base().count(db).await?;
    Ok((pending, total))
}

/// Enqueue a drain for `library_id` if it has pending rows. Called at the
/// end of every library / series scan (so a restart mid-backfill resumes on
/// the next scan) and by the admin "resume" action. Returns whether a job
/// was pushed. Best-effort: failures are logged, never propagated.
pub async fn enqueue_if_pending(state: &AppState, library_id: Uuid) -> bool {
    let pending = issue::Entity::find()
        .select_only()
        .column(issue::Column::Id)
        .filter(issue::Column::LibraryId.eq(library_id))
        .filter(issue::Column::HashAlgorithm.eq(HASH_ALGORITHM_PENDING))
        .filter(issue::Column::RemovedAt.is_null())
        .into_tuple::<String>()
        .one(&state.db)
        .await;
    match pending {
        Ok(Some(_)) => {}
        Ok(None) => return false,
        Err(e) => {
            tracing::warn!(library_id = %library_id, error = %e, "hash backfill: pending probe failed");
            return false;
        }
    }
    let mut storage = state.jobs.hash_backfill_storage.clone();
    match storage.push(HashBackfillJob { library_id }).await {
        Ok(_) => true,
        Err(e) => {
            tracing::error!(library_id = %library_id, error = %e, "hash backfill: enqueue failed");
            false
        }
    }
}
