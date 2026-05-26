//! Library validation (spec §4.2).
//!
//! Refuses to start a scan when:
//!   - root path doesn't exist or isn't readable
//!   - root path is empty
//!   - root path equals `COMIC_DATA_PATH` (would create a scan loop, since
//!     thumbnails / secrets live under data_path)
//!   - root path equals or contains another library's root (overlap → dup work)
//!
//! Per spec §12.3 these are *fatal* errors — the scan_run row is closed with
//! `state='failed'` and an explanatory `error` message.

use crate::state::AppState;
use entity::library;
use sea_orm::{ColumnTrait, EntityTrait, FromQueryResult, QueryFilter, QuerySelect};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("library root does not exist: {0}")]
    RootMissing(String),
    #[error("library root is not a directory: {0}")]
    RootNotDirectory(String),
    #[error("library root is unreadable: {0}: {1}")]
    RootUnreadable(String, String),
    #[error("library root is empty: {0}")]
    RootEmpty(String),
    #[error("library root equals data path; would create a scan loop")]
    LoopWithDataPath,
    #[error("library root overlaps another library: {0}")]
    OverlapsAnotherLibrary(String),
}

pub async fn validate_library(
    state: &AppState,
    lib: &library::Model,
) -> Result<(), ValidationError> {
    // Filesystem syscalls (canonicalize, read_dir, is_dir) are
    // synchronous and can block on slow / disconnected mounts. Route
    // them through `spawn_blocking` so the tokio runtime keeps making
    // progress on other requests during library validation. M4 of
    // code-quality-cleanup-1.0.
    let root_path = lib.root_path.clone();
    let data_path = state.cfg().data_path.clone();
    let root_path_for_block = root_path.clone();
    let prevalidation: Result<(std::path::PathBuf, Option<std::path::PathBuf>), ValidationError> =
        tokio::task::spawn_blocking(move || {
            let root = Path::new(&root_path_for_block);
            let root_canon = std::fs::canonicalize(root)
                .map_err(|_| ValidationError::RootMissing(root_path_for_block.clone()))?;
            if !root_canon.is_dir() {
                return Err(ValidationError::RootNotDirectory(
                    root_path_for_block.clone(),
                ));
            }
            let mut iter = std::fs::read_dir(&root_canon).map_err(|e| {
                ValidationError::RootUnreadable(root_path_for_block.clone(), e.to_string())
            })?;
            if iter.next().is_none() {
                return Err(ValidationError::RootEmpty(root_path_for_block.clone()));
            }
            let data_canon = std::fs::canonicalize(&data_path).ok();
            Ok((root_canon, data_canon))
        })
        .await
        .map_err(|e| ValidationError::RootUnreadable(root_path.clone(), e.to_string()))?;
    let (root_canon, data_canon) = prevalidation?;

    // Loop check: root must not be the data dir.
    if let Some(d) = data_canon
        && root_canon == d
    {
        return Err(ValidationError::LoopWithDataPath);
    }

    // Overlap check: no other library may share or contain this root. Equality
    // and ancestor relationships both count. The other-library canonicalize
    // calls are also blocking — batch them in a single spawn_blocking.
    //
    // Push the `id != lib.id` filter to SQL and project just the two columns
    // the canonicalize loop reads (audit-remediation M5.3) — the old
    // `library::Entity::find().all()` hydrated full rows then filtered in Rust.
    #[derive(FromQueryResult)]
    struct OtherRoot {
        id: uuid::Uuid,
        root_path: String,
    }
    if let Ok(other_roots) = library::Entity::find()
        .filter(library::Column::Id.ne(lib.id))
        .select_only()
        .column(library::Column::Id)
        .column(library::Column::RootPath)
        .into_model::<OtherRoot>()
        .all(&state.db)
        .await
    {
        let other_roots: Vec<(uuid::Uuid, String)> = other_roots
            .into_iter()
            .map(|r| (r.id, r.root_path))
            .collect();
        if !other_roots.is_empty() {
            let root_canon_for_block = root_canon.clone();
            let conflict = tokio::task::spawn_blocking(move || {
                for (_id, other_root) in other_roots {
                    if let Ok(other_canon) = std::fs::canonicalize(&other_root)
                        && (other_canon == root_canon_for_block
                            || other_canon.starts_with(&root_canon_for_block)
                            || root_canon_for_block.starts_with(&other_canon))
                    {
                        return Some(other_root);
                    }
                }
                None
            })
            .await
            .ok()
            .flatten();
            if let Some(other_root) = conflict {
                return Err(ValidationError::OverlapsAnotherLibrary(other_root));
            }
        }
    }

    Ok(())
}

/// Lighter-weight variant for the per-series scan path: just confirm the
/// folder still exists. A missing folder no-ops (the next full scan handles
/// reconciliation per §4.2).
pub fn folder_still_exists(folder: &Path) -> bool {
    folder.is_dir()
}
