//! Per-page content hashes for anchoring markers and reading progress
//! to a page *image* rather than only an ordinal (roadmap WP-6.2, audit
//! R28).
//!
//! A page hash is the hex BLAKE3 of the page entry's decompressed bytes,
//! read through the same random-access reader the page server uses
//! ([`CachedReader`]) so "the image the reader showed" and "the image a
//! rescan finds" hash identically. Entry names, compression, and the
//! archive container do not enter the hash: re-packing the same images
//! in a different order, under different names, or as CBT instead of
//! CBZ keeps every page hash.
//!
//! Two call shapes:
//!
//! - [`capture`] — one page, at anchor-write time (marker create,
//!   progress write). Uses the shared `zip_lru` handle, so it reads the
//!   same bytes the page server just streamed.
//! - [`hash_all_pages`] — the whole page list of an archive on disk, when
//!   a rescan or archive edit re-resolves anchors
//!   ([`super::page_remap::reanchor_issue`]).
//!
//! Every failure is soft: an unreadable page or a format without a page
//! reader (`.cbr` / `.cb7`, which the page server cannot serve either)
//! yields `None`, and the anchor keeps the ordinal-only behaviour.
//!
//! The OCR cache key (`ocr::cache::cache_key`) is deliberately not
//! derived from these hashes; it stays on `content_hash` + ordinal.

use crate::library::zip_lru::CachedReader;
use crate::state::AppState;
use archive::{ArchiveError, ArchiveLimits};
use entity::issue;
use std::path::{Path, PathBuf};

/// Hex BLAKE3 of the page at `index` in `reader`'s page order. `None`
/// when the index is past the end.
pub fn hash_page(reader: &mut CachedReader, index: usize) -> Result<Option<String>, ArchiveError> {
    let Some(entry) = reader.pages().get(index).copied().cloned() else {
        return Ok(None);
    };
    let mut hasher = blake3::Hasher::new();
    reader.pipe_entry(&entry, &mut hasher)?;
    Ok(Some(hasher.finalize().to_hex().to_string()))
}

/// Hash every page of the archive at `path`, in page order. Opens a
/// fresh reader (never the cached one) so a rescan always sees the bytes
/// currently on disk. Blocking; call from `spawn_blocking`.
pub fn hash_all_pages_blocking(
    path: &Path,
    limits: ArchiveLimits,
) -> Result<Vec<String>, ArchiveError> {
    let mut reader = CachedReader::open(path, limits)?;
    let count = reader.pages().len();
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        match hash_page(&mut reader, i)? {
            Some(h) => out.push(h),
            None => break,
        }
    }
    Ok(out)
}

/// Async wrapper over [`hash_all_pages_blocking`] that takes an
/// `archive_work_semaphore` permit like every other archive-wide read.
/// `None` (logged) on any failure.
pub async fn hash_all_pages(state: &AppState, path: PathBuf) -> Option<Vec<String>> {
    let limits = state.cfg().archive_limits();
    let _permit = state
        .archive_work_semaphore
        .clone()
        .acquire_owned()
        .await
        .ok()?;
    let shown = path.clone();
    match tokio::task::spawn_blocking(move || hash_all_pages_blocking(&path, limits)).await {
        Ok(Ok(hashes)) => Some(hashes),
        Ok(Err(e)) => {
            tracing::warn!(path = %shown.display(), error = %e, "page hash: archive unreadable; anchors fall back to ordinals");
            None
        }
        Err(e) => {
            tracing::warn!(path = %shown.display(), error = %e, "page hash: task failed");
            None
        }
    }
}

/// Hash of the page a user is anchoring to right now. Reads through the
/// shared `zip_lru` reader (the one the page server streams from).
/// `None` when the page can't be read — the anchor is still written,
/// just without a hash.
pub async fn capture(state: &AppState, row: &issue::Model, page: i32) -> Option<String> {
    let index = usize::try_from(page).ok()?;
    let arc = match state
        .zip_lru
        .get_or_open(&row.id, Path::new(&row.file_path))
    {
        Ok(a) => a,
        Err(e) => {
            tracing::debug!(issue_id = %row.id, error = %e, "page hash: no page reader; capturing without hash");
            return None;
        }
    };
    let issue_id = row.id.clone();
    let res = tokio::task::spawn_blocking(move || {
        let mut reader = arc.lock().expect("zip_lru reader mutex");
        hash_page(&mut reader, index)
    })
    .await;
    match res {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            tracing::debug!(issue_id = %issue_id, page, error = %e, "page hash: page unreadable; capturing without hash");
            None
        }
        Err(e) => {
            tracing::warn!(issue_id = %issue_id, error = %e, "page hash: capture task failed");
            None
        }
    }
}
