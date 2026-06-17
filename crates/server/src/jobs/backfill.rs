//! `BackfillJob` — operator-initiated catch-up sweeps, run as background
//! apalis jobs instead of blocking the admin request (audit B17).
//!
//! Two kinds, both idempotent + resumable:
//!   - `CoverPhash` — compute perceptual hashes for archive-extracted covers
//!     scanned before pHash matching existed.
//!   - `VariantCover` — re-download provider variant covers whose on-disk
//!     artifact is missing (or that were only ever kept as hotlinks).
//!
//! Each handler drains the *whole* backlog in a loop (the old admin endpoint
//! capped a single synchronous call at 500 rows to dodge a reverse-proxy
//! timeout; a background job has no such limit), stopping when a batch makes
//! no forward progress — i.e. only undecodable covers / dead provider URLs
//! remain. On completion it emits a `backfill.completed` event so the
//! dashboard card can report the result. The worker runs at `concurrency(1)`,
//! so a redundant re-enqueue simply queues behind the running drain and
//! finds nothing left to do.

use crate::library::events::ScanEvent;
use crate::metadata::{phash, writers};
use crate::state::AppState;
use apalis::prelude::*;
use serde::{Deserialize, Serialize};

/// Which catch-up sweep to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillKind {
    CoverPhash,
    VariantCover,
}

impl BackfillKind {
    /// Stable wire/event identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            BackfillKind::CoverPhash => "cover_phash",
            BackfillKind::VariantCover => "variant_cover",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackfillJob {
    pub kind: BackfillKind,
}

/// Defensive cap on drain iterations — termination is already guaranteed
/// (each pass strictly shrinks the NULL/missing set), this just bounds a
/// pathological loop. 500 rows/iter × this is far beyond any real library.
const MAX_DRAIN_ITERS: u32 = 100_000;

pub async fn handle(job: BackfillJob, state: Data<AppState>) -> Result<(), Error> {
    let state: AppState = (*state).clone();
    let (processed, skipped) = match job.kind {
        BackfillKind::CoverPhash => drain_phash(&state).await,
        BackfillKind::VariantCover => drain_variant_covers(&state).await,
    };
    tracing::info!(
        kind = job.kind.as_str(),
        processed,
        skipped,
        "backfill: drain complete"
    );
    state.events.emit(ScanEvent::BackfillCompleted {
        kind: job.kind.as_str().to_owned(),
        processed,
        skipped,
    });
    Ok(())
}

/// Returns `(hashed, skipped)` totals across the whole drain.
async fn drain_phash(state: &AppState) -> (u64, u64) {
    let archive_limits = state.cfg().archive_limits();
    let batch = phash::BACKFILL_BATCH_CAP as u64;
    let mut hashed = 0u64;
    let mut skipped = 0u64;
    for _ in 0..MAX_DRAIN_ITERS {
        match phash::run_backfill(&state.db, archive_limits, batch).await {
            Ok(o) => {
                hashed += o.hashed as u64;
                skipped += (o.skipped + o.errored) as u64;
                // No forward progress this pass → drained, or only
                // undecodable covers remain (which would re-fail forever).
                if o.hashed == 0 {
                    break;
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "phash backfill: query failed");
                break;
            }
        }
    }
    (hashed, skipped)
}

/// Returns `(stored, skipped)` totals across the whole drain.
async fn drain_variant_covers(state: &AppState) -> (u64, u64) {
    let data_path = state.cfg().data_path.clone();
    let mut stored = 0u64;
    let mut skipped = 0u64;
    for _ in 0..MAX_DRAIN_ITERS {
        match writers::run_variant_cover_backfill(&state.db, &data_path).await {
            Ok(o) => {
                stored += o.stored as u64;
                skipped = o.skipped as u64; // last pass's residual dead-URL rows
                // No cover re-downloaded → drained, or only dead-URL rows
                // remain (re-applying metadata is the recovery path).
                if o.stored == 0 {
                    break;
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "variant cover backfill: query failed");
                break;
            }
        }
    }
    (stored, skipped)
}

/// Enqueue a backfill drain. Returns `false` only if the push itself fails.
pub async fn enqueue(app: &AppState, kind: BackfillKind) -> bool {
    let mut storage = app.jobs.backfill_storage.clone();
    match storage.push(BackfillJob { kind }).await {
        Ok(_) => true,
        Err(e) => {
            tracing::error!(error = %e, kind = kind.as_str(), "backfill: enqueue failed");
            false
        }
    }
}
