//! Daily orphan sweep for the thumbnail cache (M5).
//!
//! Walks `data_dir/thumbs/` and drops any artifact that doesn't correspond
//! to a still-present, still-active, not-yet-confirmed-removed issue row.
//! Catches:
//!   - issues confirmed-removed without their cleanup hook running
//!     (the 04:00 auto-confirm sweep is a bulk UPDATE that never wipes
//!     thumbs per row; the admin `confirm-removal` handlers do)
//!   - issues hard-deleted out from under the scanner
//!   - server crashes that interrupt a regenerate-while-rewrite
//!
//! Soft deletion is `removed_at`, not `issues.state` (the state only says
//! whether the archive opened). A soft-removed issue whose removal is not
//! confirmed yet keeps its artifacts: a returning file auto-restores the
//! row on the next scan and an admin can restore it from the Removed tab,
//! so the thumbs would only have to be regenerated.
//!
//! Cheap: one query per [`ALIVE_QUERY_CHUNK`] candidate ids + a dirent
//! scan (run on the blocking pool). Best-effort — disk errors are logged
//! and the loop continues; we'd rather skip one path than fail the whole
//! sweep.

use crate::library::thumbnails;
use crate::state::AppState;
use entity::issue;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use std::collections::HashSet;

/// Candidate ids bound per alive-set query. Postgres caps a statement at
/// 65,535 bind parameters, so one `IN (...)` over every thumbnail
/// directory failed once a library passed ~65k issues with thumbs. 5,000
/// keeps each statement small and the round-trip count low (20 queries
/// per 100k issues).
pub const ALIVE_QUERY_CHUNK: usize = 5_000;

/// Run one sweep. Returns the count of wiped issue artifacts.
pub async fn run(state: &AppState) -> anyhow::Result<usize> {
    run_with_chunk(state, ALIVE_QUERY_CHUNK).await
}

/// [`run`] with an explicit alive-query chunk size. Public so the
/// integration suite can cross a chunk boundary with a handful of rows;
/// production always goes through [`run`].
#[doc(hidden)]
pub async fn run_with_chunk(state: &AppState, chunk: usize) -> anyhow::Result<usize> {
    let data_path = state.cfg().data_path.clone();
    // Two parallel artifact trees keyed by issue id:
    //   - generated page/cover thumbs at `thumbs/{id}` + `thumbs/{id}.ext`
    //   - downloaded provider covers at `thumbs/issues/{id}/`
    // Sweep both; the second was previously read as a single phantom
    // `issues` issue and wiped wholesale on every run.
    let list_path = data_path.clone();
    let (thumb_ids, variant_ids): (HashSet<String>, HashSet<String>) =
        tokio::task::spawn_blocking(move || -> std::io::Result<_> {
            Ok((
                thumbnails::list_issues_on_disk(&list_path)?,
                thumbnails::list_variant_cover_issues_on_disk(&list_path)?,
            ))
        })
        .await??;
    if thumb_ids.is_empty() && variant_ids.is_empty() {
        return Ok(0);
    }

    // Find issue ids that should keep their artifacts: active rows whose
    // removal is not confirmed. Everything else (confirmed-removed, not
    // active, missing entirely) is eligible for cleanup. Soft-removed rows
    // keep `state = active` and get `removed_at` set, so filtering on the
    // state alone never reclaimed a removed issue's thumbs.
    let candidates: Vec<String> = thumb_ids.union(&variant_ids).cloned().collect();
    let mut alive_set: HashSet<String> = HashSet::new();
    for batch in candidates.chunks(chunk.max(1)) {
        let alive: Vec<String> = issue::Entity::find()
            .select_only()
            .column(issue::Column::Id)
            .filter(issue::Column::State.eq(issue::IssueState::Active))
            .filter(issue::Column::RemovalConfirmedAt.is_null())
            .filter(issue::Column::Id.is_in(batch.iter().cloned()))
            .into_tuple::<String>()
            .all(&state.db)
            .await?;
        alive_set.extend(alive);
    }

    // The wipes are `remove_dir_all` / `remove_file` calls; keep them off
    // the async workers too.
    let wiped = tokio::task::spawn_blocking(move || {
        let mut wiped = 0usize;
        for id in &thumb_ids {
            if alive_set.contains(id) {
                continue;
            }
            thumbnails::wipe_issue_thumbs(&data_path, id);
            wiped += 1;
        }
        for id in &variant_ids {
            if alive_set.contains(id) {
                continue;
            }
            thumbnails::wipe_issue_variant_covers(&data_path, id);
            wiped += 1;
        }
        wiped
    })
    .await?;
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

#[cfg(test)]
mod tests {
    use super::ALIVE_QUERY_CHUNK;

    /// Each chunk binds `ALIVE_QUERY_CHUNK` ids plus the state filter;
    /// it must stay far below Postgres' 65,535 bind-parameter cap.
    #[test]
    fn alive_chunk_is_far_below_postgres_bind_limit() {
        const PG_MAX_BINDS: usize = 65_535;
        const { assert!(ALIVE_QUERY_CHUNK > 0) };
        const { assert!(ALIVE_QUERY_CHUNK * 4 <= PG_MAX_BINDS) };
    }
}
