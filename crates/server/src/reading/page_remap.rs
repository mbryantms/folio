//! Remap per-user page anchors — markers and reading progress — after an
//! archive's page list changes shape.
//!
//! Every anchor is an `(issue_id, page_index)` ordinal. The archive page
//! editor (`jobs::archive_edit`) removes and reorders pages, so without a
//! remap every bookmark, note, highlight, and resume position after the
//! edited ordinal silently points at different pixels (audit DI-19). The
//! editor already simulates its ops to validate them; the same simulation
//! yields the old→new ordinal map this module applies inside the edit's
//! own transaction.
//!
//! A removed page's anchors are not deleted: markers move to the nearest
//! surviving page and gain the [`PAGE_REMOVED_TAG`] tag so the Bookmarks
//! page can surface them; progress moves the same way. Nothing here
//! touches archive bytes.

use entity::{marker, progress_record};
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};

/// Tag appended to a marker whose page was removed by an archive edit.
/// The marker is moved to the nearest surviving page so it stays
/// reachable; the tag is the visible signal that its pixels are gone.
pub const PAGE_REMOVED_TAG: &str = "page-removed";

/// Hard cap on `markers.tags` (mirrors `api::markers::MAX_TAGS`). A
/// marker already carrying this many tags cannot take the removed tag;
/// it is still moved.
const MAX_TAGS: usize = 32;

/// Old-ordinal → new-ordinal map for one edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageMap {
    /// `old_to_new[old] = Some(new)`; `None` when the page was removed.
    old_to_new: Vec<Option<usize>>,
    new_len: usize,
}

impl PageMap {
    /// Build from the final slot list an op simulation produces, where
    /// `slots[new] == old` (see `jobs::archive_edit::simulate_slots`).
    pub fn from_slots(before: usize, slots: &[usize]) -> Self {
        let mut old_to_new = vec![None; before];
        for (new, &old) in slots.iter().enumerate() {
            if let Some(slot) = old_to_new.get_mut(old) {
                *slot = Some(new);
            }
        }
        Self {
            old_to_new,
            new_len: slots.len(),
        }
    }

    /// A pure truncation: pages `0..after` keep their ordinal, pages at
    /// or past `after` are gone. Used when a rescan finds the page count
    /// shrank without an edit we know about (external replacement).
    pub fn truncation(before: usize, after: usize) -> Self {
        let old_to_new = (0..before)
            .map(|old| (old < after).then_some(old))
            .collect();
        Self {
            old_to_new,
            new_len: after.min(before),
        }
    }

    pub fn new_len(&self) -> usize {
        self.new_len
    }

    /// True when no ordinal changes and nothing was removed.
    pub fn is_identity(&self) -> bool {
        self.old_to_new.len() == self.new_len
            && self
                .old_to_new
                .iter()
                .enumerate()
                .all(|(old, new)| *new == Some(old))
    }

    /// Where an old ordinal lands, and whether its page was removed.
    ///
    /// Removed pages resolve to the nearest surviving page, searching
    /// downward first (the page you were reading before the removed one)
    /// and then upward. Ordinals past the old page count clamp to the
    /// last page. Always returns a valid ordinal for a non-empty
    /// archive.
    pub fn resolve(&self, old: i32) -> (i32, bool) {
        if self.new_len == 0 {
            return (0, true);
        }
        let last_new = (self.new_len - 1) as i32;
        if old < 0 {
            return (0, false);
        }
        let old = old as usize;
        if old >= self.old_to_new.len() {
            return (last_new, false);
        }
        if let Some(new) = self.old_to_new[old] {
            return (new as i32, false);
        }
        let below = (0..old).rev().find_map(|o| self.old_to_new[o]);
        let above = (old + 1..self.old_to_new.len()).find_map(|o| self.old_to_new[o]);
        let target = below.or(above).unwrap_or(0);
        (target as i32, true)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct RemapOutcome {
    /// Markers whose `page_index` changed.
    pub markers_moved: usize,
    /// Markers whose page was removed (moved to a neighbour + tagged).
    pub markers_orphaned: usize,
    /// Progress rows whose `last_page` or `percent` changed.
    pub progress_moved: usize,
}

/// Apply `map` to every marker and progress row anchored on `issue_id`.
/// Call inside the transaction that records the edit so the archive
/// bytes and the anchors change together.
pub async fn remap_issue_anchors<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    map: &PageMap,
) -> Result<RemapOutcome, sea_orm::DbErr> {
    let mut outcome = RemapOutcome::default();
    if map.is_identity() {
        return Ok(outcome);
    }
    let now = chrono::Utc::now().fixed_offset();

    let markers = marker::Entity::find()
        .filter(marker::Column::IssueId.eq(issue_id))
        .all(db)
        .await?;
    for m in markers {
        let (new_index, removed) = map.resolve(m.page_index);
        let needs_tag = removed && !m.tags.iter().any(|t| t == PAGE_REMOVED_TAG);
        if new_index == m.page_index && !needs_tag {
            continue;
        }
        if new_index != m.page_index {
            outcome.markers_moved += 1;
        }
        if removed {
            outcome.markers_orphaned += 1;
        }
        let mut tags = m.tags.clone();
        if needs_tag && tags.len() < MAX_TAGS {
            tags.push(PAGE_REMOVED_TAG.to_owned());
        }
        let mut am: marker::ActiveModel = m.into();
        am.page_index = Set(new_index);
        am.tags = Set(tags);
        am.updated_at = Set(now);
        am.update(db).await?;
    }

    let progress = progress_record::Entity::find()
        .filter(progress_record::Column::IssueId.eq(issue_id))
        .all(db)
        .await?;
    let new_len = map.new_len().max(1) as f64;
    for p in progress {
        let (new_last, _) = map.resolve(p.last_page);
        let new_percent = (f64::from(new_last) / new_len).clamp(0.0, 1.0);
        if new_last == p.last_page && (new_percent - p.percent).abs() < f64::EPSILON {
            continue;
        }
        outcome.progress_moved += 1;
        let mut am: progress_record::ActiveModel = p.into();
        am.last_page = Set(new_last);
        am.percent = Set(new_percent);
        am.updated_at = Set(now);
        am.update(db).await?;
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_map_changes_nothing() {
        let map = PageMap::from_slots(4, &[0, 1, 2, 3]);
        assert!(map.is_identity());
        assert_eq!(map.resolve(2), (2, false));
    }

    #[test]
    fn remove_then_reorder_maps_survivors_and_orphans_to_neighbour() {
        // Remove ordinal 1 → [0,2,3]; reorder [2,0,1] → [3,0,2].
        let map = PageMap::from_slots(4, &[3, 0, 2]);
        assert!(!map.is_identity());
        assert_eq!(map.new_len(), 3);
        assert_eq!(map.resolve(0), (1, false));
        assert_eq!(map.resolve(2), (2, false));
        assert_eq!(map.resolve(3), (0, false));
        // Removed page 1 → nearest surviving below is old 0 → new 1.
        assert_eq!(map.resolve(1), (1, true));
    }

    #[test]
    fn removed_first_page_falls_upward() {
        // Remove ordinal 0 → survivors [1,2].
        let map = PageMap::from_slots(3, &[1, 2]);
        assert_eq!(map.resolve(0), (0, true));
        assert_eq!(map.resolve(1), (0, false));
    }

    #[test]
    fn out_of_range_clamps_to_last_page() {
        let map = PageMap::from_slots(3, &[0, 1, 2]);
        assert_eq!(map.resolve(9), (2, false));
        assert_eq!(map.resolve(-1), (0, false));
    }

    #[test]
    fn truncation_marks_trailing_pages_removed() {
        let map = PageMap::truncation(5, 3);
        assert_eq!(map.new_len(), 3);
        assert_eq!(map.resolve(1), (1, false));
        assert_eq!(map.resolve(4), (2, true));
    }
}
