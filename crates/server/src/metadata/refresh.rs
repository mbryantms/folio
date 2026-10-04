//! Bulk metadata-refresh dispatch — fan out search jobs across a
//! library scope. Drives both the user-triggered
//! `POST /libraries/{slug}/metadata/refresh?scope=…` endpoint and
//! the weekly cron in [`crate::jobs::scheduler`].
//!
//! **Scope rules** (mirrors the M7 plan's "stale" definition):
//!
//! - `unmatched` — series with zero rows in `external_ids` for any
//!   provider source AND `metadata_sync_paused=false`.
//! - `stale`     — series that are unmatched OR whose
//!   `last_metadata_sync_at IS NULL OR <
//!   now() - INTERVAL 'stale_after_days days'`, paused excluded.
//! - `all`       — every active series in the library, paused
//!   excluded.
//! - `recent`    — Mylar-pattern "recently published" window:
//!   series whose `last_issue_added_at >= now() - window_days`. The
//!   weekly cron unions this with `stale` so newly-published series
//!   refresh weekly while older ones only refresh once they cross
//!   `stale_after_days`.
//!
//! Every scope excludes paused series (`series.metadata_sync_paused
//! = true`) and the chunking cap (200 per run) guards against
//! runaway provider-quota burn — once the cap is hit the remainder
//! waits for the next call.
//!
//! metadata-providers-1.0 M7.

use crate::state::AppState;
use entity::series;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, FromQueryResult, QueryFilter,
    Statement,
};
use uuid::Uuid;

/// Hard cap so a "scope=all" refresh can't fan out tens of thousands
/// of provider calls in one click. Operators can re-trigger to
/// process the rest; the cron's per-week cadence handles the rest
/// for unattended deploys.
pub const REFRESH_BATCH_CAP: usize = 200;

/// Which series belong to a given scope. Cheap query — index hits
/// on `series.library_id` + `series.metadata_sync_paused`. The
/// `unmatched` and `stale` shapes call into raw SQL because the
/// external_ids existence check is most concisely expressed as
/// `NOT EXISTS (...)` rather than via SeaORM relations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshScope {
    Unmatched,
    Stale,
    All,
    Recent,
}

impl RefreshScope {
    pub fn as_str(self) -> &'static str {
        match self {
            RefreshScope::Unmatched => "unmatched",
            RefreshScope::All => "all",
            RefreshScope::Stale => "stale",
            RefreshScope::Recent => "recent",
        }
    }
}

impl std::str::FromStr for RefreshScope {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "unmatched" => Ok(RefreshScope::Unmatched),
            "stale" => Ok(RefreshScope::Stale),
            "all" => Ok(RefreshScope::All),
            "recent" => Ok(RefreshScope::Recent),
            _ => Err(()),
        }
    }
}

#[derive(Debug, FromQueryResult)]
struct SeriesIdRow {
    id: Uuid,
}

/// Walk the eligible series for `scope` in `library_id`, capped at
/// [`REFRESH_BATCH_CAP`]. Order is `created_at ASC` so successive
/// calls re-process the earliest deferred rows first.
pub async fn eligible_series_for_scope<C: ConnectionTrait>(
    db: &C,
    library_id: Uuid,
    scope: RefreshScope,
    stale_after_days: u32,
    window_days: u32,
) -> Result<Vec<Uuid>, sea_orm::DbErr> {
    let limit = REFRESH_BATCH_CAP as i64;
    let stmt = match scope {
        RefreshScope::All => Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r"SELECT s.id FROM series s
              WHERE s.library_id = $1
                AND s.removed_at IS NULL
                AND s.metadata_sync_paused = false
              ORDER BY s.created_at ASC
              LIMIT $2",
            [library_id.into(), limit.into()],
        ),
        RefreshScope::Unmatched => Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r"SELECT s.id FROM series s
              WHERE s.library_id = $1
                AND s.removed_at IS NULL
                AND s.metadata_sync_paused = false
                AND NOT EXISTS (
                    SELECT 1 FROM external_ids x
                    WHERE x.entity_type = 'series'
                      AND x.entity_id = s.id::text
                )
              ORDER BY s.created_at ASC
              LIMIT $2",
            [library_id.into(), limit.into()],
        ),
        RefreshScope::Stale => Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            // Three OR'd staleness predicates:
            //   1. unmatched (no provider rows yet)
            //   2. never synced
            //   3. last sync older than stale_after_days
            // Paused series excluded regardless.
            r"SELECT s.id FROM series s
              WHERE s.library_id = $1
                AND s.removed_at IS NULL
                AND s.metadata_sync_paused = false
                AND (
                    NOT EXISTS (
                        SELECT 1 FROM external_ids x
                        WHERE x.entity_type = 'series'
                          AND x.entity_id = s.id::text
                    )
                    OR s.last_metadata_sync_at IS NULL
                    OR s.last_metadata_sync_at < NOW() - ($2 || ' days')::interval
                )
              ORDER BY s.created_at ASC
              LIMIT $3",
            [library_id.into(), stale_after_days.into(), limit.into()],
        ),
        RefreshScope::Recent => Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            // Series whose latest active issue's created_at falls
            // within the recent-window. Falls back to series.created_at
            // when no issues exist yet (fresh series tripped by the
            // window). `last_issue_added_at` isn't a stored column —
            // we compute it via correlated subquery (cheap; one
            // index hit per series on issues.series_id). Paused
            // excluded.
            r"SELECT s.id FROM series s
              WHERE s.library_id = $1
                AND s.removed_at IS NULL
                AND s.metadata_sync_paused = false
                AND COALESCE(
                    (
                        SELECT MAX(i.created_at)
                        FROM issues i
                        WHERE i.series_id = s.id
                          AND i.state = 'active'
                          AND i.removed_at IS NULL
                    ),
                    s.created_at
                ) >= NOW() - ($2 || ' days')::interval
              ORDER BY s.created_at ASC
              LIMIT $3",
            [library_id.into(), window_days.into(), limit.into()],
        ),
    };
    let rows = SeriesIdRow::find_by_statement(stmt).all(db).await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

/// Result of a single bulk-refresh fan-out — surfaces what was
/// enqueued so the caller can render a useful toast / log line.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct RefreshOutcome {
    /// Total eligible series for this scope after applying the
    /// [`REFRESH_BATCH_CAP`].
    pub series_eligible: usize,
    /// Per-series search jobs enqueued. Lower than `series_eligible`
    /// when a per-entity coalesce gate found an in-flight run.
    pub jobs_enqueued: usize,
    /// Per-series search jobs that hit the coalesce gate (the
    /// existing in-flight run will land first; no extra work).
    pub jobs_coalesced: usize,
    /// Per-series search jobs that failed to enqueue (queue push
    /// error, missing series row mid-flight, etc.).
    pub jobs_failed: usize,
}

/// Fan out a search job per eligible series, honoring the per-entity
/// Redis coalesce gate the search-job module already implements.
/// Bound by [`REFRESH_BATCH_CAP`].
pub async fn fan_out_scope(
    state: &AppState,
    library_id: Uuid,
    scope: RefreshScope,
    trigger_kind: &'static str,
    batch_id: Option<Uuid>,
) -> Result<RefreshOutcome, sea_orm::DbErr> {
    let cfg = state.cfg();
    let stale_after = cfg.metadata_stale_after_days;
    let window = cfg.metadata_weekly_refresh_window_days;
    let ids = eligible_series_for_scope(&state.db, library_id, scope, stale_after, window).await?;
    let series_eligible = ids.len();
    let mut jobs_enqueued = 0usize;
    let mut jobs_coalesced = 0usize;
    let mut jobs_failed = 0usize;
    for id in ids {
        match crate::jobs::metadata_search::enqueue_series_search(
            state,
            id,
            None,
            trigger_kind,
            batch_id,
        )
        .await
        {
            Ok(outcome) => {
                if outcome.coalesced {
                    jobs_coalesced += 1;
                } else {
                    jobs_enqueued += 1;
                }
            }
            Err(e) => {
                tracing::warn!(series_id = %id, error = %e, "refresh fan-out: enqueue failed");
                jobs_failed += 1;
            }
        }
    }
    Ok(RefreshOutcome {
        series_eligible,
        jobs_enqueued,
        jobs_coalesced,
        jobs_failed,
    })
}

/// Series-row liveness check used by [`fan_out_scope`]; exposed for
/// unit tests + the weekly cron's per-library walker. Surface is
/// intentionally narrow so the scope-walker doesn't need to import
/// the full series entity.
pub async fn series_exists(
    db: &sea_orm::DatabaseConnection,
    series_id: Uuid,
) -> Result<bool, sea_orm::DbErr> {
    Ok(series::Entity::find_by_id(series_id)
        .filter(series::Column::RemovedAt.is_null())
        .one(db)
        .await?
        .is_some())
}

// ───────── issue-level refresh (coverage tie-ins PR 4) ─────────
//
// The scopes above re-search **series**. With
// `metadata.issue_refresh_enabled` on, the library refresh endpoint and
// the weekly cron also re-fetch **issues** whose provider series is known
// (a series-level external id or a covering `series_provider_range` —
// accepted coverage, folded by `range_map::fold_targets`). Each provider
// answers by direct lookup only (the series' cached issue list + one
// cached detail fetch); a miss is recorded, never searched. At most
// `metadata.issue_refresh_per_provider_cap` issues per provider per run,
// stale first (never synced, then oldest `last_metadata_sync_at`). Runs
// land as one `issue_refresh` batch in Review; children carry the
// refresh's trigger kind, so the library's existing auto-apply rule
// (`SingleGoodMatch` + `metadata_auto_apply_strong_matches`) still applies
// — nothing else writes without review.

/// Default issues per provider per issue-level refresh run.
pub const ISSUE_REFRESH_DEFAULT_CAP: u32 = 200;

/// Upper bound accepted for `metadata.issue_refresh_per_provider_cap`.
pub const ISSUE_REFRESH_MAX_CAP: u32 = 1000;

/// An issue searched (any trigger) within this many days is skipped, so a
/// second click — or a refresh right after a batch — doesn't re-propose it.
pub const ISSUE_REFRESH_RECENT_RUN_DAYS: i64 = 7;

/// Rows read per page while filling the per-provider quotas.
const ISSUE_REFRESH_PAGE: i64 = 500;

/// Most candidate issue rows one run reads before giving up on filling
/// the quotas (a library whose stale issues are mostly uncovered).
const ISSUE_REFRESH_SCAN_LIMIT: i64 = 20_000;

/// `metadata_batch.scope` of an issue-level refresh.
pub const ISSUE_REFRESH_BATCH_SCOPE: &str = "issue_refresh";

/// Issues one provider was given in an issue-level refresh run.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct IssueRefreshProviderCount {
    pub source: String,
    pub issues: usize,
    /// The per-provider cap of this run.
    pub cap: u32,
}

/// What an issue-level refresh did.
#[derive(Debug, Clone, Default, serde::Serialize, utoipa::ToSchema)]
pub struct IssueRefreshOutcome {
    /// `false` when `metadata.issue_refresh_enabled` is off (nothing ran).
    pub enabled: bool,
    /// The Review batch the runs belong to; `None` when nothing was
    /// selected.
    pub batch_id: Option<Uuid>,
    /// Issues selected (each asks only the providers listed for it).
    pub issues_selected: usize,
    /// Issues per provider (≤ cap each).
    pub per_provider: Vec<IssueRefreshProviderCount>,
    pub jobs_enqueued: usize,
    pub jobs_coalesced: usize,
    pub jobs_failed: usize,
}

#[derive(Debug, FromQueryResult)]
struct IssueRefreshRow {
    id: String,
    series_id: Uuid,
    number_raw: String,
}

#[derive(Debug, FromQueryResult)]
struct RecentRunRow {
    scope_entity_id: Option<String>,
}

/// One issue the refresh picked, with the providers it asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRefreshPick {
    pub issue_id: String,
    pub sources: Vec<crate::metadata::identifier::Source>,
}

/// Pick the issues of `library_id` an issue-level refresh sends, and the
/// providers each asks: issues whose series has accepted coverage for a
/// provider that can list series issues, stale first, at most `cap` per
/// provider. Pure DB reads; no provider call.
pub async fn select_issue_refresh(
    db: &sea_orm::DatabaseConnection,
    library_id: Uuid,
    sources: &[crate::metadata::identifier::Source],
    cap: u32,
    stale_after_days: u32,
) -> Result<Vec<IssueRefreshPick>, sea_orm::DbErr> {
    use crate::metadata::identifier::Source;
    use std::collections::{HashMap, HashSet};
    if sources.is_empty() || cap == 0 {
        return Ok(Vec::new());
    }
    let source_strs: Vec<String> = sources.iter().map(|s| s.as_str().to_owned()).collect();
    let recent: HashSet<String> = RecentRunRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r"SELECT DISTINCT scope_entity_id FROM metadata_run
          WHERE scope = 'issue' AND library_id = $1
            AND started_at > NOW() - ($2 || ' days')::interval",
        [library_id.into(), ISSUE_REFRESH_RECENT_RUN_DAYS.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .filter_map(|r| r.scope_entity_id)
    .collect();

    let mut left: HashMap<Source, u32> = sources.iter().map(|s| (*s, cap)).collect();
    let mut targets_by_series: HashMap<
        Uuid,
        (
            Vec<entity::series_provider_range::Model>,
            Vec<entity::external_id::Model>,
        ),
    > = HashMap::new();
    let mut picks = Vec::new();
    let mut offset = 0i64;
    while offset < ISSUE_REFRESH_SCAN_LIMIT && left.values().any(|n| *n > 0) {
        let rows = IssueRefreshRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r"SELECT i.id, i.series_id, i.number_raw FROM issues i
              JOIN series s ON s.id = i.series_id
              WHERE i.library_id = $1
                AND i.state = 'active' AND i.removed_at IS NULL
                AND s.removed_at IS NULL AND s.metadata_sync_paused = false
                AND i.number_raw IS NOT NULL AND btrim(i.number_raw) <> ''
                AND (i.last_metadata_sync_at IS NULL
                     OR i.last_metadata_sync_at < NOW() - ($2 || ' days')::interval)
                AND (EXISTS (SELECT 1 FROM external_ids x
                             WHERE x.entity_type = 'series' AND x.entity_id = s.id::text
                               AND x.source = ANY($3))
                     OR EXISTS (SELECT 1 FROM series_provider_range r
                                WHERE r.series_id = s.id AND r.source = ANY($3)))
              ORDER BY i.last_metadata_sync_at ASC NULLS FIRST, i.created_at ASC, i.id ASC
              LIMIT $4 OFFSET $5",
            [
                library_id.into(),
                stale_after_days.into(),
                source_strs.clone().into(),
                ISSUE_REFRESH_PAGE.into(),
                offset.into(),
            ],
        ))
        .all(db)
        .await?;
        if rows.is_empty() {
            break;
        }
        offset += rows.len() as i64;
        for row in rows {
            if recent.contains(&row.id) {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(slot) =
                targets_by_series.entry(row.series_id)
            {
                let ranges = entity::series_provider_range::Entity::find()
                    .filter(entity::series_provider_range::Column::SeriesId.eq(row.series_id))
                    .all(db)
                    .await?;
                let ids = entity::external_id::Entity::find()
                    .filter(entity::external_id::Column::EntityType.eq("series"))
                    .filter(entity::external_id::Column::EntityId.eq(row.series_id.to_string()))
                    .all(db)
                    .await?;
                slot.insert((ranges, ids));
            }
            let (ranges, ids) = &targets_by_series[&row.series_id];
            let canonical = crate::metadata::matcher::canonical_issue_number(&row.number_raw);
            // An annual only uses a range target (the series-level id is the
            // parent run, which never carries the annual) — as the search.
            let annual =
                crate::metadata::title_norm::strip_annual_prefix(&row.number_raw).is_some();
            let mut picked: Vec<Source> = Vec::new();
            for t in crate::metadata::range_map::fold_targets(ranges, ids, &canonical) {
                if (annual && !t.via_range) || picked.contains(&t.source) {
                    continue;
                }
                if let Some(n) = left.get_mut(&t.source)
                    && *n > 0
                {
                    *n -= 1;
                    picked.push(t.source);
                }
            }
            if !picked.is_empty() {
                // Stable provider order (ComicVine, Metron, GCD, …).
                picked.sort_by_key(|s| sources.iter().position(|x| x == s));
                picks.push(IssueRefreshPick {
                    issue_id: row.id,
                    sources: picked,
                });
            }
        }
    }
    Ok(picks)
}

/// Run an issue-level refresh for `library_id` when
/// `metadata.issue_refresh_enabled` is on: select ([`select_issue_refresh`]),
/// open an `issue_refresh` Review batch and enqueue one direct-lookup-only
/// search per issue. Off ⇒ returns `enabled: false` without touching the
/// database or a provider.
pub async fn fan_out_issue_refresh(
    state: &AppState,
    library_id: Uuid,
    trigger_kind: &'static str,
    created_by: Option<Uuid>,
) -> Result<IssueRefreshOutcome, sea_orm::DbErr> {
    let cfg = state.cfg();
    if !cfg.metadata_issue_refresh_enabled {
        return Ok(IssueRefreshOutcome::default());
    }
    let cap = cfg
        .metadata_issue_refresh_per_provider_cap
        .clamp(1, ISSUE_REFRESH_MAX_CAP);
    // Providers that can answer by direct lookup (list a series' issues).
    let mut sources: Vec<_> =
        crate::metadata::orchestrator::build_providers(&cfg, state.jobs.redis.clone())
            .iter()
            .filter(|p| p.lists_series_issues())
            .map(|p| p.id())
            .collect();
    // Display order: ComicVine, Metron, GCD, then anything else.
    let order = crate::metadata::coverage::COVERAGE_SOURCES;
    sources.sort_by_key(|s| order.iter().position(|x| x == s).unwrap_or(usize::MAX));
    let mut out = IssueRefreshOutcome {
        enabled: true,
        ..Default::default()
    };
    let picks = select_issue_refresh(
        &state.db,
        library_id,
        &sources,
        cap,
        cfg.metadata_stale_after_days,
    )
    .await?;
    out.per_provider = sources
        .iter()
        .map(|s| IssueRefreshProviderCount {
            source: s.as_str().to_owned(),
            issues: picks.iter().filter(|p| p.sources.contains(s)).count(),
            cap,
        })
        .collect();
    out.issues_selected = picks.len();
    if picks.is_empty() {
        return Ok(out);
    }
    let batch_id = crate::api::metadata_search::insert_metadata_batch_with(
        &state.db,
        ISSUE_REFRESH_BATCH_SCOPE,
        Some(library_id),
        created_by,
        trigger_kind,
    )
    .await?;
    out.batch_id = Some(batch_id);
    for pick in picks {
        match crate::jobs::metadata_search::enqueue_issue_search_with(
            state,
            &pick.issue_id,
            created_by,
            trigger_kind,
            Some(batch_id),
            Some(pick.sources),
        )
        .await
        {
            Ok(o) if o.coalesced => out.jobs_coalesced += 1,
            Ok(_) => out.jobs_enqueued += 1,
            Err(e) => {
                tracing::warn!(issue_id = %pick.issue_id, error = %e, "issue refresh: enqueue failed");
                out.jobs_failed += 1;
            }
        }
    }
    crate::api::metadata_search::set_batch_items_total(
        &state.db,
        batch_id,
        i32::try_from(out.jobs_enqueued).unwrap_or(i32::MAX),
    )
    .await;
    tracing::info!(
        library_id = %library_id,
        batch_id = %batch_id,
        selected = out.issues_selected,
        enqueued = out.jobs_enqueued,
        coalesced = out.jobs_coalesced,
        failed = out.jobs_failed,
        "metadata issue refresh: fan-out",
    );
    Ok(out)
}
