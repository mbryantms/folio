//! The `mylar_add_request` rows: turning missing reading-list entries
//! into requests, and reading back how each one stands.
//!
//! "Already in Mylar" is read from the pushed snapshot (`mylar_watch`,
//! `mylar_wanted`), never by asking Mylar here: creating requests and
//! showing their state cost no Mylar call. Only [`super::sweep`] calls.

use chrono::Utc;
use entity::{cbl_entry, mylar_add_request, mylar_wanted, mylar_watch};
use sea_orm::sea_query::{Expr, ExprTrait, OnConflict};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, FromQueryResult,
    QueryFilter, QuerySelect,
};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

pub mod status {
    pub const PENDING: &str = "pending";
    pub const DONE: &str = "done";
    pub const FAILED: &str = "failed";
}

/// Entries per request body; a longer selection is sent as "all missing".
pub const MAX_ENTRY_IDS: usize = 500;
const INSERT_CHUNK: usize = 500;
/// How long a `done` request stands before asking again re-sends it. By
/// then a snapshot has normally shown the series in Mylar; if it has not,
/// the series was probably removed there.
pub const REDO_DONE_AFTER: chrono::Duration = chrono::Duration::hours(24);

/// The columns of a missing entry that matter to Mylar.
#[derive(Debug, FromQueryResult)]
struct MissingEntry {
    series_name: String,
    issue_number: String,
    cv_series_id: Option<i32>,
    cv_issue_id: Option<i32>,
}

async fn missing_entries<C: ConnectionTrait>(
    db: &C,
    list_id: Uuid,
    entry_ids: Option<&[Uuid]>,
) -> Result<Vec<MissingEntry>, DbErr> {
    let mut q = cbl_entry::Entity::find()
        .select_only()
        .columns([
            cbl_entry::Column::SeriesName,
            cbl_entry::Column::IssueNumber,
            cbl_entry::Column::CvSeriesId,
            cbl_entry::Column::CvIssueId,
        ])
        .filter(cbl_entry::Column::CblListId.eq(list_id))
        .filter(cbl_entry::Column::MatchStatus.eq("missing"));
    if let Some(ids) = entry_ids {
        q = q.filter(cbl_entry::Column::Id.is_in(ids.iter().copied()));
    }
    q.into_model::<MissingEntry>().all(db).await
}

async fn watched<C: ConnectionTrait>(db: &C, ids: &[i64]) -> Result<HashSet<i64>, DbErr> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let rows: Vec<i64> = mylar_watch::Entity::find()
        .select_only()
        .column(mylar_watch::Column::CvSeriesId)
        .filter(mylar_watch::Column::CvSeriesId.is_in(ids.iter().copied()))
        .into_tuple()
        .all(db)
        .await?;
    Ok(rows.into_iter().collect())
}

async fn wanted<C: ConnectionTrait>(db: &C, ids: &[i64]) -> Result<HashSet<i64>, DbErr> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let rows: Vec<Option<i64>> = mylar_wanted::Entity::find()
        .select_only()
        .column(mylar_wanted::Column::CvIssueId)
        .filter(mylar_wanted::Column::CvIssueId.is_in(ids.iter().copied()))
        .into_tuple()
        .all(db)
        .await?;
    Ok(rows.into_iter().flatten().collect())
}

/// What one "Add to Mylar" request did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct AddSummary {
    /// Series newly queued for adding to Mylar.
    pub series_requested: u64,
    /// Issues newly queued to be marked Wanted.
    pub issues_requested: u64,
    /// Series the last snapshot already shows on Mylar's watchlist.
    pub series_in_mylar: u64,
    /// Series and issues with a request already waiting or just accepted.
    pub already_requested: u64,
    /// Chosen entries with no ComicVine series id; Mylar cannot take them.
    pub without_comicvine_id: u64,
}

/// Queue the chosen missing entries of a reading list (all of them when
/// `entry_ids` is `None`). A target that failed before is queued afresh;
/// one still waiting or just accepted is left alone (see [`upsert`]).
pub async fn create<C: ConnectionTrait>(
    db: &C,
    actor: Uuid,
    list_id: Uuid,
    entry_ids: Option<&[Uuid]>,
    want_issues: bool,
) -> Result<AddSummary, DbErr> {
    let entries = missing_entries(db, list_id, entry_ids).await?;
    let mut summary = AddSummary::default();

    // BTreeMap: a stable insert order keeps concurrent requests from
    // deadlocking on the unique index.
    let mut series: BTreeMap<i64, String> = BTreeMap::new();
    let mut issues: BTreeMap<i64, (i64, String, String)> = BTreeMap::new();
    for e in entries {
        let Some(series_id) = e.cv_series_id.map(i64::from) else {
            summary.without_comicvine_id += 1;
            continue;
        };
        series
            .entry(series_id)
            .or_insert_with(|| e.series_name.clone());
        if want_issues && let Some(issue_id) = e.cv_issue_id.map(i64::from) {
            issues
                .entry(issue_id)
                .or_insert((series_id, e.series_name, e.issue_number));
        }
    }

    let series_ids: Vec<i64> = series.keys().copied().collect();
    let in_mylar = watched(db, &series_ids).await?;
    summary.series_in_mylar = in_mylar.len() as u64;
    let issue_ids: Vec<i64> = issues.keys().copied().collect();
    let already_wanted = wanted(db, &issue_ids).await?;

    let now = Utc::now().fixed_offset();
    let row = |series_id: i64, issue_id: Option<i64>, name: String, number: Option<String>| {
        mylar_add_request::ActiveModel {
            id: Set(Uuid::now_v7()),
            cv_series_id: Set(series_id),
            cv_issue_id: Set(issue_id),
            series_name: Set(name),
            issue_number: Set(number),
            status: Set(status::PENDING.to_owned()),
            attempts: Set(0),
            last_error: Set(None),
            next_attempt_at: Set(now),
            requested_by: Set(Some(actor)),
            created_at: Set(now),
            updated_at: Set(now),
        }
    };
    let series_rows: Vec<_> = series
        .into_iter()
        .filter(|(id, _)| !in_mylar.contains(id))
        .map(|(id, name)| row(id, None, name, None))
        .collect();
    let issue_rows: Vec<_> = issues
        .into_iter()
        .filter(|(id, _)| !already_wanted.contains(id))
        .map(|(id, (series_id, name, number))| row(series_id, Some(id), name, Some(number)))
        .collect();

    let series_asked = series_rows.len() as u64;
    let issues_asked = issue_rows.len() as u64;
    summary.series_requested = upsert(db, series_rows).await?;
    summary.issues_requested = upsert(db, issue_rows).await?;
    summary.already_requested =
        (series_asked - summary.series_requested) + (issues_asked - summary.issues_requested);
    Ok(summary)
}

/// Queue one series by its ComicVine volume id (the Upcoming page: a
/// release whose series Mylar is not tracking). Same rules as [`create`]:
/// nothing is queued for a series the snapshot shows on the watchlist, or
/// one with a request waiting or just accepted.
pub async fn create_series<C: ConnectionTrait>(
    db: &C,
    actor: Uuid,
    cv_series_id: i64,
    series_name: &str,
) -> Result<AddSummary, DbErr> {
    let mut summary = AddSummary::default();
    if !watched(db, &[cv_series_id]).await?.is_empty() {
        summary.series_in_mylar = 1;
        return Ok(summary);
    }
    let now = Utc::now().fixed_offset();
    let row = mylar_add_request::ActiveModel {
        id: Set(Uuid::now_v7()),
        cv_series_id: Set(cv_series_id),
        cv_issue_id: Set(None),
        series_name: Set(series_name.to_owned()),
        issue_number: Set(None),
        status: Set(status::PENDING.to_owned()),
        attempts: Set(0),
        last_error: Set(None),
        next_attempt_at: Set(now),
        requested_by: Set(Some(actor)),
        created_at: Set(now),
        updated_at: Set(now),
    };
    summary.series_requested = upsert(db, vec![row]).await?;
    summary.already_requested = 1 - summary.series_requested;
    Ok(summary)
}

/// Insert the rows. A target whose row failed is reset to pending, and so
/// is one Mylar accepted more than [`REDO_DONE_AFTER`] ago (the series may
/// have been removed from Mylar since). A row still pending, or accepted
/// recently, is left alone. Returns how many were inserted or reset.
async fn upsert<C: ConnectionTrait>(
    db: &C,
    rows: Vec<mylar_add_request::ActiveModel>,
) -> Result<u64, DbErr> {
    use mylar_add_request::Column as Col;
    let redo_before = Utc::now().fixed_offset() - REDO_DONE_AFTER;
    let mut affected = 0u64;
    let mut rows = rows.into_iter().peekable();
    while rows.peek().is_some() {
        let chunk: Vec<_> = rows.by_ref().take(INSERT_CHUNK).collect();
        affected += mylar_add_request::Entity::insert_many(chunk)
            .on_conflict(
                OnConflict::columns([Col::CvSeriesId, Col::CvIssueId])
                    .update_columns([
                        Col::SeriesName,
                        Col::IssueNumber,
                        Col::Status,
                        Col::Attempts,
                        Col::LastError,
                        Col::NextAttemptAt,
                        Col::RequestedBy,
                        Col::CreatedAt,
                        Col::UpdatedAt,
                    ])
                    .action_and_where(
                        Expr::col((mylar_add_request::Entity, Col::Status))
                            .eq(status::FAILED)
                            .or(Expr::col((mylar_add_request::Entity, Col::Status))
                                .eq(status::DONE)
                                .and(
                                    Expr::col((mylar_add_request::Entity, Col::UpdatedAt))
                                        .lt(redo_before),
                                )),
                    )
                    .to_owned(),
            )
            .exec_without_returning(db)
            .await?;
    }
    Ok(affected)
}

/// Where a series or an issue stands with Mylar, as far as Folio knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    /// On Mylar's watchlist (series) or wanted / snatched there (issue):
    /// the last snapshot shows it, or Mylar accepted Folio's request.
    InMylar,
    /// Queued in Folio; the next sweep sends it.
    Requested,
    /// Mylar refused it or never imported the series.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct TargetStatus {
    /// ComicVine volume id (series) or issue id (issue).
    pub cv_id: i64,
    pub state: TargetState,
    /// Why it failed.
    pub error: Option<String>,
}

/// The Mylar state of a reading list's missing entries. Targets Folio
/// knows nothing about are left out.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ListStatus {
    /// Missing entries in the list.
    pub missing: u64,
    /// Missing entries with no ComicVine series id.
    pub without_comicvine_id: u64,
    pub series: Vec<TargetStatus>,
    pub issues: Vec<TargetStatus>,
}

pub async fn list_status<C: ConnectionTrait>(db: &C, list_id: Uuid) -> Result<ListStatus, DbErr> {
    let entries = missing_entries(db, list_id, None).await?;
    let mut out = ListStatus {
        missing: entries.len() as u64,
        ..ListStatus::default()
    };
    let mut series_ids: Vec<i64> = Vec::new();
    let mut issue_ids: Vec<i64> = Vec::new();
    for e in &entries {
        match e.cv_series_id {
            Some(id) => series_ids.push(i64::from(id)),
            None => out.without_comicvine_id += 1,
        }
        if e.cv_series_id.is_some()
            && let Some(id) = e.cv_issue_id
        {
            issue_ids.push(i64::from(id));
        }
    }
    series_ids.sort_unstable();
    series_ids.dedup();
    issue_ids.sort_unstable();
    issue_ids.dedup();
    if series_ids.is_empty() {
        return Ok(out);
    }

    let in_mylar = watched(db, &series_ids).await?;
    let already_wanted = wanted(db, &issue_ids).await?;
    let requests = mylar_add_request::Entity::find()
        .filter(mylar_add_request::Column::CvSeriesId.is_in(series_ids.iter().copied()))
        .all(db)
        .await?;
    let mut series_req: HashMap<i64, &mylar_add_request::Model> = HashMap::new();
    let mut issue_req: HashMap<i64, &mylar_add_request::Model> = HashMap::new();
    for r in &requests {
        match r.cv_issue_id {
            None => series_req.insert(r.cv_series_id, r),
            Some(issue) => issue_req.insert(issue, r),
        };
    }
    let resolve = |cv_id: i64, snapshot_has: bool, req: Option<&&mylar_add_request::Model>| {
        let (state, error) = match (snapshot_has, req.map(|r| r.status.as_str())) {
            (true, _) | (false, Some(status::DONE)) => (TargetState::InMylar, None),
            (false, Some(status::FAILED)) => {
                (TargetState::Failed, req.and_then(|r| r.last_error.clone()))
            }
            (false, Some(_)) => (TargetState::Requested, None),
            (false, None) => return None,
        };
        Some(TargetStatus {
            cv_id,
            state,
            error,
        })
    };
    out.series = series_ids
        .iter()
        .filter_map(|id| resolve(*id, in_mylar.contains(id), series_req.get(id)))
        .collect();
    out.issues = issue_ids
        .iter()
        .filter_map(|id| resolve(*id, already_wanted.contains(id), issue_req.get(id)))
        .collect();
    Ok(out)
}
