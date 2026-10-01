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
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, Set,
};

/// Tag appended to a marker whose page was removed by an archive edit.
/// The marker is moved to the nearest surviving page so it stays
/// reachable; the tag is the visible signal that its pixels are gone.
pub const PAGE_REMOVED_TAG: &str = "page-removed";

/// Tag appended to a marker whose page image no longer exists anywhere
/// in a replaced archive (WP-6.2's drift note): its page hash matched no
/// page, so it stayed on its old ordinal, which may now show different
/// pixels. Cleared again if a later rescan finds the image.
pub const PAGE_DRIFT_TAG: &str = "page-drift";

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

    /// The ordinal guess for a page list that changed without an edit we
    /// know about (external replacement): pages `0..after` keep their
    /// ordinal, pages at or past `after` are gone. A truncation when the
    /// count shrank, the identity (over `after` pages) otherwise.
    pub fn truncation(before: usize, after: usize) -> Self {
        let old_to_new = (0..before)
            .map(|old| (old < after).then_some(old))
            .collect();
        Self {
            old_to_new,
            new_len: after,
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
    /// Anchors (markers + progress) that a page hash placed on a
    /// different ordinal than the ordinal map would have (WP-6.2).
    pub resolved_by_hash: usize,
    /// Markers whose page hash matched no page in the new archive; they
    /// fell back to the ordinal map and gained [`PAGE_DRIFT_TAG`].
    pub markers_drifted: usize,
}

/// Which source of truth wins when re-anchoring an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Authority {
    /// Folio performed the edit and knows the exact old→new map
    /// (`jobs::archive_edit`). The map wins; anchors are re-stamped with
    /// the hash of whatever image now sits at their new ordinal, so a
    /// rotated or replaced page carries its markers along.
    Ordinal,
    /// The bytes changed under us (rescan of a replaced archive). The
    /// page hash wins; the ordinal map is only the fallback for anchors
    /// without a hash, or whose image is gone (WP-6.2).
    Hash,
}

/// Apply `map` to every marker and progress row anchored on `issue_id`.
/// Ordinal-only (WP-1.2); equivalent to [`reanchor_issue`] with no page
/// hashes. Call inside the transaction that records the edit so the
/// archive bytes and the anchors change together.
pub async fn remap_issue_anchors<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    map: &PageMap,
) -> Result<RemapOutcome, sea_orm::DbErr> {
    reanchor_issue(db, issue_id, map, None, Authority::Ordinal).await
}

/// Where one anchor lands.
#[derive(Debug, PartialEq, Eq)]
struct Placement {
    index: i32,
    /// The ordinal map removed the anchor's page (and no hash rescued it).
    removed: bool,
    /// The anchor's hash matched no page; the ordinal is a guess.
    drifted: bool,
    /// A hash match placed it somewhere the ordinal map would not have.
    by_hash: bool,
    /// The anchor's hash was found in the new archive.
    matched: bool,
    hash: Option<String>,
}

/// The candidate ordinal holding `hash` nearest to `near` (duplicate
/// images — blank pages, repeated ads — resolve to the closest copy).
fn nearest_match(hashes: &[String], hash: &str, near: i32) -> Option<i32> {
    hashes
        .iter()
        .enumerate()
        .filter(|(_, h)| h.as_str() == hash)
        .map(|(i, _)| i as i32)
        .min_by_key(|i| (i - near).abs())
}

fn place(
    map: &PageMap,
    old: i32,
    old_hash: Option<&str>,
    new_hashes: Option<&[String]>,
    authority: Authority,
) -> Placement {
    let (ord_index, ord_removed) = map.resolve(old);
    let ordinal = |hash: Option<String>| Placement {
        index: ord_index,
        removed: ord_removed,
        drifted: false,
        by_hash: false,
        matched: false,
        hash,
    };
    match (authority, new_hashes) {
        // Exact map; stamp the image now at the new ordinal (or clear the
        // hash when the new bytes couldn't be hashed — a stale hash would
        // read as drift on the next rescan).
        (Authority::Ordinal, hashes) => {
            ordinal(hashes.and_then(|h| h.get(ord_index as usize).cloned()))
        }
        (Authority::Hash, Some(hashes)) => match old_hash {
            Some(h) => match nearest_match(hashes, h, ord_index) {
                Some(index) => Placement {
                    index,
                    removed: false,
                    drifted: false,
                    by_hash: index != ord_index,
                    matched: true,
                    hash: Some(h.to_owned()),
                },
                // The image is gone. Keep the ordinal guess and the old
                // hash: if a later replacement restores the image, the
                // next rescan re-resolves onto it.
                None => Placement {
                    drifted: !ord_removed,
                    ..ordinal(Some(h.to_owned()))
                },
            },
            // Legacy row: ordinal only. The hash stays unknown — stamping
            // the image at the guessed ordinal would bless the guess.
            None => ordinal(None),
        },
        (Authority::Hash, None) => ordinal(old_hash.map(str::to_owned)),
    }
}

/// Re-anchor every marker and progress row on `issue_id` after the
/// archive's page list changed.
///
/// `map` is the ordinal map (an exact edit map, or a truncation guess
/// for an external replacement); `new_hashes` is the page-hash list of
/// the archive as it is now, when it could be read. See [`Authority`]
/// for which wins. Anchors whose page was removed land on a neighbour
/// with [`PAGE_REMOVED_TAG`]; markers whose image vanished while their
/// ordinal survived gain [`PAGE_DRIFT_TAG`] (the drift note). A hash
/// re-resolution clears a previous drift tag.
///
/// Moves bump `updated_at`; a hash-only re-stamp does not (it is not a
/// user-visible change and must not wake sync clients).
pub async fn reanchor_issue<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    map: &PageMap,
    new_hashes: Option<&[String]>,
    authority: Authority,
) -> Result<RemapOutcome, sea_orm::DbErr> {
    let mut outcome = RemapOutcome::default();
    if map.is_identity() && new_hashes.is_none() {
        return Ok(outcome);
    }
    let now = chrono::Utc::now().fixed_offset();

    let markers = marker::Entity::find()
        .filter(marker::Column::IssueId.eq(issue_id))
        .all(db)
        .await?;
    for m in markers {
        let p = place(
            map,
            m.page_index,
            m.page_hash.as_deref(),
            new_hashes,
            authority,
        );
        let mut tags = m.tags.clone();
        if p.removed && !tags.iter().any(|t| t == PAGE_REMOVED_TAG) && tags.len() < MAX_TAGS {
            tags.push(PAGE_REMOVED_TAG.to_owned());
        }
        if p.drifted && !tags.iter().any(|t| t == PAGE_DRIFT_TAG) && tags.len() < MAX_TAGS {
            tags.push(PAGE_DRIFT_TAG.to_owned());
        }
        if p.matched {
            tags.retain(|t| t != PAGE_DRIFT_TAG);
        }
        let moved = p.index != m.page_index;
        let retagged = tags != m.tags;
        if moved {
            outcome.markers_moved += 1;
        }
        if p.removed {
            outcome.markers_orphaned += 1;
        }
        if p.drifted {
            outcome.markers_drifted += 1;
        }
        if p.by_hash && moved {
            outcome.resolved_by_hash += 1;
        }
        if !moved && !retagged && p.hash == m.page_hash {
            continue;
        }
        let bump = moved || retagged;
        let mut am: marker::ActiveModel = m.into();
        am.page_index = Set(p.index);
        am.tags = Set(tags);
        am.page_hash = Set(p.hash);
        if bump {
            am.updated_at = Set(now);
        }
        am.update(db).await?;
    }

    let progress = progress_record::Entity::find()
        .filter(progress_record::Column::IssueId.eq(issue_id))
        .all(db)
        .await?;
    let new_len = map.new_len().max(1) as f64;
    for r in progress {
        let p = place(
            map,
            r.last_page,
            r.page_hash.as_deref(),
            new_hashes,
            authority,
        );
        let new_percent = (f64::from(p.index) / new_len).clamp(0.0, 1.0);
        let moved = p.index != r.last_page || (new_percent - r.percent).abs() >= f64::EPSILON;
        if !moved && p.hash == r.page_hash {
            continue;
        }
        if moved {
            outcome.progress_moved += 1;
        }
        if p.by_hash && p.index != r.last_page {
            outcome.resolved_by_hash += 1;
        }
        let mut am: progress_record::ActiveModel = r.into();
        am.last_page = Set(p.index);
        am.percent = Set(new_percent);
        am.page_hash = Set(p.hash);
        if moved {
            am.updated_at = Set(now);
        }
        am.update(db).await?;
    }

    Ok(outcome)
}

/// True when any marker or progress row on the issue carries a page
/// hash — the only case a rescan needs to hash the new archive. One
/// round trip, served by the `*_issue_hashed_idx` partial indexes.
pub async fn issue_has_hashed_anchors<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
) -> Result<bool, sea_orm::DbErr> {
    let stmt = sea_orm::Statement::from_sql_and_values(
        db.get_database_backend(),
        "SELECT (EXISTS (SELECT 1 FROM markers \
                          WHERE issue_id = $1 AND page_hash IS NOT NULL) \
              OR EXISTS (SELECT 1 FROM progress_records \
                          WHERE issue_id = $1 AND page_hash IS NOT NULL)) AS hashed",
        [issue_id.into()],
    );
    let row = db.query_one_raw(stmt).await?;
    Ok(match row {
        Some(r) => r.try_get::<bool>("", "hashed")?,
        None => false,
    })
}

/// True when the issue has any marker or progress row at all — an
/// archive edit only hashes the rewritten archive when there is
/// something to re-stamp.
pub async fn issue_has_anchors<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
) -> Result<bool, sea_orm::DbErr> {
    let m = marker::Entity::find()
        .filter(marker::Column::IssueId.eq(issue_id))
        .count(db)
        .await?;
    if m > 0 {
        return Ok(true);
    }
    let p = progress_record::Entity::find()
        .filter(progress_record::Column::IssueId.eq(issue_id))
        .count(db)
        .await?;
    Ok(p > 0)
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

    fn hashes(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn hash_authority_follows_the_image_over_the_ordinal() {
        let map = PageMap::truncation(4, 4);
        let new = hashes(&["c", "d", "a", "b"]);
        let p = place(&map, 1, Some("b"), Some(&new), Authority::Hash);
        assert_eq!(
            (p.index, p.by_hash, p.matched, p.drifted),
            (3, true, true, false)
        );
    }

    #[test]
    fn duplicate_images_resolve_to_the_nearest_copy() {
        let map = PageMap::truncation(5, 5);
        let new = hashes(&["blank", "x", "y", "blank", "z"]);
        assert_eq!(
            place(&map, 4, Some("blank"), Some(&new), Authority::Hash).index,
            3
        );
        assert_eq!(
            place(&map, 1, Some("blank"), Some(&new), Authority::Hash).index,
            0
        );
    }

    #[test]
    fn missing_image_falls_back_to_the_ordinal_and_drifts() {
        let map = PageMap::truncation(4, 4);
        let new = hashes(&["a", "z", "c", "d"]);
        let p = place(&map, 1, Some("b"), Some(&new), Authority::Hash);
        assert_eq!((p.index, p.drifted, p.removed), (1, true, false));
        assert_eq!(
            p.hash.as_deref(),
            Some("b"),
            "old hash kept for a later restore"
        );
        // Past the new end: removed, not drifted (one tag, not two).
        let map = PageMap::truncation(4, 2);
        let p = place(&map, 3, Some("q"), Some(&new[..2]), Authority::Hash);
        assert_eq!((p.index, p.drifted, p.removed), (1, false, true));
    }

    #[test]
    fn legacy_anchor_without_hash_stays_unhashed_under_hash_authority() {
        let map = PageMap::truncation(4, 4);
        let new = hashes(&["a", "b", "c", "d"]);
        let p = place(&map, 2, None, Some(&new), Authority::Hash);
        assert_eq!((p.index, p.hash), (2, None));
    }

    #[test]
    fn ordinal_authority_restamps_from_the_new_bytes() {
        let map = PageMap::from_slots(3, &[2, 0, 1]);
        let new = hashes(&["c2", "a2", "b2"]);
        let p = place(&map, 0, Some("a"), Some(&new), Authority::Ordinal);
        assert_eq!((p.index, p.hash.as_deref()), (1, Some("a2")));
        // Without new hashes the stale hash is cleared, not kept.
        let p = place(&map, 0, Some("a"), None, Authority::Ordinal);
        assert_eq!(p.hash, None);
    }

    #[test]
    fn truncation_with_growth_is_identity_over_the_old_pages() {
        let map = PageMap::truncation(3, 5);
        assert_eq!(map.new_len(), 5);
        assert_eq!(map.resolve(2), (2, false));
        assert!(!map.is_identity());
    }
}
