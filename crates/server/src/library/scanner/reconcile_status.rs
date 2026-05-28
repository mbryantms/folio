//! Post-scan publication-status reconciliation.
//!
//! Called once per folder after ingest. Resolves the right
//! `series.status` / `series.total_issues` / `series.summary` /
//! `series.comicvine_id` values from a precedence ladder and writes
//! through any changes:
//!
//! 1. **Manual user override** (`series.status_user_set_at IS NOT NULL`)
//!    pins the status string. The other fields still refresh —
//!    manual override only freezes status, so the
//!    Complete/Incomplete UI badge keeps tracking the publisher's
//!    claimed total even on user-pinned series.
//! 2. **`series.json` sidecar** (passed in via the `sidecar` parameter
//!    when this is called from a folder scan). When present, its
//!    `total_issues` and `status` fields win over per-issue
//!    inference — series.json is authoritative per-series intent.
//!    `description_text` (fallback `description_formatted`) seeds
//!    `series.summary`. `comicid` backfills `comicvine_id` when the
//!    row's value is NULL.
//! 3. **MAX(`issues.comicinfo_count`)** — fallback when the sidecar
//!    is absent. Per-issue `<Count>` carries a publisher-claimed
//!    total, so MAX captures it regardless of which issue tagged it.
//! 4. **Default** — when nothing else applies, leave the existing
//!    values alone. Notably `total_issues` is NEVER overwritten with
//!    NULL; a sidecar-set total survives a later signal-less scan.
//!
//! Manual override sticky-pattern is the same shape as
//! `series.match_key` — see `library/identity.rs`.

use chrono::Utc;
use entity::{issue, series};
use parsers::series_json::SeriesMetadata;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait,
    FromQueryResult, PaginatorTrait, QueryFilter, QuerySelect, Statement, sea_query::Expr,
};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, FromQueryResult)]
struct CountRow {
    max_count: Option<i32>,
}

#[derive(Debug, FromQueryResult)]
struct CountBySeriesRow {
    series_id: Uuid,
    max_count: Option<i32>,
}

#[derive(Debug, FromQueryResult)]
struct VolumeModeRow {
    mode_volume: Option<i32>,
}

#[derive(Debug, FromQueryResult)]
struct VolumeModeBySeriesRow {
    series_id: Uuid,
    mode_volume: Option<i32>,
}

/// Bag of post-ingest aggregates used by `apply_reconciled_status`.
/// One per series row — gathered once via grouped queries when reconciling
/// a scan batch, or one-off for the single-series variant.
#[derive(Debug, Default, Clone, Copy)]
struct IssueAggregates {
    /// MAX(`<Count>`) across the series' active issues. Drives the
    /// status / total_issues ladder.
    comicinfo_count: Option<i32>,
    /// Statistical mode of `issues.volume` across the series' active
    /// issues — survives the per-issue plausibility filter applied
    /// during `process::ingest`, so any value here is already in the
    /// realistic 1–99 range. Used to backfill `series.volume` when no
    /// sidecar value is available, healing rows that earlier scans
    /// stamped with a `V<year>` filename token.
    volume_mode: Option<i32>,
}

/// Recompute the series's metadata-derived fields. The `sidecar`
/// argument is `Some(_)` when called from a folder scan with a
/// `series.json` present, `None` when called from the tombstone
/// reconcile path or from any context that doesn't have the folder
/// in scope.
///
/// # Errors
/// Propagates any DB error encountered. The reconciliation is
/// idempotent — repeated calls converge.
///
// TODO(future): the MAX() reduction is robust to a missing `<Count>`
// on most issues but a single mis-tagged annual with `<Count>1` can
// flip a 99-issue ongoing series to "ended". If users report this in
// the wild, swap the reduction for "Count from the issue with the
// highest sort_number, fall back to highest year/month".
pub async fn reconcile_series_status<C>(
    db: &C,
    series_id: Uuid,
    sidecar: Option<&SeriesMetadata>,
) -> Result<(), DbErr>
where
    C: ConnectionTrait,
{
    let backend = db.get_database_backend();
    let count_stmt = Statement::from_sql_and_values(
        backend,
        "SELECT MAX(comicinfo_count) AS max_count \
         FROM issues \
         WHERE series_id = $1 AND removed_at IS NULL",
        [series_id.into()],
    );
    let max_row = CountRow::find_by_statement(count_stmt).one(db).await?;
    let comicinfo_count: Option<i32> = max_row.and_then(|r| r.max_count).filter(|n| *n > 0);

    // MODE() is Postgres-specific; the dev + prod backends are both
    // Postgres so this is safe. If we ever add a SQLite backend the
    // mode call would need a fallback (e.g. `SELECT volume, COUNT(*)
    // ... GROUP BY volume ORDER BY 2 DESC LIMIT 1`).
    let mode_stmt = Statement::from_sql_and_values(
        backend,
        "SELECT MODE() WITHIN GROUP (ORDER BY volume) AS mode_volume \
         FROM issues \
         WHERE series_id = $1 AND removed_at IS NULL AND volume IS NOT NULL",
        [series_id.into()],
    );
    let mode_row = VolumeModeRow::find_by_statement(mode_stmt).one(db).await?;
    let volume_mode: Option<i32> = mode_row.and_then(|r| r.mode_volume);

    let row = match series::Entity::find_by_id(series_id).one(db).await? {
        Some(r) => r,
        None => return Ok(()), // series was deleted between scan and reconcile
    };
    apply_reconciled_status(
        db,
        row,
        IssueAggregates {
            comicinfo_count,
            volume_mode,
        },
        sidecar,
    )
    .await
}

/// Set-based equivalent of [`reconcile_series_status`] for scanner batches.
/// Each entry is a touched series id plus the sidecar parsed from that
/// series folder, if one was present. Counts and current series rows are
/// loaded in two grouped queries instead of one query pair per folder.
pub async fn reconcile_series_status_many<C>(
    db: &C,
    entries: &[(Uuid, Option<SeriesMetadata>)],
) -> Result<(), DbErr>
where
    C: ConnectionTrait,
{
    if entries.is_empty() {
        return Ok(());
    }

    let sidecars: HashMap<Uuid, Option<SeriesMetadata>> = entries.iter().cloned().collect();
    let ids: Vec<Uuid> = sidecars.keys().copied().collect();

    let count_rows: Vec<CountBySeriesRow> = issue::Entity::find()
        .select_only()
        .column(issue::Column::SeriesId)
        .column_as(Expr::col(issue::Column::ComicinfoCount).max(), "max_count")
        .filter(issue::Column::SeriesId.is_in(ids.clone()))
        .filter(issue::Column::RemovedAt.is_null())
        .group_by(issue::Column::SeriesId)
        .into_model()
        .all(db)
        .await?;
    let counts: HashMap<Uuid, Option<i32>> = count_rows
        .into_iter()
        .map(|r| (r.series_id, r.max_count.filter(|n| *n > 0)))
        .collect();

    // Grouped MODE() across active issues' `volume`. Raw SQL because
    // SeaORM's expression builder doesn't model `WITHIN GROUP`. The
    // `ANY($1::uuid[])` pattern works under both `sea-orm-postgres`
    // and bare `sqlx` parameter binding without manual quoting.
    let backend = db.get_database_backend();
    let mode_stmt = Statement::from_sql_and_values(
        backend,
        "SELECT series_id, \
                MODE() WITHIN GROUP (ORDER BY volume) AS mode_volume \
         FROM issues \
         WHERE series_id = ANY($1::uuid[]) \
           AND removed_at IS NULL \
           AND volume IS NOT NULL \
         GROUP BY series_id",
        [ids.clone().into()],
    );
    let mode_rows: Vec<VolumeModeBySeriesRow> = VolumeModeBySeriesRow::find_by_statement(mode_stmt)
        .all(db)
        .await?;
    let volume_modes: HashMap<Uuid, Option<i32>> = mode_rows
        .into_iter()
        .map(|r| (r.series_id, r.mode_volume))
        .collect();

    let rows = series::Entity::find()
        .filter(series::Column::Id.is_in(ids))
        .all(db)
        .await?;

    for row in rows {
        let sidecar = sidecars.get(&row.id).and_then(|m| m.as_ref());
        let aggregates = IssueAggregates {
            comicinfo_count: counts.get(&row.id).copied().flatten(),
            volume_mode: volume_modes.get(&row.id).copied().flatten(),
        };
        apply_reconciled_status(db, row, aggregates, sidecar).await?;
    }
    Ok(())
}

async fn apply_reconciled_status<C>(
    db: &C,
    row: series::Model,
    aggregates: IssueAggregates,
    sidecar: Option<&SeriesMetadata>,
) -> Result<(), DbErr>
where
    C: ConnectionTrait,
{
    let comicinfo_count = aggregates.comicinfo_count;

    // Resolve each field using the precedence ladder. None on either
    // signal source means "no change" — never overwrite an existing
    // value with NULL just because we didn't see it this scan.
    let resolved_total: Option<i32> = sidecar
        .and_then(|m| m.total_issues)
        .filter(|n| *n > 0)
        .or(comicinfo_count);

    // `normalize_status` takes `Option<&str>` and always returns a
    // valid enum value, so we wrap to keep the call shape uniform.
    let resolved_status: Option<&'static str> = sidecar
        .and_then(|m| m.status.as_deref())
        .map(|s| parsers::series_json::normalize_status(Some(s)))
        .or_else(|| comicinfo_count.is_some().then_some("ended"));

    let resolved_summary: Option<String> = sidecar.and_then(|m| {
        m.description_text
            .as_deref()
            .or(m.description_formatted.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    });

    let resolved_comicvine_id: Option<i64> = sidecar.and_then(|m| m.comicid);

    let mut am: series::ActiveModel = row.clone().into();
    let mut dirty = false;

    // total_issues: write only when we have a signal AND it differs.
    // No-signal scans (e.g. tombstone path with no sidecar and no
    // Count) leave the previous value intact — this is the bug fix
    // vs. the v1 "always overwrite" behavior.
    if let Some(t) = resolved_total
        && row.total_issues != Some(t)
    {
        am.total_issues = Set(Some(t));
        am.updated_at = Set(Utc::now().fixed_offset());
        dirty = true;
    }

    // status: skip when user has pinned it. Otherwise apply when the
    // resolved value differs from what's on the row.
    if row.status_user_set_at.is_none()
        && let Some(s) = resolved_status
        && row.status != s
    {
        am.status = Set(s.to_owned());
        am.updated_at = Set(Utc::now().fixed_offset());
        dirty = true;
    }

    // summary: write only when sidecar provided one AND we don't
    // already have it. Don't clobber a richer existing summary
    // (could have been set by an admin via PATCH or a richer source
    // like a future ComicVine API integration).
    if let Some(s) = resolved_summary
        && row.summary.as_deref().is_none_or(str::is_empty)
    {
        am.summary = Set(Some(s));
        am.updated_at = Set(Utc::now().fixed_offset());
        dirty = true;
    }

    // CV id backfill routes through writers::set_external_id
    // (entity_type='series', source='comicvine'). The helper's
    // "skip if set_by='user'" semantics replace the old IS NULL
    // check inline at this layer.
    if let Some(cv) = resolved_comicvine_id {
        crate::metadata::writers::set_external_id(
            db,
            "series",
            &row.id.to_string(),
            &crate::metadata::Identifier::new(crate::metadata::Source::ComicVine, cv.to_string()),
            crate::metadata::writers::SetBy::ComicInfo,
        )
        .await?;
    }

    // ───── volume / name / publisher self-heal ─────
    //
    // These three fields were historically only set at series-creation
    // time (identity.rs tier 4), which meant a rescan couldn't repair
    // values stamped by earlier-buggy scanner versions. The most visible
    // case: filename `V<year>` tokens (Mylar3 fill-in) landed in
    // `series.volume` as `2016` / `2023` / etc. on ~99 % of one user's
    // library. With the plausibility filter on the inference side,
    // FRESH series get correct values — but existing rows can only heal
    // here.
    //
    // Precedence ladder (matches what identity.rs uses for hint
    // computation on new series):
    //   1. series.json sidecar — curated metadata wins outright,
    //      INCLUDING an explicit `null` / absent `volume`. A sidecar
    //      that doesn't carry a volume is a positive assertion that
    //      no volume exists for this series (most single-run titles);
    //      treating it as "no signal" leaves stale year-stamped values
    //      from prior buggy scans untouched.
    //   2. MODE() over `issues.volume` — survives per-issue plausibility
    //      filter, so any value here is realistic.
    //   3. No signal → leave the row alone.
    //
    // For `name` / `publisher` we don't fall back to issue-level
    // aggregates today — series-level data isn't denormalized onto
    // issues, and trusting the first-issue ComicInfo enough to overwrite
    // an existing `name` row has too many false-positive failure modes
    // (renamed crossovers, multi-series anthology folders). Sidecar
    // only.

    // `Some(Some(v))` = write `v`; `Some(None)` = write NULL;
    // `None` = no signal, leave the row alone.
    let target_volume: Option<Option<i32>> = match sidecar {
        Some(meta) => Some(meta.volume),
        None => aggregates.volume_mode.map(Some),
    };
    if let Some(new_value) = target_volume
        && row.volume != new_value
    {
        am.volume = Set(new_value);
        am.updated_at = Set(Utc::now().fixed_offset());
        dirty = true;
    }

    if let Some(name) = sidecar
        .and_then(|m| m.name.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && row.name != name
    {
        am.name = Set(name.to_owned());
        am.normalized_name = Set(entity::series::normalize_name(name));
        am.updated_at = Set(Utc::now().fixed_offset());
        dirty = true;
    }

    if let Some(publisher) = sidecar
        .and_then(|m| m.publisher.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && row.publisher.as_deref() != Some(publisher)
    {
        am.publisher = Set(Some(publisher.to_owned()));
        am.updated_at = Set(Utc::now().fixed_offset());
        dirty = true;
    }

    if dirty {
        am.update(db).await?;
    }
    Ok(())
}

/// Convenience wrapper: count active, non-removed issues for a series.
/// Mirrors the aggregate semantics of `hydrate_series` in the API
/// layer, so `collection_size` here matches what the UI shows.
/// Currently unused by the reconcile path itself — the
/// Complete/Incomplete derivation lives client-side — but exposed for
/// tests and future server-side consumers.
pub async fn collection_size<C>(db: &C, series_id: Uuid) -> Result<i64, DbErr>
where
    C: ConnectionTrait,
{
    issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(series_id))
        .filter(issue::Column::RemovedAt.is_null())
        .filter(issue::Column::State.eq("active"))
        .count(db)
        .await
        .map(|n| n as i64)
}
