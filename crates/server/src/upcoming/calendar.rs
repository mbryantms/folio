//! The release calendar: Metron's issues by store date, kept in the
//! `release_calendar_*` tables.
//!
//! Two pieces of work, both driven from [`crate::jobs::release_calendar`]:
//!
//! - [`list_pass`] walks the window week by week, one Metron request per
//!   week (a second only if a week overflows a page), and upserts what
//!   it gets. It saves its place after every week, so a pass stopped by
//!   the budget gate or by a provider `429` picks up at the same week.
//!   Only a pass that listed every week completely may delete rows.
//! - [`enrich`] fetches the series detail once per series, a few per
//!   tick, for the publisher and the ComicVine volume id the list
//!   endpoint does not carry.
//!
//! **Budget.** Nothing reserves Metron headroom for interactive
//! matching, so this background caller holds itself back: before every
//! request it reads the shared buckets and stops when fewer than
//! [`MIN_BUCKET_RESERVE`] of the per-minute or [`DAY_BUCKET_RESERVE`] of
//! the per-day requests remain. A stopped run is not a failure.

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use entity::{release_calendar_issue, release_calendar_series, release_calendar_state};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr,
    EntityTrait, FromQueryResult, IntoActiveModel, PaginatorTrait, QueryFilter, Set, Statement,
};
use std::collections::{BTreeMap, HashSet};

use super::{PRUNE_AFTER_DAYS, Window, window};
use crate::metadata::identifier::Source;
use crate::metadata::metron::{CalendarIssue, MetronClient};
use crate::metadata::provider::{GenericMetadata, MetadataProvider, ProviderError};
use crate::metadata::rate_limit;
use crate::state::AppState;

/// Per-minute requests left untouched for interactive callers (of 20).
pub const MIN_BUCKET_RESERVE: u32 = 8;
/// Per-day requests left untouched for interactive callers (of 5,000).
pub const DAY_BUCKET_RESERVE: u32 = 1000;
/// Series details fetched per enrich tick.
pub const ENRICH_PER_TICK: u64 = 10;
/// A series still without a ComicVine id is asked about again after
/// this many days (a new #1 often gains the id after it is announced).
pub const RECHECK_MISSING_CV_DAYS: i64 = 3;
/// A list pass is due again this long after the last complete one.
pub const PASS_DUE_AFTER_HOURS: i64 = 26;
/// After a failed pass, the next attempt waits this long.
pub const FAILURE_BACKOFF_MINUTES: i64 = 30;
/// A stopped pass older than this starts over instead of resuming: its
/// earlier weeks are too old to count as "seen in this pass".
pub const RESUME_WITHIN_HOURS: i64 = 24;
/// Pages asked for one week before giving up on it (1,000 issues).
pub const MAX_PAGES_PER_WEEK: u32 = 10;

/// Gauge: issues the calendar holds, set when a list pass finishes.
pub const ISSUES_GAUGE: &str = "folio_release_calendar_issues";
/// Gauge: Unix time of the last complete list pass.
pub const LAST_SYNC_GAUGE: &str = "folio_release_calendar_last_sync_timestamp_seconds";

/// Why a run stopped before it was done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pause {
    /// The local budget gate: too few requests left for interactive use.
    BudgetReserve,
    /// The provider (or the shared bucket) refused the request.
    Quota,
}

/// What one [`list_pass`] call did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PassOutcome {
    /// `upcoming.enabled` is off, or Metron is off or has no credentials.
    Disabled,
    /// The last pass failed and its backoff has not run out.
    BackingOff { until: DateTime<FixedOffset> },
    /// Stopped early; the next call resumes at `next_week`.
    Paused {
        reason: Pause,
        next_week: NaiveDate,
        requests: u32,
    },
    /// Every week of the window was listed.
    Finished {
        weeks: u32,
        issues: u64,
        removed: u64,
        requests: u32,
    },
    /// A request failed for a reason other than quota; backoff is set.
    Failed { error: String },
}

/// What one [`enrich`] call did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnrichOutcome {
    /// Series whose detail row was stamped this tick.
    pub fetched: u32,
    /// The tick stopped on the budget gate or a quota refusal.
    pub paused: bool,
}

/// The Metron client, when the calendar may use it.
fn metron(state: &AppState) -> Option<MetronClient> {
    let cfg = state.cfg();
    if !cfg.upcoming_enabled || !cfg.metron_enabled {
        return None;
    }
    MetronClient::from_config(&cfg, state.jobs.redis.clone())
}

/// True when the calendar is switched on and Metron can be asked.
pub fn ready(state: &AppState) -> bool {
    metron(state).is_some()
}

/// True while the shared Metron buckets hold more than the reserve. A
/// Redis error reads as "no budget": the caller stops and tries later.
async fn budget_ok(state: &AppState) -> bool {
    let mut conn = state.jobs.redis.clone();
    let minute = rate_limit::snapshot(&mut conn, &rate_limit::METRON_MIN).await;
    let day = rate_limit::snapshot(&mut conn, &rate_limit::METRON_DAY).await;
    match (minute, day) {
        (Ok((minute_left, _)), Ok((day_left, _))) => {
            minute_left > MIN_BUCKET_RESERVE && day_left > DAY_BUCKET_RESERVE
        }
        (Err(e), _) | (_, Err(e)) => {
            tracing::warn!(error = %e, "release calendar: budget snapshot failed");
            false
        }
    }
}

pub async fn load_state(db: &DatabaseConnection) -> Result<release_calendar_state::Model, DbErr> {
    if let Some(row) =
        release_calendar_state::Entity::find_by_id(release_calendar_state::SINGLETON_ID)
            .one(db)
            .await?
    {
        return Ok(row);
    }
    // The migration seeds the row; this covers a database where it was
    // deleted by hand.
    release_calendar_state::ActiveModel {
        id: Set(release_calendar_state::SINGLETON_ID),
        updated_at: Set(Utc::now().fixed_offset()),
        ..Default::default()
    }
    .insert(db)
    .await
}

/// Drop a failure backoff so the next pass may start at once (an admin
/// pressed "Sync now").
pub async fn clear_backoff(db: &DatabaseConnection) -> Result<(), DbErr> {
    let row = load_state(db).await?;
    if row.next_attempt_at.is_none() {
        return Ok(());
    }
    let mut active = row.into_active_model();
    active.next_attempt_at = Set(None);
    active.updated_at = Set(Utc::now().fixed_offset());
    active.update(db).await?;
    Ok(())
}

/// True when a list pass should be queued: one is stopped part-way, or
/// the last complete one is older than [`PASS_DUE_AFTER_HOURS`] (or
/// there has never been one), and no failure backoff is running.
pub fn pass_due(state: &release_calendar_state::Model, now: DateTime<FixedOffset>) -> bool {
    if state.next_attempt_at.is_some_and(|until| until > now) {
        return false;
    }
    if state.pass_next_week.is_some() {
        return true;
    }
    state
        .last_finished_at
        .is_none_or(|at| now - at > Duration::hours(PASS_DUE_AFTER_HOURS))
}

/// The pass to run now: resume the stopped one if it is recent and its
/// window still covers today's, otherwise start a new one.
fn plan_pass(
    state: &release_calendar_state::Model,
    wanted: Window,
    now: DateTime<FixedOffset>,
) -> (DateTime<FixedOffset>, Window, NaiveDate) {
    if let (Some(started), Some(start), Some(end), Some(next)) = (
        state.pass_started_at,
        state.pass_window_start,
        state.pass_window_end,
        state.pass_next_week,
    ) && now - started <= Duration::hours(RESUME_WITHIN_HOURS)
        && start == wanted.start
        && end == wanted.end
        && next >= start
        && next <= end
    {
        return (started, Window { start, end }, next);
    }
    (now, wanted, wanted.start)
}

/// Walk the window and upsert what Metron lists. See the module docs.
pub async fn list_pass(state: &AppState) -> Result<PassOutcome, DbErr> {
    let Some(client) = metron(state) else {
        return Ok(PassOutcome::Disabled);
    };
    let db = &state.db;
    let now = Utc::now().fixed_offset();
    let row = load_state(db).await?;
    if let Some(until) = row.next_attempt_at
        && until > now
    {
        return Ok(PassOutcome::BackingOff { until });
    }

    let wanted = window(now.date_naive(), state.cfg().upcoming_weeks_ahead);
    let (started, win, first_week) = plan_pass(&row, wanted, now);
    let mut active = row.into_active_model();
    active.pass_started_at = Set(Some(started));
    active.pass_window_start = Set(Some(win.start));
    active.pass_window_end = Set(Some(win.end));
    active.pass_next_week = Set(Some(first_week));
    active.updated_at = Set(now);
    let mut row = active.update(db).await?;

    let mut requests = 0u32;
    let mut weeks = 0u32;
    let mut issues = 0u64;
    // False once any week could not be listed to its last page: the
    // pass then must not delete what it "did not see".
    let mut complete = true;

    for monday in win.weeks().filter(|monday| *monday >= first_week) {
        let sunday = monday + Duration::days(6);
        let mut seen: HashSet<i64> = HashSet::new();
        let mut page = 1u32;
        loop {
            if !budget_ok(state).await {
                return Ok(PassOutcome::Paused {
                    reason: Pause::BudgetReserve,
                    next_week: monday,
                    requests,
                });
            }
            let fetched = match client.list_issues_by_store_date(monday, sunday, page).await {
                Ok(p) => p,
                Err(ProviderError::QuotaExceeded { .. }) => {
                    return Ok(PassOutcome::Paused {
                        reason: Pause::Quota,
                        next_week: monday,
                        requests,
                    });
                }
                Err(e) => {
                    let error = e.to_string();
                    let failed_at = Utc::now().fixed_offset();
                    let mut active = row.into_active_model();
                    active.last_error = Set(Some(error.clone()));
                    active.last_error_at = Set(Some(failed_at));
                    active.next_attempt_at =
                        Set(Some(failed_at + Duration::minutes(FAILURE_BACKOFF_MINUTES)));
                    active.updated_at = Set(failed_at);
                    active.update(db).await?;
                    return Ok(PassOutcome::Failed { error });
                }
            };
            requests += 1;
            // Dedupe on the Metron issue id: a row can repeat across
            // pages when the list shifts while it is being read.
            let fresh: Vec<CalendarIssue> = fetched
                .issues
                .into_iter()
                .filter(|i| seen.insert(i.issue_id))
                .collect();
            issues += fresh.len() as u64;
            upsert(db, &fresh, Utc::now().fixed_offset()).await?;
            if !fetched.has_next {
                break;
            }
            if page >= MAX_PAGES_PER_WEEK {
                complete = false;
                tracing::warn!(
                    week = %monday,
                    pages = page,
                    "release calendar: week exceeded its page cap; nothing will be removed this pass"
                );
                break;
            }
            page += 1;
        }
        weeks += 1;
        let mut active = row.into_active_model();
        active.pass_next_week = Set(Some(monday + Duration::weeks(1)));
        active.updated_at = Set(Utc::now().fixed_offset());
        row = active.update(db).await?;
    }

    let removed = if complete {
        remove_unseen(db, win, started).await?
    } else {
        0
    };
    let finished = Utc::now().fixed_offset();
    prune(db, finished.date_naive()).await?;
    let mut active = row.into_active_model();
    active.pass_next_week = Set(None);
    active.last_finished_at = Set(Some(finished));
    active.last_error = Set(None);
    active.last_error_at = Set(None);
    active.next_attempt_at = Set(None);
    active.updated_at = Set(finished);
    active.update(db).await?;
    metrics::gauge!(LAST_SYNC_GAUGE).set(finished.timestamp() as f64);
    match release_calendar_issue::Entity::find().count(db).await {
        Ok(n) => metrics::gauge!(ISSUES_GAUGE).set(n as f64),
        Err(e) => tracing::warn!(error = %e, "release calendar: issue count for the gauge failed"),
    }
    Ok(PassOutcome::Finished {
        weeks,
        issues,
        removed,
        requests,
    })
}

/// Insert or refresh a page of issues and the series they belong to.
/// A series row keeps the detail columns the enrich sweep filled.
async fn upsert(
    db: &DatabaseConnection,
    issues: &[CalendarIssue],
    now: DateTime<FixedOffset>,
) -> Result<(), DbErr> {
    if issues.is_empty() {
        return Ok(());
    }
    let mut series: BTreeMap<i64, release_calendar_series::ActiveModel> = BTreeMap::new();
    for i in issues {
        series
            .entry(i.series_id)
            .or_insert_with(|| release_calendar_series::ActiveModel {
                metron_series_id: Set(i.series_id),
                name: Set(i.series_name.clone()),
                volume: Set(i.volume),
                year_began: Set(i.year_began),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            });
    }
    release_calendar_series::Entity::insert_many(series.into_values())
        .on_conflict(
            OnConflict::column(release_calendar_series::Column::MetronSeriesId)
                .update_columns([
                    release_calendar_series::Column::Name,
                    release_calendar_series::Column::Volume,
                    release_calendar_series::Column::YearBegan,
                    release_calendar_series::Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(db)
        .await?;

    let rows = issues.iter().map(|i| release_calendar_issue::ActiveModel {
        metron_issue_id: Set(i.issue_id),
        metron_series_id: Set(i.series_id),
        number: Set(i.number.clone()),
        store_date: Set(i.store_date),
        cover_date: Set(i.cover_date),
        image_url: Set(i.image_url.clone()),
        cover_hash: Set(i.cover_hash.clone()),
        metron_modified: Set(i.modified.map(|m| m.fixed_offset())),
        first_seen_at: Set(now),
        last_seen_at: Set(now),
        // The cover colour columns belong to `read_cover_tones`.
        ..Default::default()
    });
    release_calendar_issue::Entity::insert_many(rows)
        .on_conflict(
            OnConflict::column(release_calendar_issue::Column::MetronIssueId)
                .update_columns([
                    release_calendar_issue::Column::MetronSeriesId,
                    release_calendar_issue::Column::Number,
                    release_calendar_issue::Column::StoreDate,
                    release_calendar_issue::Column::CoverDate,
                    release_calendar_issue::Column::ImageUrl,
                    release_calendar_issue::Column::CoverHash,
                    release_calendar_issue::Column::MetronModified,
                    release_calendar_issue::Column::LastSeenAt,
                ])
                .to_owned(),
        )
        .exec(db)
        .await?;
    Ok(())
}

/// Delete window rows the pass that began at `pass_started` did not
/// stamp: Metron no longer lists them (cancelled, or moved out of the
/// window). A release that moved to another week inside the window was
/// stamped under its new date and stays.
async fn remove_unseen(
    db: &DatabaseConnection,
    win: Window,
    pass_started: DateTime<FixedOffset>,
) -> Result<u64, DbErr> {
    let res = release_calendar_issue::Entity::delete_many()
        .filter(release_calendar_issue::Column::StoreDate.between(win.start, win.end))
        .filter(release_calendar_issue::Column::LastSeenAt.lt(pass_started))
        .exec(db)
        .await?;
    Ok(res.rows_affected)
}

/// Delete issues long past their store date, then series left with no
/// issue at all.
async fn prune(db: &DatabaseConnection, today: NaiveDate) -> Result<(), DbErr> {
    release_calendar_issue::Entity::delete_many()
        .filter(
            release_calendar_issue::Column::StoreDate.lt(today - Duration::days(PRUNE_AFTER_DAYS)),
        )
        .exec(db)
        .await?;
    db.execute_unprepared(
        "DELETE FROM release_calendar_series s \
         WHERE NOT EXISTS (SELECT 1 FROM release_calendar_issue i \
                           WHERE i.metron_series_id = s.metron_series_id)",
    )
    .await?;
    Ok(())
}

/// Covers whose colours are read per sweep tick. Each is one image
/// request to Metron's image host (not its API, so it draws on no rate
/// bucket): thirty a minute fills a first window of some 800 covers in
/// under half an hour and is a trickle afterwards.
pub const COVER_TONES_PER_TICK: u64 = 30;
/// A cover that could not be fetched or decoded is tried again after
/// this long.
const COVER_TONE_RETRY_HOURS: i64 = 24;
const COVER_TONE_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// A tick stops starting downloads after this long, so a slow image host
/// cannot hold the sweep's lock (four minutes) past the next ticks.
const COVER_TONE_TICK_BUDGET: std::time::Duration = std::time::Duration::from_secs(40);

/// The pooled, SSRF-vetted client the cover fetches share.
fn cover_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            crate::util::ssrf::shared_public_client(
                crate::build_info::USER_AGENT_COVER,
                COVER_TONE_FETCH_TIMEOUT,
                2,
                true,
            )
        })
        .clone()
}

#[derive(Debug, FromQueryResult)]
struct TonePick {
    metron_issue_id: i64,
    image_url: String,
}

/// Download and decode one cover and read its colours. `None` on any
/// failure: the caller stamps the attempt and moves on.
async fn fetch_cover_tone(url: String) -> Option<String> {
    let fetched = match crate::util::ssrf::fetch_public_bytes_pooled(
        &cover_client(),
        &url,
        crate::util::ssrf::MAX_IMAGE_BYTES,
        COVER_TONE_FETCH_TIMEOUT,
        true,
    )
    .await
    {
        Ok(fetched) => fetched,
        Err(e) => {
            tracing::debug!(url, error = %e, "release calendar: cover fetch failed");
            return None;
        }
    };
    let bytes = fetched.bytes;
    tokio::task::spawn_blocking(move || {
        let img = crate::util::image_decode::decode_limited(&bytes).ok()?;
        crate::metadata::cover_tone::tone_from_image(&img).map(|t| t.encode())
    })
    .await
    .ok()
    .flatten()
}

/// Read the colours of up to [`COVER_TONES_PER_TICK`] covers that have
/// none for their current image, soonest release first (this week's
/// before the far end of the window). Returns how many were read.
///
/// A cover read before whose image has since changed is read again at
/// once. One that has never been read successfully waits out
/// [`COVER_TONE_RETRY_HOURS`] between attempts.
///
/// Metron's image host sends no CORS headers, so the browser cannot do
/// this itself; the server downloads each cover once instead (owner
/// decision, 2026-11-15). These are image requests, not API requests, so
/// the tick does not wait on the Metron budget gate; its cap, the daily
/// retry and one request at a time keep it polite. Only covers on
/// [`super::COVER_HOST_PREFIX`] are fetched, the same ones the browser is
/// allowed to show, and only while the calendar is fed by Metron itself:
/// with `metron_base_url` pointed elsewhere (a test double, a mirror)
/// the covers are not ours to assume, and nothing is fetched.
pub async fn read_cover_tones(state: &AppState) -> Result<u32, DbErr> {
    if state.cfg().metron_base_url.is_some() {
        return Ok(0);
    }
    read_cover_tones_with(state, fetch_cover_tone).await
}

/// [`read_cover_tones`] with the download swapped out, so its picking,
/// stamping and retry rules can be tested without the network.
pub async fn read_cover_tones_with<F, Fut>(state: &AppState, fetch: F) -> Result<u32, DbErr>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    let db = &state.db;
    let now = Utc::now().fixed_offset();
    let picks = TonePick::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT metron_issue_id, image_url \
         FROM release_calendar_issue \
         WHERE image_url IS NOT NULL \
           AND cover_tone_url IS DISTINCT FROM image_url \
           AND (cover_tone_url IS NOT NULL \
                OR cover_tone_checked_at IS NULL \
                OR cover_tone_checked_at < $1) \
         ORDER BY (store_date < $2), store_date, metron_issue_id \
         LIMIT $3",
        [
            (now - Duration::hours(COVER_TONE_RETRY_HOURS)).into(),
            super::week_start(now.date_naive()).into(),
            (COVER_TONES_PER_TICK as i64).into(),
        ],
    ))
    .all(db)
    .await?;

    let started = std::time::Instant::now();
    let mut read = 0u32;
    for pick in picks {
        if started.elapsed() > COVER_TONE_TICK_BUDGET {
            break;
        }
        let tone = if pick.image_url.starts_with(super::COVER_HOST_PREFIX) {
            fetch(pick.image_url.clone())
                .await
                .filter(|t| crate::metadata::cover_tone::is_encoded(t))
        } else {
            None
        };
        let mut row = release_calendar_issue::ActiveModel {
            metron_issue_id: Set(pick.metron_issue_id),
            cover_tone_checked_at: Set(Some(now)),
            ..Default::default()
        };
        if let Some(tone) = tone {
            row.cover_tone = Set(Some(tone));
            row.cover_tone_url = Set(Some(pick.image_url));
            read += 1;
        }
        // The row can vanish under a list pass; that is not an error.
        match row.update(db).await {
            Ok(_) | Err(DbErr::RecordNotUpdated) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(read)
}

#[derive(Debug, FromQueryResult)]
struct EnrichPick {
    metron_series_id: i64,
    recheck: bool,
}

/// Fetch the series detail for up to [`ENRICH_PER_TICK`] series: never
/// fetched first, soonest release first (the weeks a reader looks at are
/// complete long before the far end of the window), then those still
/// without a ComicVine id whose last fetch is older than
/// [`RECHECK_MISSING_CV_DAYS`] and that have a release this week or later.
pub async fn enrich(state: &AppState) -> Result<EnrichOutcome, DbErr> {
    let Some(client) = metron(state) else {
        return Ok(EnrichOutcome::default());
    };
    let db = &state.db;
    let now = Utc::now().fixed_offset();
    let picks = EnrichPick::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT s.metron_series_id, (s.detail_fetched_at IS NOT NULL) AS recheck \
         FROM release_calendar_series s \
         LEFT JOIN LATERAL ( \
             SELECT MIN(i.store_date) AS next_date \
             FROM release_calendar_issue i \
             WHERE i.metron_series_id = s.metron_series_id \
               AND i.store_date >= $2 \
         ) n ON TRUE \
         WHERE s.detail_fetched_at IS NULL \
            OR (s.cv_series_id IS NULL AND s.detail_fetched_at < $1 \
                AND n.next_date IS NOT NULL) \
         ORDER BY (s.detail_fetched_at IS NOT NULL), n.next_date NULLS LAST, \
                  s.detail_fetched_at, s.metron_series_id \
         LIMIT $3",
        [
            (now - Duration::days(RECHECK_MISSING_CV_DAYS)).into(),
            // From Monday: a release earlier this week is still on the
            // page's first section.
            super::week_start(now.date_naive()).into(),
            (ENRICH_PER_TICK as i64).into(),
        ],
    ))
    .all(db)
    .await?;
    if picks.is_empty() {
        return Ok(EnrichOutcome::default());
    }
    // Shown on the Background work page while the tick runs.
    let _task = state.track_background_task("release_calendar_enrich", None);

    let mut out = EnrichOutcome::default();
    for pick in picks {
        if !budget_ok(state).await {
            out.paused = true;
            break;
        }
        let id = pick.metron_series_id.to_string();
        // A re-check must reach Metron: the cached copy (168 h) is the
        // answer that had no ComicVine id.
        let result = if pick.recheck {
            client.fetch_series(&id).await
        } else {
            client.fetch_series_cached(db, &id).await
        };
        match result {
            Ok(detail) => {
                apply_detail(db, pick.metron_series_id, Some(&detail)).await?;
                out.fetched += 1;
            }
            Err(ProviderError::QuotaExceeded { .. }) => {
                out.paused = true;
                break;
            }
            Err(e) => {
                // Stamp it anyway so one bad series cannot hold the head
                // of the queue; it is asked again after the re-check
                // interval because its ComicVine id is still unknown.
                tracing::warn!(
                    metron_series_id = pick.metron_series_id,
                    error = %e,
                    "release calendar: series detail fetch failed"
                );
                apply_detail(db, pick.metron_series_id, None).await?;
                out.fetched += 1;
            }
        }
    }
    Ok(out)
}

/// The ComicVine volume id a Metron series detail names, if any.
fn comicvine_series_id(detail: &GenericMetadata) -> Option<i64> {
    detail
        .identifiers
        .iter()
        .find(|i| i.source == Source::ComicVine)
        .and_then(|i| i.id.trim().parse::<i64>().ok())
}

async fn apply_detail(
    db: &DatabaseConnection,
    metron_series_id: i64,
    detail: Option<&GenericMetadata>,
) -> Result<(), DbErr> {
    let now = Utc::now().fixed_offset();
    let mut active = release_calendar_series::ActiveModel {
        metron_series_id: Set(metron_series_id),
        detail_fetched_at: Set(Some(now)),
        updated_at: Set(now),
        ..Default::default()
    };
    if let Some(d) = detail {
        active.publisher = Set(d.publisher.clone());
        active.series_type = Set(d.series_type.clone());
        active.cv_series_id = Set(comicvine_series_id(d));
    }
    match active.update(db).await {
        Ok(_) => Ok(()),
        // The list pass pruned the series between the pick and now.
        Err(DbErr::RecordNotUpdated) => Ok(()),
        Err(e) => Err(e),
    }
}

/// One week of the calendar, for the admin status card.
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct WeekCount {
    /// The Monday.
    pub week: NaiveDate,
    pub issues: i64,
}

/// Row counts behind the admin status card.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub issues: i64,
    pub series: i64,
    /// Series whose detail has never been fetched.
    pub series_pending_detail: i64,
    /// Series fetched at least once that Metron has not linked to
    /// ComicVine.
    pub series_without_comicvine: i64,
    pub weeks: Vec<WeekCount>,
}

#[derive(Debug, FromQueryResult)]
struct SeriesCounts {
    series: i64,
    pending: i64,
    without_cv: i64,
}

pub async fn counts<C: ConnectionTrait>(db: &C) -> Result<Counts, DbErr> {
    let weeks = WeekCount::find_by_statement(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT date_trunc('week', store_date)::date AS week, count(*) AS issues \
         FROM release_calendar_issue GROUP BY 1 ORDER BY 1",
    ))
    .all(db)
    .await?;
    let series = SeriesCounts::find_by_statement(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT count(*) AS series, \
                count(*) FILTER (WHERE detail_fetched_at IS NULL) AS pending, \
                count(*) FILTER (WHERE detail_fetched_at IS NOT NULL AND cv_series_id IS NULL) \
                    AS without_cv \
         FROM release_calendar_series",
    ))
    .one(db)
    .await?;
    let series = series.unwrap_or(SeriesCounts {
        series: 0,
        pending: 0,
        without_cv: 0,
    });
    Ok(Counts {
        issues: weeks.iter().map(|w| w.issues).sum(),
        series: series.series,
        series_pending_detail: series.pending,
        series_without_comicvine: series.without_cv,
        weeks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn blank() -> release_calendar_state::Model {
        release_calendar_state::Model {
            id: release_calendar_state::SINGLETON_ID,
            pass_started_at: None,
            pass_window_start: None,
            pass_window_end: None,
            pass_next_week: None,
            last_finished_at: None,
            last_error: None,
            last_error_at: None,
            next_attempt_at: None,
            updated_at: at("2026-11-14T00:00:00Z"),
        }
    }

    #[test]
    fn a_pass_is_due_when_none_has_ever_finished() {
        assert!(pass_due(&blank(), at("2026-11-14T12:00:00Z")));
    }

    #[test]
    fn a_pass_is_due_only_after_the_interval() {
        let mut s = blank();
        s.last_finished_at = Some(at("2026-11-14T00:00:00Z"));
        assert!(!pass_due(&s, at("2026-11-15T01:00:00Z")));
        assert!(pass_due(&s, at("2026-11-15T02:30:00Z")));
    }

    #[test]
    fn a_stopped_pass_is_due_at_once_but_not_during_backoff() {
        let mut s = blank();
        s.last_finished_at = Some(at("2026-11-14T00:00:00Z"));
        s.pass_next_week = Some(day("2026-11-23"));
        assert!(pass_due(&s, at("2026-11-14T00:05:00Z")));
        s.next_attempt_at = Some(at("2026-11-14T00:30:00Z"));
        assert!(!pass_due(&s, at("2026-11-14T00:05:00Z")));
        assert!(pass_due(&s, at("2026-11-14T00:31:00Z")));
    }

    #[test]
    fn a_recent_stopped_pass_resumes_at_its_week() {
        let now = at("2026-11-14T12:00:00Z");
        let wanted = window(now.date_naive(), 8);
        let mut s = blank();
        s.pass_started_at = Some(at("2026-11-14T11:00:00Z"));
        s.pass_window_start = Some(wanted.start);
        s.pass_window_end = Some(wanted.end);
        s.pass_next_week = Some(day("2026-11-23"));
        let (started, win, first) = plan_pass(&s, wanted, now);
        assert_eq!(started, at("2026-11-14T11:00:00Z"));
        assert_eq!(win, wanted);
        assert_eq!(first, day("2026-11-23"));
    }

    #[test]
    fn an_old_or_mismatched_stopped_pass_starts_over() {
        let now = at("2026-11-14T12:00:00Z");
        let wanted = window(now.date_naive(), 8);
        let mut s = blank();
        s.pass_started_at = Some(at("2026-11-13T11:00:00Z"));
        s.pass_window_start = Some(wanted.start);
        s.pass_window_end = Some(wanted.end);
        s.pass_next_week = Some(day("2026-11-23"));
        // Older than the resume limit.
        assert_eq!(plan_pass(&s, wanted, now), (now, wanted, wanted.start));
        // Recent, but `weeks_ahead` changed since it began.
        s.pass_started_at = Some(at("2026-11-14T11:00:00Z"));
        let wider = window(now.date_naive(), 10);
        assert_eq!(plan_pass(&s, wider, now), (now, wider, wider.start));
    }
}
