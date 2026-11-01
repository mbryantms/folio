//! Scan-time provider search for new series (audit 2026-11 N-M9 / DI-17).
//!
//! When a library has `auto_search_new_series = true`, the scan's
//! [`super::finalize_run`] hands the ids of the series it **created**
//! (identity resolution returned `Created`; a re-homed or renamed series
//! keeps its id and is not in the list) to [`enqueue_for_created_series`].
//! Each eligible one gets a `SearchSeriesJob` with `trigger_kind = scanner`,
//! queued through [`crate::jobs::metadata_search::enqueue_series_search`] —
//! the same run row + per-series Redis in-flight slot + queue the manual
//! "Match series" button and the weekly refresh use, so provider rate
//! limits, quota parking (`awaiting_quota`) and resume on the same run all
//! apply unchanged.
//!
//! **Eligible** means: not removed, not paused, at least one active issue
//! (a folder whose archives all failed to ingest has nothing to match on),
//! no series-level `external_ids` row yet (a folder tag, ComicInfo /
//! MetronInfo id or `series.json` already identified it — the same rule as
//! the `unmatched` refresh scope), and no `metadata_run` for it still
//! `queued`, `searching` or `awaiting_quota`. The in-flight Redis slot in
//! `enqueue_series_search` covers the race with a click that lands in
//! between (the speculative run row is dropped, reported as coalesced).
//!
//! **Throttle: a per-library rolling budget.** At most
//! [`SCAN_AUTO_SEARCH_CAP`] scanner-triggered searches per library per
//! [`SCAN_AUTO_SEARCH_WINDOW_HOURS`], counted from the `metadata_run` rows
//! themselves, so a first import split across many file-watcher scans is
//! bounded the same as one big scan. Series over the budget are queued in
//! creation order up to it; the rest stay unmatched. They are never
//! re-queued by a later scan (it only looks at the series *it* created):
//! a library refresh with `scope=unmatched`, or the weekly refresh when
//! `metadata.weekly_refresh_enabled` is on, picks them up — both walk
//! unmatched series oldest first, 200 per call.
//!
//! The flag is re-read when the scan finalizes, so turning it off during a
//! long scan stops that scan from queuing anything.
//!
//! A search writes nothing by itself. With
//! `metadata_auto_apply_strong_matches` also on, a strong single match
//! auto-applies (scanner is a non-manual trigger), and on a writeback
//! library that apply rewrites the archive's sidecars.

use crate::metadata::orchestrator::{self, status, trigger_kind};
use crate::state::AppState;
use entity::{external_id, issue, library, metadata_run, series};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QuerySelect};
use std::collections::HashSet;
use uuid::Uuid;

/// Most scanner-triggered series searches one library queues per
/// [`SCAN_AUTO_SEARCH_WINDOW_HOURS`] (see the module docs).
pub const SCAN_AUTO_SEARCH_CAP: usize = 50;

/// Length of the rolling window [`SCAN_AUTO_SEARCH_CAP`] applies to.
pub const SCAN_AUTO_SEARCH_WINDOW_HOURS: i64 = 24;

/// Ids per eligibility query.
const ELIGIBILITY_CHUNK: usize = 500;

/// What [`enqueue_for_created_series`] did. Logged by the scanner; returned
/// for tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutoSearchOutcome {
    /// Fresh runs queued (`trigger_kind = scanner`).
    pub enqueued: usize,
    /// Lost the in-flight slot race to a run started meanwhile.
    pub coalesced: usize,
    /// Not eligible: already matched, no active issue, removed / paused,
    /// or a search for it is already queued / running / parked.
    pub skipped: usize,
    /// Over the library's rolling budget; left for an unmatched refresh.
    pub deferred: usize,
    /// Enqueue or eligibility-query errors.
    pub failed: usize,
}

/// Queue a provider search for each eligible series a scan created, when
/// the library has `auto_search_new_series` on. Best-effort: errors are
/// logged, never fail the scan.
pub async fn enqueue_for_created_series(
    state: &AppState,
    library_id: Uuid,
    created: &[Uuid],
) -> AutoSearchOutcome {
    enqueue_for_created_series_capped(state, library_id, created, SCAN_AUTO_SEARCH_CAP).await
}

/// [`enqueue_for_created_series`] with an explicit budget (tests).
pub async fn enqueue_for_created_series_capped(
    state: &AppState,
    library_id: Uuid,
    created: &[Uuid],
    cap: usize,
) -> AutoSearchOutcome {
    let mut out = AutoSearchOutcome::default();
    if created.is_empty() {
        return out;
    }
    // Live value, not the row the scan loaded when it started.
    let enabled: Option<bool> = match library::Entity::find_by_id(library_id)
        .select_only()
        .column(library::Column::AutoSearchNewSeries)
        .into_tuple()
        .one(&state.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(%library_id, error = %e, "scan auto-search: library lookup failed");
            out.failed = created.len();
            return out;
        }
    };
    if enabled != Some(true) {
        return out;
    }
    if orchestrator::build_providers(&state.cfg(), state.jobs.redis.clone()).is_empty() {
        tracing::info!(
            %library_id,
            created = created.len(),
            "scan auto-search: no metadata providers configured; nothing queued",
        );
        return out;
    }
    let budget = match recent_scanner_runs(state, library_id).await {
        Ok(n) => cap.saturating_sub(n),
        Err(e) => {
            tracing::warn!(%library_id, error = %e, "scan auto-search: budget query failed");
            out.failed = created.len();
            return out;
        }
    };

    // Creation order, no repeats.
    let mut seen = HashSet::new();
    let ordered: Vec<Uuid> = created
        .iter()
        .copied()
        .filter(|id| seen.insert(*id))
        .collect();

    // Enqueue attempts against the budget. Eligibility-query failures are
    // counted as `failed` but spend no budget (nothing was attempted).
    let mut attempts = 0usize;
    // Eligibility is read a chunk at a time so a first import's thousands
    // of ids never become one oversized IN list; once the budget is spent
    // the rest are counted as deferred without another query.
    for chunk in ordered.chunks(ELIGIBILITY_CHUNK) {
        if attempts >= budget {
            out.deferred += chunk.len();
            continue;
        }
        let eligible = match eligible(state, chunk).await {
            Ok(set) => set,
            Err(e) => {
                tracing::warn!(%library_id, error = %e, "scan auto-search: eligibility query failed");
                out.failed += chunk.len();
                continue;
            }
        };
        for &id in chunk {
            if !eligible.contains(&id) {
                out.skipped += 1;
                continue;
            }
            if attempts >= budget {
                out.deferred += 1;
                continue;
            }
            attempts += 1;
            match crate::jobs::metadata_search::enqueue_series_search(
                state,
                id,
                None,
                trigger_kind::SCANNER,
                None,
            )
            .await
            {
                Ok(o) if o.coalesced => out.coalesced += 1,
                Ok(_) => out.enqueued += 1,
                Err(e) => {
                    tracing::warn!(series_id = %id, error = %e, "scan auto-search: enqueue failed");
                    out.failed += 1;
                }
            }
        }
    }
    tracing::info!(
        %library_id,
        enqueued = out.enqueued,
        coalesced = out.coalesced,
        skipped = out.skipped,
        deferred = out.deferred,
        failed = out.failed,
        budget,
        "scan auto-search: provider searches queued for new series",
    );
    out
}

/// Scanner-triggered series runs this library started inside the rolling
/// window.
async fn recent_scanner_runs(state: &AppState, library_id: Uuid) -> Result<usize, sea_orm::DbErr> {
    let since = (chrono::Utc::now() - chrono::Duration::hours(SCAN_AUTO_SEARCH_WINDOW_HOURS))
        .fixed_offset();
    let n = metadata_run::Entity::find()
        .filter(metadata_run::Column::LibraryId.eq(library_id))
        .filter(metadata_run::Column::TriggerKind.eq(trigger_kind::SCANNER))
        .filter(metadata_run::Column::Scope.eq(orchestrator::scope::SERIES))
        .filter(metadata_run::Column::StartedAt.gte(since))
        .count(&state.db)
        .await?;
    Ok(usize::try_from(n).unwrap_or(usize::MAX))
}

/// The subset of `ids` that is eligible for a scan-triggered search (see
/// the module docs).
async fn eligible(state: &AppState, ids: &[Uuid]) -> Result<HashSet<Uuid>, sea_orm::DbErr> {
    let live: Vec<Uuid> = series::Entity::find()
        .select_only()
        .column(series::Column::Id)
        .filter(series::Column::Id.is_in(ids.iter().copied()))
        .filter(series::Column::RemovedAt.is_null())
        .filter(series::Column::MetadataSyncPaused.eq(false))
        .into_tuple()
        .all(&state.db)
        .await?;
    if live.is_empty() {
        return Ok(HashSet::new());
    }
    let live_text: Vec<String> = live.iter().map(Uuid::to_string).collect();

    let with_issues: HashSet<Uuid> = issue::Entity::find()
        .select_only()
        .column(issue::Column::SeriesId)
        .distinct()
        .filter(issue::Column::SeriesId.is_in(live.iter().copied()))
        .filter(issue::Column::State.eq(issue::IssueState::Active))
        .filter(issue::Column::RemovedAt.is_null())
        .into_tuple::<Uuid>()
        .all(&state.db)
        .await?
        .into_iter()
        .collect();

    let matched: HashSet<String> = external_id::Entity::find()
        .select_only()
        .column(external_id::Column::EntityId)
        .distinct()
        .filter(external_id::Column::EntityType.eq("series"))
        .filter(external_id::Column::EntityId.is_in(live_text.iter().cloned()))
        .into_tuple::<String>()
        .all(&state.db)
        .await?
        .into_iter()
        .collect();

    let active: HashSet<String> = metadata_run::Entity::find()
        .select_only()
        .column(metadata_run::Column::ScopeEntityId)
        .filter(metadata_run::Column::Scope.eq(orchestrator::scope::SERIES))
        .filter(metadata_run::Column::ScopeEntityId.is_in(live_text))
        .filter(metadata_run::Column::Status.is_in([
            status::QUEUED,
            status::SEARCHING,
            status::AWAITING_QUOTA,
        ]))
        .into_tuple::<Option<String>>()
        .all(&state.db)
        .await?
        .into_iter()
        .flatten()
        .collect();

    Ok(live
        .into_iter()
        .filter(|id| {
            let key = id.to_string();
            with_issues.contains(id) && !matched.contains(&key) && !active.contains(&key)
        })
        .collect())
}
