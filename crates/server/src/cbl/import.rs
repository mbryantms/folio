//! Persist a parsed CBL: write the list/entries rows + run the matcher
//! and capture the structural diff in `cbl_refresh_log`.
//!
//! Three call sites:
//!
//!   - `POST /me/cbl-lists` (handler) for first-time imports — there's
//!     no previous state, so the diff records "added: all entries".
//!   - `POST /me/cbl-lists/{id}/refresh` for manual / scheduled
//!     re-fetches — diff covers added / removed / reordered.
//!   - The post-scan rematch hook for re-resolving previously missing
//!     entries against newly-scanned issues — only the matcher runs;
//!     entries are unchanged.

use chrono::Utc;
use entity::{cbl_entry, cbl_list, cbl_refresh_log};
use parsers::cbl::ParsedCbl;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use uuid::Uuid;

use super::matcher::{self, EntryToMatch, MatchStatus};

/// Trigger that caused this run — recorded on the refresh-log row so
/// the History tab can color-code rows.
#[derive(Debug, Clone, Copy)]
pub enum RefreshTrigger {
    Manual,
    Scheduled,
    PostScan,
}

impl RefreshTrigger {
    fn as_str(self) -> &'static str {
        match self {
            RefreshTrigger::Manual => "manual",
            RefreshTrigger::Scheduled => "scheduled",
            RefreshTrigger::PostScan => "post_scan",
        }
    }
}

/// Outcome the API returns to the caller after an import / refresh run.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ImportSummary {
    pub list_id: Uuid,
    pub upstream_changed: bool,
    pub matched: i32,
    pub ambiguous: i32,
    pub missing: i32,
    pub manual: i32,
    pub added: i32,
    pub removed: i32,
    pub reordered: i32,
    pub rematched: i32,
}

/// Apply a freshly-fetched / freshly-parsed CBL to an existing
/// `cbl_lists` row. Replaces entries to mirror the new state, preserves
/// any `match_status='manual'` overrides on entries whose composite key
/// still exists, runs the matcher, and writes a `cbl_refresh_log` row
/// with the structural diff.
pub async fn apply_parsed(
    db: &impl ConnectionTrait,
    list_id: Uuid,
    parsed: &ParsedCbl,
    raw_xml: &str,
    blob_sha: Option<&str>,
    trigger: RefreshTrigger,
) -> Result<ImportSummary, sea_orm::DbErr> {
    let now = Utc::now().fixed_offset();

    // Snapshot existing entries so the diff has a "before" frame.
    let existing: Vec<cbl_entry::Model> = cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(list_id))
        .order_by_asc(cbl_entry::Column::Position)
        .all(db)
        .await?;

    // Map composite-key → existing entry; the key (series, number,
    // volume, year) survives reorders, so we can preserve manual
    // overrides + matched_issue_id where the entry is still present.
    let manual_by_key: HashMap<EntryKey, cbl_entry::Model> = existing
        .iter()
        .filter(|e| e.match_status == "manual")
        .map(|e| (key_of_entry(e), e.clone()))
        .collect();
    let prev_by_key: HashMap<EntryKey, cbl_entry::Model> = existing
        .iter()
        .map(|e| (key_of_entry(e), e.clone()))
        .collect();
    let prev_keys_in_order: Vec<EntryKey> = existing.iter().map(key_of_entry).collect();
    let new_keys_in_order: Vec<EntryKey> = parsed.books.iter().map(key_of_book).collect();

    // Replace entries wholesale. Cheaper than per-row UPSERT-by-key, and
    // the cascade FK from cbl_entries → cbl_lists handles cleanup
    // implicitly when we delete the list, but here we're keeping the
    // list row intact.
    cbl_entry::Entity::delete_many()
        .filter(cbl_entry::Column::CblListId.eq(list_id))
        .exec(db)
        .await?;
    #[cfg(debug_assertions)]
    if fail_after_delete_hit(list_id) {
        return Err(sea_orm::DbErr::Custom(
            "injected failure after the entry delete".into(),
        ));
    }

    // Run the matcher fresh on every refresh — even unchanged entries
    // can newly resolve when the scanner adds matching issues.
    let to_match: Vec<EntryToMatch> = parsed
        .books
        .iter()
        .map(|b| EntryToMatch {
            series_name: b.series.clone(),
            number: b.number.clone(),
            volume: b.volume.clone(),
            cv_issue_id: b.comicvine_issue_id(),
            metron_issue_id: b.metron_issue_id(),
        })
        .collect();
    let outcomes = matcher::match_entries(db, &to_match).await?;

    let mut summary = ImportSummary {
        list_id,
        upstream_changed: blob_sha
            .is_some_and(|sha| existing.iter().all(|_| true) && !sha.is_empty()),
        ..Default::default()
    };

    let diff = structural_diff(&prev_keys_in_order, &new_keys_in_order);
    let mut added_diffs = Vec::with_capacity(diff.added.len());
    let mut removed_diffs = Vec::with_capacity(diff.removed.len());
    let mut reordered_diffs = Vec::with_capacity(diff.reordered.len());
    for &i in &diff.added {
        summary.added += 1;
        added_diffs.push(diff_row(i as i32, &parsed.books[i]));
    }
    for &(i, prev_pos) in &diff.reordered {
        let book = &parsed.books[i];
        summary.reordered += 1;
        reordered_diffs.push(serde_json::json!({
            "position": i,
            "previous_position": prev_pos,
            "series": book.series,
            "number": book.number,
        }));
    }
    for &i in &diff.removed {
        if let Some(prev) = prev_by_key.get(&prev_keys_in_order[i]) {
            summary.removed += 1;
            removed_diffs.push(serde_json::json!({
                "position": i,
                "series": prev.series_name,
                "number": prev.issue_number,
            }));
        }
    }

    // Bulk insert the refreshed entries with their match outcomes.
    let mut inserts = Vec::with_capacity(parsed.books.len());
    for (i, (book, outcome)) in parsed.books.iter().zip(outcomes.iter()).enumerate() {
        let key = key_of_book(book);
        let manual_entry = manual_by_key.get(&key);

        let (
            status,
            method,
            confidence,
            candidates_json,
            matched_issue_id,
            matched_at,
            user_resolved_at,
        ) = if let Some(prev) = manual_entry {
            // Preserve manual overrides verbatim.
            (
                "manual".to_string(),
                Some("manual".to_string()),
                Some(1.0_f32),
                None::<sea_orm::JsonValue>,
                prev.matched_issue_id.clone(),
                prev.matched_at,
                prev.user_resolved_at,
            )
        } else {
            let prev_match = prev_by_key
                .get(&key)
                .filter(|p| p.match_status == "matched")
                .and_then(|p| p.matched_issue_id.clone());
            let now_match = outcome.issue_id.clone();
            if prev_match.is_some() && now_match.is_none() {
                // Used to match — now missing/ambiguous. The
                // `rematched_count` counter only increments for the
                // opposite transition (was missing → now matched),
                // so we don't bump it here.
            }
            if prev_match.is_none() && now_match.is_some() {
                summary.rematched += 1;
            }
            let candidates = outcome
                .ambiguous_candidates
                .as_ref()
                .map(|cs| serde_json::to_value(cs).unwrap_or(serde_json::json!([])));
            (
                outcome.status.as_str().to_string(),
                outcome.method.map(str::to_owned),
                outcome.confidence,
                candidates,
                outcome.issue_id.clone(),
                if matches!(outcome.status, MatchStatus::Matched) {
                    Some(now)
                } else {
                    None
                },
                None,
            )
        };

        match status.as_str() {
            "matched" => summary.matched += 1,
            "ambiguous" => summary.ambiguous += 1,
            "missing" => summary.missing += 1,
            "manual" => summary.manual += 1,
            _ => {}
        }

        inserts.push(cbl_entry::ActiveModel {
            id: Set(Uuid::now_v7()),
            cbl_list_id: Set(list_id),
            position: Set(i as i32),
            series_name: Set(book.series.clone()),
            issue_number: Set(book.number.clone()),
            volume: Set(book.volume.clone()),
            year: Set(book.year.clone()),
            cv_series_id: Set(book.comicvine_series_id()),
            cv_issue_id: Set(book.comicvine_issue_id()),
            metron_series_id: Set(book.metron_series_id()),
            metron_issue_id: Set(book.metron_issue_id()),
            matched_issue_id: Set(matched_issue_id),
            match_status: Set(status),
            match_method: Set(method),
            match_confidence: Set(confidence),
            ambiguous_candidates: Set(candidates_json),
            matched_at: Set(matched_at),
            user_resolved_at: Set(user_resolved_at),
        });
    }
    if !inserts.is_empty() {
        // Insert in batches to keep statements bounded.
        for chunk in inserts.chunks(500) {
            cbl_entry::Entity::insert_many(chunk.to_vec())
                .exec(db)
                .await?;
        }
    }

    // Record the structural diff.
    let prev_blob_sha = match cbl_list::Entity::find_by_id(list_id).one(db).await? {
        Some(m) => m.github_blob_sha.clone(),
        None => None,
    };
    let upstream_changed = match (prev_blob_sha.as_deref(), blob_sha) {
        (Some(prev), Some(new)) => prev != new,
        (None, Some(_)) => true,
        // Re-import without a blob SHA (upload / URL without ETag): if
        // the bytes' SHA-256 changed we treat that as upstream-changed,
        // computed by the caller and signaled via the prev-existing
        // count diff. Conservative default: assume changed when any
        // structural diff exists.
        _ => summary.added > 0 || summary.removed > 0 || summary.reordered > 0,
    };
    summary.upstream_changed = upstream_changed;
    let diff_summary = serde_json::json!({
        "added": added_diffs,
        "removed": removed_diffs,
        "reordered": reordered_diffs,
    });
    cbl_refresh_log::ActiveModel {
        id: Set(Uuid::now_v7()),
        cbl_list_id: Set(list_id),
        ran_at: Set(now),
        trigger: Set(trigger.as_str().to_owned()),
        upstream_changed: Set(upstream_changed),
        prev_blob_sha: Set(prev_blob_sha),
        new_blob_sha: Set(blob_sha.map(str::to_owned)),
        added_count: Set(summary.added),
        removed_count: Set(summary.removed),
        reordered_count: Set(summary.reordered),
        rematched_count: Set(summary.rematched),
        diff_summary: Set(Some(diff_summary)),
    }
    .insert(db)
    .await?;

    // Bump the list's last-refresh / last-match / blob fields.
    if let Some(list) = cbl_list::Entity::find_by_id(list_id).one(db).await? {
        let mut am: cbl_list::ActiveModel = list.into();
        am.raw_xml = Set(raw_xml.to_owned());
        am.raw_sha256 = Set(sha256_of(raw_xml.as_bytes()));
        am.parsed_name = Set(parsed.name.clone());
        am.parsed_matchers_present = Set(parsed.matchers_present);
        am.num_issues_declared = Set(parsed.num_issues_declared);
        if let Some(sha) = blob_sha {
            am.github_blob_sha = Set(Some(sha.to_owned()));
        }
        am.last_refreshed_at = Set(Some(now));
        am.last_match_run_at = Set(Some(now));
        am.updated_at = Set(now);
        am.update(db).await?;
    }

    Ok(summary)
}

// ───── debug-build fault hook ─────
//
// Lets an integration test fail `apply_parsed` right after the entry
// delete, to prove the caller's transaction leaves the stored list
// unchanged. Keyed by list id so concurrent tests don't interfere. Not
// compiled into release builds.

#[cfg(debug_assertions)]
static FAIL_AFTER_DELETE: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<Uuid>>> =
    std::sync::LazyLock::new(Default::default);

/// Make the next [`apply_parsed`] for `list_id` fail after deleting the
/// entries. Debug builds only; for integration tests.
#[cfg(debug_assertions)]
pub fn fail_after_delete_for_tests(list_id: Uuid) {
    FAIL_AFTER_DELETE
        .lock()
        .expect("fault hook lock")
        .insert(list_id);
}

#[cfg(debug_assertions)]
fn fail_after_delete_hit(list_id: Uuid) -> bool {
    FAIL_AFTER_DELETE
        .lock()
        .expect("fault hook lock")
        .remove(&list_id)
}

/// Run the matcher against the existing `cbl_entries` rows without
/// re-importing the file. Used by the post-scan rematch hook so newly-
/// indexed issues lift previously-missing entries into `matched`.
/// Returns the count of entries that transitioned `missing|ambiguous →
/// matched` so the History tab can show a meaningful counter.
pub async fn rematch_existing(
    db: &sea_orm::DatabaseConnection,
    list_id: Uuid,
    trigger: RefreshTrigger,
) -> Result<i32, sea_orm::DbErr> {
    let now = Utc::now().fixed_offset();
    let entries: Vec<cbl_entry::Model> = cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(list_id))
        .order_by_asc(cbl_entry::Column::Position)
        .all(db)
        .await?;
    if entries.is_empty() {
        return Ok(0);
    }
    let to_match: Vec<EntryToMatch> = entries
        .iter()
        .map(|e| EntryToMatch {
            series_name: e.series_name.clone(),
            number: e.issue_number.clone(),
            volume: e.volume.clone(),
            cv_issue_id: e.cv_issue_id,
            metron_issue_id: e.metron_issue_id,
        })
        .collect();
    let outcomes = matcher::match_entries(db, &to_match).await?;
    let txn = db.begin().await?;
    let mut rematched = 0;
    for (entry, outcome) in entries.iter().zip(outcomes.iter()) {
        if entry.match_status == "manual" {
            continue;
        }
        let was_unmatched = entry.match_status != "matched";
        let now_matched = matches!(outcome.status, MatchStatus::Matched);
        let mut am: cbl_entry::ActiveModel = entry.clone().into();
        am.match_status = Set(outcome.status.as_str().to_owned());
        am.match_method = Set(outcome.method.map(str::to_owned));
        am.match_confidence = Set(outcome.confidence);
        am.matched_issue_id = Set(outcome.issue_id.clone());
        am.ambiguous_candidates = Set(outcome
            .ambiguous_candidates
            .as_ref()
            .map(|cs| serde_json::to_value(cs).unwrap_or(serde_json::json!([]))));
        if now_matched {
            am.matched_at = Set(Some(now));
            if was_unmatched {
                rematched += 1;
            }
        }
        am.update(&txn).await?;
    }
    if let Some(list) = cbl_list::Entity::find_by_id(list_id).one(&txn).await? {
        let mut am: cbl_list::ActiveModel = list.into();
        am.last_match_run_at = Set(Some(now));
        am.update(&txn).await?;
    }
    if rematched > 0 {
        cbl_refresh_log::ActiveModel {
            id: Set(Uuid::now_v7()),
            cbl_list_id: Set(list_id),
            ran_at: Set(now),
            trigger: Set(trigger.as_str().to_owned()),
            upstream_changed: Set(false),
            prev_blob_sha: Set(None),
            new_blob_sha: Set(None),
            added_count: Set(0),
            removed_count: Set(0),
            reordered_count: Set(0),
            rematched_count: Set(rematched),
            diff_summary: Set(None),
        }
        .insert(&txn)
        .await?;
    }
    txn.commit().await?;
    Ok(rematched)
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
struct EntryKey {
    series: String,
    number: String,
    volume: Option<String>,
    year: Option<String>,
}

/// Positions that changed between the stored entry keys (`prev`) and the
/// fetched ones (`new`), each list in ascending position order.
#[derive(Debug, Default, PartialEq, Eq)]
struct StructuralDiff {
    /// New positions whose key is not among the stored entries.
    added: Vec<usize>,
    /// `(new position, previous position)` for keys that were stored but
    /// now sit elsewhere. The previous position is the key's first stored
    /// occurrence, so a duplicated key compares against its first copy.
    reordered: Vec<(usize, usize)>,
    /// Stored positions whose key is gone from the new file.
    removed: Vec<usize>,
}

/// O(n) diff: one index of each key's first stored position, one set of
/// the new keys.
fn structural_diff(prev: &[EntryKey], new: &[EntryKey]) -> StructuralDiff {
    let mut first_prev_pos: HashMap<&EntryKey, usize> = HashMap::with_capacity(prev.len());
    for (i, k) in prev.iter().enumerate() {
        first_prev_pos.entry(k).or_insert(i);
    }
    let new_keyset: std::collections::HashSet<&EntryKey> = new.iter().collect();
    let mut diff = StructuralDiff::default();
    for (i, key) in new.iter().enumerate() {
        match first_prev_pos.get(key) {
            None => diff.added.push(i),
            Some(&prev_pos) if prev_pos != i => diff.reordered.push((i, prev_pos)),
            Some(_) => {}
        }
    }
    for (i, key) in prev.iter().enumerate() {
        if !new_keyset.contains(key) {
            diff.removed.push(i);
        }
    }
    diff
}

fn key_of_book(b: &parsers::cbl::ParsedCblBook) -> EntryKey {
    EntryKey {
        series: b.series.clone(),
        number: b.number.clone(),
        volume: b.volume.clone(),
        year: b.year.clone(),
    }
}

fn key_of_entry(e: &cbl_entry::Model) -> EntryKey {
    EntryKey {
        series: e.series_name.clone(),
        number: e.issue_number.clone(),
        volume: e.volume.clone(),
        year: e.year.clone(),
    }
}

fn diff_row(position: i32, b: &parsers::cbl::ParsedCblBook) -> serde_json::Value {
    serde_json::json!({
        "position": position,
        "series": b.series,
        "number": b.number,
        "volume": b.volume,
    })
}

pub fn sha256_of(bytes: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(series: &str, number: &str) -> EntryKey {
        EntryKey {
            series: series.to_owned(),
            number: number.to_owned(),
            volume: None,
            year: None,
        }
    }

    /// The previous algorithm, kept verbatim as the reference: a linear
    /// `position` search per new entry.
    fn reference_diff(prev: &[EntryKey], new: &[EntryKey]) -> StructuralDiff {
        let prev_keyset: std::collections::HashSet<EntryKey> = prev.iter().cloned().collect();
        let new_keyset: std::collections::HashSet<EntryKey> = new.iter().cloned().collect();
        let mut diff = StructuralDiff::default();
        for (i, key) in new.iter().enumerate() {
            if !prev_keyset.contains(key) {
                diff.added.push(i);
            } else if let Some(prev_pos) = prev.iter().position(|p| p == key)
                && prev_pos != i
            {
                diff.reordered.push((i, prev_pos));
            }
        }
        for (i, prev_key) in prev.iter().enumerate() {
            if !new_keyset.contains(prev_key) {
                diff.removed.push(i);
            }
        }
        diff
    }

    #[test]
    fn structural_diff_matches_the_linear_search_with_duplicates_and_reorders() {
        let a = k("A", "1");
        let b = k("B", "1");
        let c = k("C", "2");
        let d = k("D", "3");
        let e = k("E", "4");
        let cases: Vec<(Vec<EntryKey>, Vec<EntryKey>)> = vec![
            (vec![], vec![a.clone(), b.clone()]),
            (vec![a.clone(), b.clone()], vec![]),
            (
                vec![a.clone(), b.clone(), c.clone()],
                vec![a.clone(), b.clone(), c.clone()],
            ),
            (
                vec![a.clone(), b.clone(), c.clone()],
                vec![c.clone(), a.clone(), b.clone()],
            ),
            // Duplicates in the stored list, the new list, and both.
            (
                vec![a.clone(), a.clone(), b.clone()],
                vec![b.clone(), a.clone(), a.clone()],
            ),
            (
                vec![a.clone(), b.clone()],
                vec![a.clone(), b.clone(), a.clone(), d.clone(), d.clone()],
            ),
            (
                vec![c.clone(), a.clone(), c.clone(), b.clone(), c.clone()],
                vec![b.clone(), c.clone(), e.clone(), c.clone(), a.clone()],
            ),
            (
                vec![a.clone(), b.clone(), b.clone(), d.clone()],
                vec![d.clone(), e.clone(), b.clone()],
            ),
        ];
        for (prev, new) in &cases {
            assert_eq!(
                structural_diff(prev, new),
                reference_diff(prev, new),
                "prev={prev:?} new={new:?}"
            );
        }

        // A larger pseudo-random list with many duplicates.
        let pool = [a, b, c, d, e];
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..50 {
            let prev: Vec<EntryKey> = (0..(next() % 40))
                .map(|_| pool[(next() % 5) as usize].clone())
                .collect();
            let new: Vec<EntryKey> = (0..(next() % 40))
                .map(|_| pool[(next() % 5) as usize].clone())
                .collect();
            assert_eq!(structural_diff(&prev, &new), reference_diff(&prev, &new));
        }
    }

    #[test]
    fn structural_diff_reports_the_first_stored_copy_of_a_duplicate() {
        let a = k("A", "1");
        let b = k("B", "1");
        let diff = structural_diff(&[b.clone(), a.clone(), a.clone()], &[a.clone(), b, a]);
        assert_eq!(diff.added, Vec::<usize>::new());
        assert_eq!(diff.removed, Vec::<usize>::new());
        // New #0 (A) was first stored at 1, new #1 (B) at 0, new #2 (A) at 1.
        assert_eq!(diff.reordered, vec![(0, 1), (1, 0), (2, 1)]);
    }
}
