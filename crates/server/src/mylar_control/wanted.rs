//! Check Mylar's live wanted list against the library.
//!
//! Two questions, answered from one `getWanted` call:
//!
//! - Which issues does Mylar still want that the library already holds?
//!   Usually a file Mylar has not noticed (a recheck of the series fixes
//!   it) or a duplicate want.
//! - How many issues do reading lists miss that Mylar is not looking for?
//!
//! "Holds" means a live issue row carries the ComicVine issue id
//! (`external_ids`), the same link the reading-list matcher uses.

use super::client::WantedIssue;
use entity::{cbl_entry, external_id, issue};
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

/// Owned-but-wanted series returned; the totals count all of them.
pub const MAX_OWNED_SERIES: usize = 200;
const ID_CHUNK: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct OwnedWantedIssue {
    pub cv_issue_id: i64,
    pub issue_number: String,
}

/// A series with issues Mylar wants and the library already holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct OwnedWantedSeries {
    pub cv_series_id: i64,
    pub series: String,
    pub issues: Vec<OwnedWantedIssue>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct WantedCheck {
    /// Issues on Mylar's wanted list right now.
    pub wanted: u64,
    /// Of those, issues the library already holds.
    pub owned_issues: u64,
    /// Series those issues belong to.
    pub owned_series_total: u64,
    /// The first [`MAX_OWNED_SERIES`] of them, by name.
    pub owned: Vec<OwnedWantedSeries>,
    /// Distinct issues reading lists are missing (with a ComicVine id).
    pub list_missing: u64,
    /// Of those, issues Mylar is not looking for.
    pub list_missing_not_wanted: u64,
}

pub async fn check<C: ConnectionTrait>(
    db: &C,
    wanted: Vec<WantedIssue>,
) -> Result<WantedCheck, DbErr> {
    let wanted_ids: HashSet<i64> = wanted.iter().map(|w| w.cv_issue_id).collect();
    let held = held_issue_ids(db, &wanted_ids).await?;

    // Grouped by series id; the name is the first row's (an annual's row
    // carries the annual's title under the same series id).
    let mut by_series: BTreeMap<i64, OwnedWantedSeries> = BTreeMap::new();
    let mut owned_issues = 0u64;
    for w in &wanted {
        if !held.contains(&w.cv_issue_id) {
            continue;
        }
        owned_issues += 1;
        by_series
            .entry(w.cv_series_id)
            .or_insert_with(|| OwnedWantedSeries {
                cv_series_id: w.cv_series_id,
                series: w.series.clone(),
                issues: Vec::new(),
            })
            .issues
            .push(OwnedWantedIssue {
                cv_issue_id: w.cv_issue_id,
                issue_number: w.issue_number.clone(),
            });
    }
    let owned_series_total = by_series.len() as u64;
    let mut owned: Vec<OwnedWantedSeries> = by_series.into_values().collect();
    owned.sort_by_cached_key(|s| (s.series.to_lowercase(), s.cv_series_id));
    owned.truncate(MAX_OWNED_SERIES);

    let missing: Vec<i32> = cbl_entry::Entity::find()
        .select_only()
        .column(cbl_entry::Column::CvIssueId)
        .filter(cbl_entry::Column::MatchStatus.eq("missing"))
        .filter(cbl_entry::Column::CvIssueId.is_not_null())
        .distinct()
        .into_tuple()
        .all(db)
        .await?;
    let list_missing_not_wanted = missing
        .iter()
        .filter(|id| !wanted_ids.contains(&i64::from(**id)))
        .count() as u64;

    Ok(WantedCheck {
        wanted: wanted.len() as u64,
        owned_issues,
        owned_series_total,
        owned,
        list_missing: missing.len() as u64,
        list_missing_not_wanted,
    })
}

/// The ComicVine issue ids, among `ids`, that a live issue row carries.
async fn held_issue_ids<C: ConnectionTrait>(
    db: &C,
    ids: &HashSet<i64>,
) -> Result<HashSet<i64>, DbErr> {
    let mut held = HashSet::new();
    let all: Vec<String> = ids.iter().map(i64::to_string).collect();
    for chunk in all.chunks(ID_CHUNK) {
        let links: Vec<(String, String)> = external_id::Entity::find()
            .select_only()
            .column(external_id::Column::EntityId)
            .column(external_id::Column::ExternalId)
            .filter(external_id::Column::EntityType.eq("issue"))
            .filter(external_id::Column::Source.eq("comicvine"))
            .filter(external_id::Column::ExternalId.is_in(chunk.iter().cloned()))
            .into_tuple()
            .all(db)
            .await?;
        if links.is_empty() {
            continue;
        }
        let live: HashSet<String> = issue::Entity::find()
            .select_only()
            .column(issue::Column::Id)
            .filter(issue::Column::Id.is_in(links.iter().map(|(id, _)| id.clone())))
            .filter(issue::Column::RemovedAt.is_null())
            .into_tuple::<String>()
            .all(db)
            .await?
            .into_iter()
            .collect();
        for (issue_id, cv) in links {
            if live.contains(&issue_id)
                && let Ok(cv) = cv.parse::<i64>()
            {
                held.insert(cv);
            }
        }
    }
    Ok(held)
}
