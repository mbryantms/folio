//! Cron-driven scan scheduling (spec §3 trigger row "Scheduled scan").
//!
//! Library Scanner v1, Milestone 9.
//!
//! Wraps `tokio_cron_scheduler` with the integration this codebase needs:
//!   - One job per library with a non-null `scan_schedule_cron`
//!   - Each job enqueues a full scan via `JobRuntime::coalesce_scan`
//!   - Reload requires a server restart; live-reload-on-config-change is
//!     deferred (the `PATCH /libraries/{id}` handler logs a notice when
//!     scan_schedule_cron is touched so admins know to restart)
//!
//! `tokio_cron_scheduler` accepts both 5-field and 6-field cron expressions;
//! mirroring spec §11's defaults of `0 */6 * * *` (every 6 hours).
//!
//! On any scheduler error during boot we log and proceed without the
//! scheduler — scheduled scans are a convenience, not a release-gate.

use crate::state::AppState;
use entity::library;
use sea_orm::EntityTrait;
use tokio_cron_scheduler::{Job, JobScheduler};

pub async fn start(state: AppState) -> anyhow::Result<JobScheduler> {
    let scheduler = JobScheduler::new().await?;
    scheduler.start().await?;
    register_library_scans(&scheduler, &state).await;
    register_reconcile_sweep(&scheduler, &state).await;
    register_scan_runs_prune(&scheduler, &state).await;
    register_thumbnail_orphan_sweep(&scheduler, &state).await;
    register_thumbnail_catchup_sweep(&scheduler, &state).await;
    register_close_dangling_sessions(&scheduler, &state).await;
    register_cbl_refresh_sweep(&scheduler, &state).await;
    register_prune_auth_sessions(&scheduler, &state).await;
    Ok(scheduler)
}

/// M3 (audit S-11): daily prune of expired `auth_sessions` rows. Refresh
/// tokens past their `expires_at` are already rejected by the handler;
/// keeping them costs nothing security-wise but bloats the table. Runs
/// at 03:00 UTC daily, well outside any reasonable user-traffic peak.
async fn register_prune_auth_sessions(scheduler: &JobScheduler, state: &AppState) {
    let state = state.clone();
    let job_result = Job::new_async("0 0 3 * * *", move |_uuid, _l| {
        let state = state.clone();
        Box::pin(async move {
            match crate::jobs::prune_auth_sessions::run(&state.db).await {
                Ok(n) => {
                    if n > 0 {
                        tracing::info!(deleted = n, "auth_sessions pruned");
                    }
                }
                Err(e) => tracing::error!(error = %e, "auth_sessions prune failed"),
            }
        })
    });
    match job_result {
        Ok(job) => {
            if let Err(e) = scheduler.add(job).await {
                tracing::error!(error = %e, "scheduler: add prune_auth_sessions failed");
            } else {
                tracing::info!("prune_auth_sessions registered (03:00 UTC daily)");
            }
        }
        Err(e) => tracing::error!(error = %e, "scheduler: build prune_auth_sessions failed"),
    }
}

/// Every hour, walk `cbl_lists` whose `refresh_schedule` is non-NULL
/// and `last_refreshed_at` is older than the configured cadence. v1
/// supports a small set of canonical schedules (`@hourly`, `@daily`,
/// `@weekly`, `@monthly`) plus the literal `manual` (which refuses to
/// auto-fire). Real cron parsing is deferred — the API caps the schedule
/// to these tokens.
async fn register_cbl_refresh_sweep(
    scheduler: &tokio_cron_scheduler::JobScheduler,
    state: &AppState,
) {
    let state = state.clone();
    let job_result = tokio_cron_scheduler::Job::new_async("0 0 * * * *", move |_uuid, _l| {
        let state = state.clone();
        Box::pin(async move {
            let due = match find_due_cbl_lists(&state.db).await {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(error = %e, "cbl_refresh sweep: query failed");
                    return;
                }
            };
            for list_id in due {
                match crate::cbl::refresh::refresh(
                    &state.db,
                    list_id,
                    crate::cbl::import::RefreshTrigger::Scheduled,
                    false,
                )
                .await
                {
                    Ok(s) => {
                        if s.upstream_changed {
                            tracing::info!(
                                list_id = %list_id,
                                added = s.added,
                                removed = s.removed,
                                rematched = s.rematched,
                                "cbl_refresh applied diff",
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(list_id = %list_id, error = %e, "cbl_refresh failed");
                    }
                }
            }
        })
    });
    match job_result {
        Ok(job) => {
            if let Err(e) = scheduler.add(job).await {
                tracing::error!(error = %e, "scheduler: add cbl_refresh_sweep failed");
            } else {
                tracing::info!("cbl_refresh sweep registered (hourly)");
            }
        }
        Err(e) => tracing::error!(error = %e, "scheduler: build cbl_refresh_sweep failed"),
    }
}

async fn find_due_cbl_lists(
    db: &sea_orm::DatabaseConnection,
) -> Result<Vec<uuid::Uuid>, sea_orm::DbErr> {
    use chrono::Utc;
    use entity::cbl_list;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let now = Utc::now();
    let rows = cbl_list::Entity::find()
        .filter(cbl_list::Column::RefreshSchedule.is_not_null())
        .all(db)
        .await?;
    let mut due = Vec::new();
    for list in rows {
        let Some(schedule) = list.refresh_schedule.as_deref() else {
            continue;
        };
        let cadence_hours: i64 = match schedule.trim() {
            "@hourly" => 1,
            // Cron-shaped strings or named tokens; fall back to weekly
            // for anything we don't recognize so a typo doesn't disable
            // refresh entirely.
            "@daily" => 24,
            "@weekly" | "0 0 * * 0" => 24 * 7,
            "@monthly" => 24 * 30,
            "manual" => continue,
            _ => 24 * 7,
        };
        let last = list.last_refreshed_at.unwrap_or(list.imported_at);
        let age = now - last.with_timezone(&Utc);
        if age >= chrono::Duration::hours(cadence_hours) {
            due.push(list.id);
        }
    }
    Ok(due)
}

/// M6a: close reading sessions whose last heartbeat is > 5 min stale. Runs
/// every 2 minutes — cheap query gated on the partial index
/// `reading_sessions_dangling_idx`.
async fn register_close_dangling_sessions(scheduler: &JobScheduler, state: &AppState) {
    let state = state.clone();
    let job_result = Job::new_async("0 */2 * * * *", move |_uuid, _l| {
        let state = state.clone();
        Box::pin(async move {
            match crate::jobs::close_dangling_sessions::run(&state.db).await {
                Ok(n) => {
                    if n > 0 {
                        tracing::info!(closed = n, "reading_sessions sweep");
                    }
                }
                Err(e) => tracing::error!(error = %e, "reading_sessions sweep failed"),
            }
        })
    });
    match job_result {
        Ok(job) => {
            if let Err(e) = scheduler.add(job).await {
                tracing::error!(error = %e, "scheduler: add close_dangling_sessions failed");
            } else {
                tracing::info!("close_dangling_sessions registered (every 2 minutes)");
            }
        }
        Err(e) => tracing::error!(error = %e, "scheduler: build close_dangling_sessions failed"),
    }
}

async fn register_library_scans(scheduler: &JobScheduler, state: &AppState) {
    let libs = match library::Entity::find().all(&state.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "scheduler: failed to load libraries");
            return;
        }
    };

    for lib in libs {
        let Some(cron_expr) = lib.scan_schedule_cron.clone() else {
            continue;
        };
        let trimmed = cron_expr.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }

        register_one_library_scan(scheduler, state, lib.id, &trimmed).await;
    }
}

pub async fn reload_library_scan(state: &AppState, lib: &library::Model) {
    let Some(scheduler) = state.scheduler.lock().await.clone() else {
        tracing::debug!(library_id = %lib.id, "scheduler reload skipped; scheduler unavailable");
        return;
    };

    if let Some(old_id) = state.library_scan_job_ids.lock().await.remove(&lib.id)
        && let Err(e) = scheduler.remove(&old_id).await
    {
        tracing::warn!(
            library_id = %lib.id,
            job_id = %old_id,
            error = %e,
            "scheduler: remove old library scan job failed",
        );
    }

    let Some(cron_expr) = lib.scan_schedule_cron.as_deref() else {
        tracing::info!(library_id = %lib.id, "scheduled scan removed");
        return;
    };
    let trimmed = cron_expr.trim();
    if trimmed.is_empty() {
        tracing::info!(library_id = %lib.id, "scheduled scan removed");
        return;
    }
    register_one_library_scan(&scheduler, state, lib.id, trimmed).await;
}

async fn register_one_library_scan(
    scheduler: &JobScheduler,
    state: &AppState,
    lib_id: uuid::Uuid,
    cron_expr: &str,
) {
    let trimmed = cron_expr.trim().to_owned();
    let state_for_job = state.clone();
    let job_result = Job::new_async(trimmed.as_str(), move |_uuid, _l| {
        let state = state_for_job.clone();
        Box::pin(async move {
            match state.jobs.coalesce_scan(lib_id, false).await {
                Ok(outcome) => tracing::info!(
                    library_id = %lib_id,
                    scan_id = %outcome.scan_id(),
                    coalesced = outcome.was_coalesced(),
                    "scheduled scan enqueued",
                ),
                Err(e) => tracing::error!(
                    library_id = %lib_id,
                    error = %e,
                    "scheduled scan enqueue failed",
                ),
            }
        })
    });
    match job_result {
        Ok(job) => {
            let job_id = job.guid();
            if let Err(e) = scheduler.add(job).await {
                tracing::error!(
                    library_id = %lib_id,
                    cron = %trimmed,
                    error = %e,
                    "scheduler: add job failed",
                );
            } else {
                state
                    .library_scan_job_ids
                    .lock()
                    .await
                    .insert(lib_id, job_id);
                tracing::info!(library_id = %lib_id, cron = %trimmed, "scheduled scan registered");
            }
        }
        Err(e) => tracing::error!(
            library_id = %lib_id,
            cron = %trimmed,
            error = %e,
            "scheduler: invalid cron expression for library",
        ),
    }
}

/// Daily prune of `scan_runs` history — keep last 50 per library
/// (spec §8.2). Runs at 03:00 UTC.
async fn register_scan_runs_prune(scheduler: &JobScheduler, state: &AppState) {
    let state = state.clone();
    let job_result = Job::new_async("0 0 3 * * *", move |_uuid, _l| {
        let state = state.clone();
        Box::pin(async move {
            match crate::api::scan_runs::prune(&state.db, 50).await {
                Ok(n) => {
                    if n > 0 {
                        tracing::info!(deleted = n, "scan_runs prune");
                    }
                }
                Err(e) => tracing::error!(error = %e, "scan_runs prune failed"),
            }
        })
    });
    match job_result {
        Ok(job) => {
            if let Err(e) = scheduler.add(job).await {
                tracing::error!(error = %e, "scheduler: add scan_runs_prune failed");
            } else {
                tracing::info!("scan_runs prune registered (daily at 03:00 UTC, keep=50)");
            }
        }
        Err(e) => tracing::error!(error = %e, "scheduler: build scan_runs_prune failed"),
    }
}

/// Daily catchup sweep for the thumbnail pipeline (M6). Picks up:
///   - Issues whose `thumbnails_generated_at IS NULL` and were missed by
///     the post-scan enqueue (e.g. server crashed between scan finish
///     and enqueue).
///   - Issues whose stamped `thumbnail_version < CURRENT` after a code-side
///     bump (filter / quality / format change).
///
/// Runs at 02:00 UTC so it's clear of the scan / reconcile / orphan sweeps.
async fn register_thumbnail_catchup_sweep(scheduler: &JobScheduler, state: &AppState) {
    let state = state.clone();
    let job_result = Job::new_async("0 0 2 * * *", move |_uuid, _l| {
        let state = state.clone();
        Box::pin(async move {
            let n = crate::jobs::post_scan::enqueue_pending_all_libraries(&state).await;
            if n > 0 {
                tracing::info!(enqueued = n, "thumbnail catchup sweep");
            }
        })
    });
    match job_result {
        Ok(job) => {
            if let Err(e) = scheduler.add(job).await {
                tracing::error!(error = %e, "scheduler: add thumbnail_catchup_sweep failed");
            } else {
                tracing::info!("thumbnail catchup sweep registered (daily at 02:00 UTC)");
            }
        }
        Err(e) => tracing::error!(error = %e, "scheduler: build thumbnail_catchup_sweep failed"),
    }
}

/// Daily orphan sweep for the thumbnail cache (M5). Runs at 04:30 UTC,
/// 30 min after the auto-confirm sweep so confirmed-removed issues land
/// before we scan for orphans. Cheap dirent walk + one query per run.
async fn register_thumbnail_orphan_sweep(scheduler: &JobScheduler, state: &AppState) {
    let state = state.clone();
    let job_result = Job::new_async("0 30 4 * * *", move |_uuid, _l| {
        let state = state.clone();
        Box::pin(async move {
            match crate::jobs::orphan_sweep::run(&state).await {
                Ok(n) => {
                    if n > 0 {
                        tracing::info!(wiped = n, "thumbnail orphan sweep");
                    }
                }
                Err(e) => tracing::error!(error = %e, "thumbnail orphan sweep failed"),
            }
        })
    });
    match job_result {
        Ok(job) => {
            if let Err(e) = scheduler.add(job).await {
                tracing::error!(error = %e, "scheduler: add thumbnail_orphan_sweep failed");
            } else {
                tracing::info!("thumbnail orphan sweep registered (daily at 04:30 UTC)");
            }
        }
        Err(e) => tracing::error!(error = %e, "scheduler: build thumbnail_orphan_sweep failed"),
    }
}

/// Daily auto-confirm sweep (spec §4.7). Runs at 04:00 UTC by default to
/// avoid overlapping with typical 6-hour scan windows.
async fn register_reconcile_sweep(scheduler: &JobScheduler, state: &AppState) {
    let state = state.clone();
    let job_result = Job::new_async("0 0 4 * * *", move |_uuid, _l| {
        let state = state.clone();
        Box::pin(async move {
            match crate::library::reconcile::auto_confirm_sweep(&state.db).await {
                Ok(n) => {
                    if n > 0 {
                        tracing::info!(confirmed = n, "reconcile sweep: confirmed removals");
                    }
                }
                Err(e) => tracing::error!(error = %e, "reconcile sweep failed"),
            }
        })
    });
    match job_result {
        Ok(job) => {
            if let Err(e) = scheduler.add(job).await {
                tracing::error!(error = %e, "scheduler: add reconcile_sweep failed");
            } else {
                tracing::info!("reconcile sweep registered (daily at 04:00 UTC)");
            }
        }
        Err(e) => tracing::error!(error = %e, "scheduler: build reconcile_sweep failed"),
    }
}
