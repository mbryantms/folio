//! Daily orphan sweep for the thumbnail cache (M5).
//!
//! Walks `data_dir/thumbs/` and drops any artifact that doesn't correspond
//! to a still-present, still-active issue row. Catches:
//!   - issues confirmed-removed without their cleanup hook running
//!     (e.g. the auto-confirm sweep's bulk UPDATE doesn't fire per-row)
//!   - issues hard-deleted out from under the scanner
//!   - server crashes that interrupt a regenerate-while-rewrite
//!
//! Cheap: one query per run + a dirent scan. Best-effort — disk errors are
//! logged and the loop continues; we'd rather skip one path than fail the
//! whole sweep.

use crate::library::thumbnails;
use crate::state::AppState;
use entity::issue;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use std::collections::HashSet;

/// Run one sweep. Returns the count of wiped issue artifacts.
pub async fn run(state: &AppState) -> anyhow::Result<usize> {
    let data_path = &state.cfg().data_path;
    // Two parallel artifact trees keyed by issue id:
    //   - generated page/cover thumbs at `thumbs/{id}` + `thumbs/{id}.ext`
    //   - downloaded provider covers at `thumbs/issues/{id}/`
    // Sweep both; the second was previously read as a single phantom
    // `issues` issue and wiped wholesale on every run.
    let thumb_ids: HashSet<String> = thumbnails::list_issues_on_disk(data_path)?;
    let variant_ids: HashSet<String> = thumbnails::list_variant_cover_issues_on_disk(data_path)?;
    if thumb_ids.is_empty() && variant_ids.is_empty() {
        return Ok(0);
    }

    // Find issue ids that should keep their artifacts: active rows only.
    // Anything else (removed, removal_confirmed, missing entirely) is
    // eligible for cleanup.
    let candidates: Vec<String> = thumb_ids.union(&variant_ids).cloned().collect();
    let alive: Vec<String> = issue::Entity::find()
        .select_only()
        .column(issue::Column::Id)
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::Id.is_in(candidates))
        .into_tuple::<String>()
        .all(&state.db)
        .await?;
    let alive_set: HashSet<String> = alive.into_iter().collect();

    let mut wiped = 0usize;
    for id in &thumb_ids {
        if alive_set.contains(id) {
            continue;
        }
        thumbnails::wipe_issue_thumbs(data_path, id);
        wiped += 1;
    }
    for id in &variant_ids {
        if alive_set.contains(id) {
            continue;
        }
        thumbnails::wipe_issue_variant_covers(data_path, id);
        wiped += 1;
    }
    Ok(wiped)
}

/// Daily sweep entry point: the orphan pass, then a byte-budget sweep so
/// the `folio_thumbs_bytes` gauge reflects what the cleanup freed. Each
/// step is best-effort — one failing never skips the other.
pub async fn run_all(state: &AppState) {
    match run(state).await {
        Ok(n) if n > 0 => tracing::info!(wiped = n, "thumbnail orphan sweep"),
        Ok(_) => {}
        Err(e) => tracing::error!(error = %e, "thumbnail orphan sweep failed"),
    }
    if let Err(e) = run_budget_sweep(state).await {
        tracing::warn!(error = %e, "thumbnail budget sweep failed");
    }
}

/// Walk `thumbs/`, publish the `folio_thumbs_bytes` gauge, and enforce the
/// optional `cache.thumbs_budget_mb` byte budget (WP-3.8, audit OP-5). Runs
/// hourly from the scheduler, once at boot, and after the daily orphan
/// sweep; thumbnail writes also trigger a throttled one. The budget is read
/// from the live `Config` per run, so a settings change applies on the next
/// sweep without a restart.
pub async fn run_budget_sweep(state: &AppState) -> anyhow::Result<thumbnails::ThumbsBudgetSweep> {
    let cfg = state.cfg();
    let data_path = cfg.data_path.clone();
    let budget = cfg.thumbs_budget_bytes();
    let out =
        tokio::task::spawn_blocking(move || thumbnails::thumbs_budget_sweep(&data_path, budget))
            .await??;
    Ok(out)
}
