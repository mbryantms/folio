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
    /// Derive the small (`@sm`) `srcset` cover variant from each issue's
    /// existing full cover (audit G9) — no archive access.
    CoverVariant,
}

impl BackfillKind {
    /// Stable wire/event identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            BackfillKind::CoverPhash => "cover_phash",
            BackfillKind::VariantCover => "variant_cover",
            BackfillKind::CoverVariant => "cover_variant",
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
        BackfillKind::CoverVariant => drain_cover_variants(&state).await,
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

/// Returns `(generated, skipped)` totals. Unlike the phash/variant drains
/// (which query rows still needing work), the `@sm` cover variant's existence
/// is on-disk only — there's no column to filter on — so this walks every
/// issue that has a generated cover by **id cursor**, deriving the small
/// variant from the full cover. Idempotent per issue (skips when `@sm`
/// already exists or no full cover is on disk), so re-runs are cheap no-ops.
async fn drain_cover_variants(state: &AppState) -> (u64, u64) {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
    const BATCH: u64 = 1000;
    let data_path = state.cfg().data_path.clone();
    let mut generated = 0u64;
    let mut skipped = 0u64;
    let mut cursor: Option<String> = None;

    for _ in 0..MAX_DRAIN_ITERS {
        let mut q = entity::issue::Entity::find()
            .filter(entity::issue::Column::ThumbnailsGeneratedAt.is_not_null())
            .order_by_asc(entity::issue::Column::Id)
            .limit(BATCH);
        if let Some(c) = &cursor {
            q = q.filter(entity::issue::Column::Id.gt(c.clone()));
        }
        let rows = match q.all(&state.db).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "cover-variant backfill: query failed");
                break;
            }
        };
        if rows.is_empty() {
            break;
        }
        let n = rows.len();
        cursor = rows.last().map(|r| r.id.clone());

        // Decode + downscale + encode is blocking CPU/IO work — run the
        // whole batch off the async runtime in one hop.
        let dp = data_path.clone();
        let ids: Vec<String> = rows.into_iter().map(|r| r.id).collect();
        let (g, s) = tokio::task::spawn_blocking(move || {
            let mut g = 0u64;
            let mut s = 0u64;
            for id in ids {
                match crate::library::thumbnails::generate_cover_small_from_existing(&dp, &id) {
                    Ok(Some(_)) => g += 1,
                    Ok(None) => s += 1,
                    Err(e) => {
                        tracing::debug!(issue_id = %id, error = %e, "cover-variant backfill: derive failed");
                        s += 1;
                    }
                }
            }
            (g, s)
        })
        .await
        .unwrap_or((0, 0));
        generated += g;
        skipped += s;

        if n < BATCH as usize {
            break;
        }
    }
    (generated, skipped)
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
