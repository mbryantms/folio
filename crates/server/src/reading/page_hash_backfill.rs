//! Lazy `page_hash` backfill for anchors written before WP-6.2 (roadmap
//! WP-8.4).
//!
//! Markers and progress rows created before page-hash anchoring have a
//! NULL `page_hash`, so a replaced archive re-resolves them by ordinal
//! only. Rather than a library-wide hashing job, the hash is filled in
//! when the archive is next opened for reading: the page server (and the
//! OPDS-PSE streamer) call [`spawn_on_open`] when the shared `zip_lru`
//! opens an issue's archive on a cache miss. The spawned task:
//!
//! 1. asks which page ordinals of the issue carry an unhashed marker or
//!    progress row (one query, served by `markers(issue_id, page_index)`
//!    and `progress_records_issue_unhashed_idx`), at most
//!    [`MAX_PAGES_PER_OPEN`] of them;
//! 2. hashes those pages through the same cached reader the page server
//!    streams from, releasing the reader lock between pages;
//! 3. stamps every unhashed row on each hashed page
//!    (`… WHERE page_hash IS NULL`, so a concurrent capture or rescan
//!    always wins). `updated_at` is not bumped: a hash is not a
//!    user-visible change and must not wake sync clients.
//!
//! An issue with more pending pages than the cap finishes on later opens.
//! One task per issue at a time (an in-flight set on `AppState`), and the
//! work never blocks the page response.
//!
//! The stamp records the page the anchor points at *now* — the page the
//! reader shows for it. A legacy anchor whose archive was replaced before
//! this ran may already point at different pixels than the user marked;
//! stamping freezes that state, which is still the best evidence there
//! is, and later replacements then re-resolve by image. The one known-bad
//! case is skipped: a marker tagged `page-removed` was moved to a
//! neighbour of a page that no longer exists, so its ordinal is a guess
//! and it stays unhashed.

use crate::reading::page_remap::PAGE_REMOVED_TAG;
use crate::state::AppState;
use entity::issue;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use std::path::Path;

/// Most distinct pages hashed per archive open.
pub const MAX_PAGES_PER_OPEN: usize = 32;

/// What one backfill pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BackfillOutcome {
    /// Distinct pages hashed.
    pub pages_hashed: usize,
    /// Marker rows that gained a hash.
    pub markers_stamped: u64,
    /// Progress rows that gained a hash.
    pub progress_stamped: u64,
}

/// Fire-and-forget backfill after the archive for `row` was opened.
/// Returns immediately; does nothing if a backfill for the issue is
/// already running.
pub fn spawn_on_open(state: &AppState, row: &issue::Model) {
    {
        let mut inflight = state
            .page_hash_backfill_inflight
            .lock()
            .expect("page-hash backfill in-flight mutex");
        if !inflight.insert(row.id.clone()) {
            return;
        }
    }
    let state = state.clone();
    let row = row.clone();
    tokio::spawn(async move {
        let res = backfill_issue(&state, &row).await;
        state
            .page_hash_backfill_inflight
            .lock()
            .expect("page-hash backfill in-flight mutex")
            .remove(&row.id);
        match res {
            Ok(o) if o.pages_hashed > 0 => tracing::debug!(
                issue_id = %row.id,
                pages = o.pages_hashed,
                markers = o.markers_stamped,
                progress = o.progress_stamped,
                "page hash backfill"
            ),
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(issue_id = %row.id, error = %e, "page hash backfill failed");
            }
        }
    });
}

/// Page ordinals of `issue_id` with an unhashed marker or progress row,
/// ascending, at most [`MAX_PAGES_PER_OPEN`]. Ordinals past the issue's
/// page count are left alone (nothing to hash; a rescan re-anchors them).
async fn pending_pages<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    page_count: i32,
) -> Result<Vec<i32>, sea_orm::DbErr> {
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT p FROM ( \
             SELECT page_index AS p FROM markers \
              WHERE issue_id = $1 AND page_hash IS NULL \
                AND NOT ($4 = ANY(tags)) \
             UNION \
             SELECT last_page AS p FROM progress_records \
              WHERE issue_id = $1 AND page_hash IS NULL \
         ) t WHERE p >= 0 AND p < $2 ORDER BY p LIMIT $3",
        [
            issue_id.into(),
            page_count.into(),
            (MAX_PAGES_PER_OPEN as i64).into(),
            PAGE_REMOVED_TAG.into(),
        ],
    );
    let rows = db.query_all_raw(stmt).await?;
    rows.iter().map(|r| r.try_get::<i32>("", "p")).collect()
}

/// Hash and stamp one bounded batch of pages for `row`. Public so tests
/// can await it directly; production goes through [`spawn_on_open`].
pub async fn backfill_issue(
    state: &AppState,
    row: &issue::Model,
) -> Result<BackfillOutcome, sea_orm::DbErr> {
    let mut outcome = BackfillOutcome::default();
    let page_count = row.page_count.unwrap_or(i32::MAX);
    let pages = pending_pages(&state.db, &row.id, page_count).await?;
    if pages.is_empty() {
        return Ok(outcome);
    }
    let Ok(arc) = state
        .zip_lru
        .get_or_open(&row.id, Path::new(&row.file_path))
    else {
        return Ok(outcome);
    };
    for page in pages {
        let Ok(index) = usize::try_from(page) else {
            continue;
        };
        let reader = arc.clone();
        // One page per blocking task so the page server can take the
        // reader lock between pages.
        let hashed = tokio::task::spawn_blocking(move || {
            let mut r = reader.lock().expect("zip_lru reader mutex");
            crate::reading::page_hash::hash_page(&mut r, index)
        })
        .await;
        let hash = match hashed {
            Ok(Ok(Some(h))) => h,
            Ok(Ok(None)) => continue,
            Ok(Err(e)) => {
                tracing::debug!(issue_id = %row.id, page, error = %e, "page hash backfill: page unreadable");
                continue;
            }
            Err(e) => {
                tracing::warn!(issue_id = %row.id, error = %e, "page hash backfill: task failed");
                break;
            }
        };
        outcome.pages_hashed += 1;
        let m = state
            .db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE markers SET page_hash = $1 \
                  WHERE issue_id = $2 AND page_index = $3 AND page_hash IS NULL \
                    AND NOT ($4 = ANY(tags))",
                [
                    hash.clone().into(),
                    row.id.clone().into(),
                    page.into(),
                    PAGE_REMOVED_TAG.into(),
                ],
            ))
            .await?;
        outcome.markers_stamped += m.rows_affected();
        let p = state
            .db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE progress_records SET page_hash = $1 \
                  WHERE issue_id = $2 AND last_page = $3 AND page_hash IS NULL",
                [hash.into(), row.id.clone().into(), page.into()],
            ))
            .await?;
        outcome.progress_stamped += p.rows_affected();
    }
    Ok(outcome)
}
