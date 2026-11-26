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
use entity::reading_list_break;
use entity::reading_list_link::{self, AnchorStatus, LinkKind};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult,
    QueryFilter, QueryOrder, QuerySelect,
};
use std::collections::HashMap;
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

    /// The key of a stored chapter break.
    pub fn of_break(b: &reading_list_break::Model) -> Self {
        Self {
            cv_issue_id: b.anchor_cv_issue_id,
            series: b.anchor_series.clone(),
            number: b.anchor_number.clone(),
            volume: b.anchor_volume.clone(),
            year: b.anchor_year.clone(),
        }
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

/// Every position in `entries` that `key` names, in list order; empty
/// when the issue is no longer in the list.
///
/// Tried in order, each step only when the previous found nothing:
/// 1. the ComicVine issue id;
/// 2. the exact (series, number, volume, year) key;
/// 3. series and number alone, case-insensitive, for a list that
///    re-tagged a volume or a year. This step never guesses: it needs
///    exactly one such entry, and that entry must not differ in **both**
///    volume and year (a re-tag changes one; another series of the same
///    name, such as Avengers #1 of 1963 and of 1998, changes both). When
///    it can't tell, the anchor is lost and the reader re-places it.
pub fn candidates(entries: &[EntryRef], key: &AnchorKey) -> Vec<i32> {
    if let Some(cv) = key.cv_issue_id {
        let hits: Vec<i32> = entries
            .iter()
            .filter(|e| e.cv_issue_id == Some(cv))
            .map(|e| e.position)
            .collect();
        if !hits.is_empty() {
            return hits;
        }
    }
    let (Some(series), Some(number)) = (key.series.as_deref(), key.number.as_deref()) else {
        return Vec::new();
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
        return exact;
    }
    let loose: Vec<&EntryRef> = entries
        .iter()
        .filter(|e| {
            e.series_name.eq_ignore_ascii_case(series)
                && e.issue_number.eq_ignore_ascii_case(number)
        })
        .collect();
    // Both known and different.
    let differs =
        |a: &Option<String>, b: &Option<String>| matches!((a, b), (Some(x), Some(y)) if x != y);
    match loose.as_slice() {
        [only] if !(differs(&only.volume, &key.volume) && differs(&only.year, &key.year)) => {
            vec![only.position]
        }
        _ => Vec::new(),
    }
}

/// The position of the entry `key` names in `entries`, or `None` when the
/// issue is no longer in the list. See [`candidates`] for the match.
///
/// An issue listed twice resolves to the occurrence nearest `hint` (the
/// position it last resolved to), else the first. Across a refresh use
/// [`resolve_nth`]: positions before and after a rewrite don't compare.
pub fn resolve(entries: &[EntryRef], key: &AnchorKey, hint: Option<i32>) -> Option<i32> {
    let matches = candidates(entries, key);
    match hint {
        Some(h) => matches.into_iter().min_by_key(|p| (p - h).abs()),
        None => matches.into_iter().min(),
    }
}

/// [`resolve`] for an issue listed more than once, by **which occurrence**
/// the anchor was on (0 = first) instead of by distance to an old
/// position. With fewer occurrences now, the last one.
pub fn resolve_nth(entries: &[EntryRef], key: &AnchorKey, nth: usize) -> Option<i32> {
    let matches = candidates(entries, key);
    matches.get(nth).or(matches.last()).copied()
}

/// Which occurrence of its issue each event link and chapter break of
/// `list_id` sits on, keyed by link / break id, read from `entries` (the
/// list **before** a rewrite). Only anchors on a repeated issue appear.
pub async fn anchor_occurrences(
    db: &impl ConnectionTrait,
    list_id: Uuid,
    entries: &[EntryRef],
) -> Result<HashMap<Uuid, usize>, sea_orm::DbErr> {
    let mut out = HashMap::new();
    let mut note = |id: Uuid, key: &AnchorKey, hint: Option<i32>| {
        let matches = candidates(entries, key);
        if matches.len() < 2 {
            return;
        }
        if let Some(nth) = hint.and_then(|h| matches.iter().position(|p| *p == h)) {
            out.insert(id, nth);
        }
    };
    let links = reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(list_id))
        .filter(reading_list_link::Column::Kind.eq(LinkKind::Event))
        .filter(reading_list_link::Column::AnchorStatus.eq(AnchorStatus::Placed))
        .all(db)
        .await?;
    for l in &links {
        if let Some(key) = AnchorKey::of_link(l) {
            note(l.id, &key, l.anchor_position_hint);
        }
    }
    let breaks = reading_list_break::Entity::find()
        .filter(reading_list_break::Column::ListId.eq(list_id))
        .filter(reading_list_break::Column::AnchorStatus.eq(AnchorStatus::Placed))
        .all(db)
        .await?;
    for b in &breaks {
        note(b.id, &AnchorKey::of_break(b), b.anchor_position_hint);
    }
    Ok(out)
}

/// What [`reresolve_parent`] changed (event links and chapter breaks).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReresolveSummary {
    /// Links / breaks whose anchor issue left the list in this pass.
    pub lost: i32,
    /// Lost links / breaks whose anchor issue came back in this pass.
    pub restored: i32,
}

/// The new status + hint for an anchor, and how it moves the summary.
/// `None` when nothing changed.
fn reresolved(
    entries: &[EntryRef],
    key: &AnchorKey,
    status: AnchorStatus,
    hint: Option<i32>,
    nth: Option<usize>,
    summary: &mut ReresolveSummary,
) -> Option<(AnchorStatus, Option<i32>)> {
    let found = match nth {
        Some(n) => resolve_nth(entries, key, n),
        None => resolve(entries, key, hint),
    };
    let (next, next_hint) = match found {
        Some(p) => (AnchorStatus::Placed, Some(p)),
        // Keep the last known place so the card stays where it was.
        None => (AnchorStatus::Lost, hint),
    };
    if next == status && next_hint == hint {
        return None;
    }
    match (status, next) {
        (AnchorStatus::Placed, AnchorStatus::Lost) => summary.lost += 1,
        (AnchorStatus::Lost, AnchorStatus::Placed) => summary.restored += 1,
        _ => {}
    }
    Some((next, next_hint))
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

/// Re-resolve every event link and chapter break of `list_id` against its
/// current entries and write back position hints and status. Call inside
/// the transaction that rewrote the entries, so a failed refresh leaves
/// them untouched.
///
/// `occurrences` is [`anchor_occurrences`] taken before the rewrite, so an
/// anchor on a twice-listed issue stays on the same occurrence.
pub async fn reresolve_parent(
    db: &impl ConnectionTrait,
    list_id: Uuid,
    occurrences: &HashMap<Uuid, usize>,
) -> Result<ReresolveSummary, sea_orm::DbErr> {
    let links = reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(list_id))
        .filter(reading_list_link::Column::Kind.eq(LinkKind::Event))
        .all(db)
        .await?;
    let breaks = reading_list_break::Entity::find()
        .filter(reading_list_break::Column::ListId.eq(list_id))
        .all(db)
        .await?;
    let mut summary = ReresolveSummary::default();
    if links.is_empty() && breaks.is_empty() {
        return Ok(summary);
    }
    let entries = load_entries(db, list_id).await?;
    let now = Utc::now().fixed_offset();
    for link in links {
        let Some(key) = AnchorKey::of_link(&link) else {
            continue;
        };
        let Some((status, hint)) = reresolved(
            &entries,
            &key,
            link.anchor_status,
            link.anchor_position_hint,
            occurrences.get(&link.id).copied(),
            &mut summary,
        ) else {
            continue;
        };
        let mut am: reading_list_link::ActiveModel = link.into();
        am.anchor_status = Set(status);
        am.anchor_position_hint = Set(hint);
        am.updated_at = Set(now);
        am.update(db).await?;
    }
    for b in breaks {
        let key = AnchorKey::of_break(&b);
        let Some((status, hint)) = reresolved(
            &entries,
            &key,
            b.anchor_status,
            b.anchor_position_hint,
            occurrences.get(&b.id).copied(),
            &mut summary,
        ) else {
            continue;
        };
        let mut am: reading_list_break::ActiveModel = b.into();
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
    fn a_repeated_issue_keeps_its_occurrence_across_a_rewrite() {
        // Before: the issue at 0 and at 9, the anchor on the second.
        let k = key("Avengers", "1", Some(7));
        // After 20 books are inserted near the top, they sit at 0 and 29.
        // Nearest the old position (9) would pick 0, the wrong one.
        let after = [
            e(0, "Avengers", "1", Some(7)),
            e(29, "Avengers", "1", Some(7)),
        ];
        assert_eq!(resolve(&after, &k, Some(9)), Some(0));
        assert_eq!(resolve_nth(&after, &k, 1), Some(29));
        // One occurrence left: that one.
        let one = [e(4, "Avengers", "1", Some(7))];
        assert_eq!(resolve_nth(&one, &k, 1), Some(4));
        assert_eq!(resolve_nth(&[], &k, 1), None);
    }

    #[test]
    fn the_loose_match_never_guesses() {
        let mut k = key("Avengers", "1", None);
        k.volume = Some("1963".into());
        k.year = Some("1963".into());
        // Another series of the same name: volume and year both differ.
        let mut other = e(3, "Avengers", "1", None);
        other.volume = Some("1998".into());
        other.year = Some("1998".into());
        assert_eq!(resolve(&[other.clone()], &k, Some(3)), None);
        // A re-tagged year alone still resolves.
        let mut retagged = e(3, "Avengers", "1", None);
        retagged.volume = Some("1963".into());
        retagged.year = Some("1964".into());
        assert_eq!(resolve(&[retagged.clone()], &k, None), Some(3));
        // Two loose candidates: no way to tell, so lost.
        let mut second = retagged.clone();
        second.position = 8;
        second.volume = Some("1".into());
        assert_eq!(resolve(&[retagged, second], &k, None), None);
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
