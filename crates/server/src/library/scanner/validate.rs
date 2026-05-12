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
use sea_orm::EntityTrait;
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
    let root = Path::new(&lib.root_path);
    let root_canon = std::fs::canonicalize(root)
        .map_err(|_| ValidationError::RootMissing(lib.root_path.clone()))?;

    if !root_canon.is_dir() {
        return Err(ValidationError::RootNotDirectory(lib.root_path.clone()));
    }

    let mut iter = std::fs::read_dir(&root_canon)
        .map_err(|e| ValidationError::RootUnreadable(lib.root_path.clone(), e.to_string()))?;
    if iter.next().is_none() {
        return Err(ValidationError::RootEmpty(lib.root_path.clone()));
    }

    // Loop check: root must not be the data dir.
    if let Ok(data_canon) = std::fs::canonicalize(&state.cfg.data_path)
        && root_canon == data_canon
    {
        return Err(ValidationError::LoopWithDataPath);
    }

    // Overlap check: no other library may share or contain this root. Equality
    // and ancestor relationships both count.
    if let Ok(others) = library::Entity::find().all(&state.db).await {
        for other in others.into_iter().filter(|l| l.id != lib.id) {
            if let Ok(other_canon) = std::fs::canonicalize(&other.root_path)
                && (other_canon == root_canon
                    || other_canon.starts_with(&root_canon)
                    || root_canon.starts_with(&other_canon))
            {
                return Err(ValidationError::OverlapsAnotherLibrary(other.root_path));
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
