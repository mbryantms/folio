//! Linked reading lists: where in a parent list each linked list sits.
//!
//! A `reading_list_links` row of kind `event` names its place by the
//! **issue** it is read after, never by a `cbl_entries` row: a refresh
//! ([`super::import::apply_parsed`]) deletes and rewrites every entry, so
//! an entry id would dangle after the first refresh. The key is the
//! ComicVine issue id when the list carries one, else the
//! (series, number, volume, year) key refresh already uses to carry manual
//! matches across.
//!
//! [`resolve`] is pure and answers "which position is that issue at now?".
//! [`reresolve_parent`] runs it for every event link of a list after its
//! entries change and records the result on the row: `anchor_position_hint`
//! follows the issue, and `anchor_status` turns `lost` when the issue is no
//! longer in the list (the row is kept, drawn at the last known position,
//! and turns `placed` again if a later refresh brings the issue back).

use chrono::Utc;
use entity::cbl_entry;
use entity::reading_list_link::{self, AnchorStatus, LinkKind};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult,
    QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

/// The identity of the issue a link is read after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorKey {
    pub cv_issue_id: Option<i32>,
    pub series: Option<String>,
    pub number: Option<String>,
    pub volume: Option<String>,
    pub year: Option<String>,
}

impl AnchorKey {
    /// The key of a stored link (`None` for a `continues` link).
    pub fn of_link(link: &reading_list_link::Model) -> Option<Self> {
        if link.kind != LinkKind::Event {
            return None;
        }
        Some(Self {
            cv_issue_id: link.anchor_cv_issue_id,
            series: link.anchor_series.clone(),
            number: link.anchor_number.clone(),
            volume: link.anchor_volume.clone(),
            year: link.anchor_year.clone(),
        })
    }

    /// The key that identifies `entry`: both halves are stored, so a list
    /// that later drops its ComicVine ids still resolves by name.
    pub fn of_entry(entry: &EntryRef) -> Self {
        Self {
            cv_issue_id: entry.cv_issue_id,
            series: Some(entry.series_name.clone()),
            number: Some(entry.issue_number.clone()),
            volume: entry.volume.clone(),
            year: entry.year.clone(),
        }
    }
}

/// The columns of a `cbl_entries` row the resolver reads.
#[derive(Debug, Clone, FromQueryResult)]
pub struct EntryRef {
    pub position: i32,
    pub series_name: String,
    pub issue_number: String,
    pub volume: Option<String>,
    pub year: Option<String>,
    pub cv_issue_id: Option<i32>,
}

/// The position of the entry `key` names in `entries`, or `None` when the
/// issue is no longer in the list.
///
/// Tried in order, each step only when the previous found nothing:
/// 1. the ComicVine issue id;
/// 2. the exact (series, number, volume, year) key;
/// 3. series and number alone, case-insensitive — a list that re-tagged a
///    volume or year still finds the issue.
///
/// An issue listed twice resolves to the occurrence nearest `hint` (the
/// position it last resolved to), else the first.
pub fn resolve(entries: &[EntryRef], key: &AnchorKey, hint: Option<i32>) -> Option<i32> {
    let pick = |matches: Vec<i32>| -> Option<i32> {
        match hint {
            Some(h) => matches.into_iter().min_by_key(|p| (p - h).abs()),
            None => matches.into_iter().min(),
        }
    };
    if let Some(cv) = key.cv_issue_id {
        let hits: Vec<i32> = entries
            .iter()
            .filter(|e| e.cv_issue_id == Some(cv))
            .map(|e| e.position)
            .collect();
        if !hits.is_empty() {
            return pick(hits);
        }
    }
    let (Some(series), Some(number)) = (key.series.as_deref(), key.number.as_deref()) else {
        return None;
    };
    let exact: Vec<i32> = entries
        .iter()
        .filter(|e| {
            e.series_name == series
                && e.issue_number == number
                && e.volume == key.volume
                && e.year == key.year
        })
        .map(|e| e.position)
        .collect();
    if !exact.is_empty() {
        return pick(exact);
    }
    let loose: Vec<i32> = entries
        .iter()
        .filter(|e| {
            e.series_name.eq_ignore_ascii_case(series)
                && e.issue_number.eq_ignore_ascii_case(number)
        })
        .map(|e| e.position)
        .collect();
    pick(loose)
}

/// What [`reresolve_parent`] changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReresolveSummary {
    /// Links whose anchor issue left the list in this pass.
    pub lost: i32,
    /// Lost links whose anchor issue came back in this pass.
    pub restored: i32,
}

/// The parent's entries as [`EntryRef`]s, in position order.
pub async fn load_entries(
    db: &impl ConnectionTrait,
    list_id: Uuid,
) -> Result<Vec<EntryRef>, sea_orm::DbErr> {
    cbl_entry::Entity::find()
        .select_only()
        .columns([
            cbl_entry::Column::Position,
            cbl_entry::Column::SeriesName,
            cbl_entry::Column::IssueNumber,
            cbl_entry::Column::Volume,
            cbl_entry::Column::Year,
            cbl_entry::Column::CvIssueId,
        ])
        .filter(cbl_entry::Column::CblListId.eq(list_id))
        .order_by_asc(cbl_entry::Column::Position)
        .into_model::<EntryRef>()
        .all(db)
        .await
}

/// Re-resolve every event link of `list_id` against its current entries
/// and write back position hints and status. Call inside the transaction
/// that rewrote the entries, so a failed refresh leaves links untouched.
pub async fn reresolve_parent(
    db: &impl ConnectionTrait,
    list_id: Uuid,
) -> Result<ReresolveSummary, sea_orm::DbErr> {
    let links = reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(list_id))
        .filter(reading_list_link::Column::Kind.eq(LinkKind::Event))
        .all(db)
        .await?;
    let mut summary = ReresolveSummary::default();
    if links.is_empty() {
        return Ok(summary);
    }
    let entries = load_entries(db, list_id).await?;
    let now = Utc::now().fixed_offset();
    for link in links {
        let Some(key) = AnchorKey::of_link(&link) else {
            continue;
        };
        let resolved = resolve(&entries, &key, link.anchor_position_hint);
        let (status, hint) = match resolved {
            Some(p) => (AnchorStatus::Placed, Some(p)),
            // Keep the last known place so the card stays where it was.
            None => (AnchorStatus::Lost, link.anchor_position_hint),
        };
        if status == link.anchor_status && hint == link.anchor_position_hint {
            continue;
        }
        match (link.anchor_status, status) {
            (AnchorStatus::Placed, AnchorStatus::Lost) => summary.lost += 1,
            (AnchorStatus::Lost, AnchorStatus::Placed) => summary.restored += 1,
            _ => {}
        }
        let mut am: reading_list_link::ActiveModel = link.into();
        am.anchor_status = Set(status);
        am.anchor_position_hint = Set(hint);
        am.updated_at = Set(now);
        am.update(db).await?;
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(position: i32, series: &str, number: &str, cv: Option<i32>) -> EntryRef {
        EntryRef {
            position,
            series_name: series.into(),
            issue_number: number.into(),
            volume: Some("2004".into()),
            year: Some("2005".into()),
            cv_issue_id: cv,
        }
    }

    fn key(series: &str, number: &str, cv: Option<i32>) -> AnchorKey {
        AnchorKey {
            cv_issue_id: cv,
            series: Some(series.into()),
            number: Some(number.into()),
            volume: Some("2004".into()),
            year: Some("2005".into()),
        }
    }

    #[test]
    fn comicvine_id_wins_over_name() {
        let entries = [
            e(0, "Excalibur", "12", Some(1)),
            e(1, "Renamed", "12", Some(2)),
        ];
        assert_eq!(
            resolve(&entries, &key("Excalibur", "12", Some(2)), None),
            Some(1)
        );
    }

    #[test]
    fn falls_back_to_exact_key_then_loose_name() {
        let entries = [e(0, "Excalibur", "11", None), e(1, "Excalibur", "12", None)];
        assert_eq!(
            resolve(&entries, &key("Excalibur", "12", Some(99)), None),
            Some(1)
        );
        let mut retagged = key("excalibur", "12", None);
        retagged.volume = Some("2005".into());
        assert_eq!(resolve(&entries, &retagged, None), Some(1));
    }

    #[test]
    fn follows_a_reorder_and_reports_a_removal() {
        let before = [e(0, "A", "1", Some(10)), e(1, "B", "1", Some(20))];
        let after = [
            e(0, "B", "1", Some(20)),
            e(1, "C", "1", Some(30)),
            e(2, "A", "1", Some(10)),
        ];
        let k = key("A", "1", Some(10));
        assert_eq!(resolve(&before, &k, None), Some(0));
        assert_eq!(resolve(&after, &k, Some(0)), Some(2));
        assert_eq!(resolve(&after, &key("Z", "9", Some(99)), Some(1)), None);
    }

    #[test]
    fn a_repeated_issue_resolves_nearest_the_hint() {
        let entries = [
            e(0, "A", "1", Some(10)),
            e(5, "B", "1", None),
            e(9, "A", "1", Some(10)),
        ];
        let k = key("A", "1", Some(10));
        assert_eq!(resolve(&entries, &k, None), Some(0));
        assert_eq!(resolve(&entries, &k, Some(8)), Some(9));
    }

    #[test]
    fn a_key_without_name_or_id_resolves_nothing() {
        let entries = [e(0, "A", "1", None)];
        let empty = AnchorKey {
            cv_issue_id: None,
            series: None,
            number: None,
            volume: None,
            year: None,
        };
        assert_eq!(resolve(&entries, &empty, None), None);
    }
}
