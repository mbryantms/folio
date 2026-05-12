//! Per-folder enumeration (spec §4.3).
//!
//! Lists direct children of the library root. Series folders become work
//! items; files at root and disallowed entries become health issues (the
//! actual `library_health_issues` persistence ships in Milestone 5 — until
//! then we just log warnings).
//!
//! Hidden folders (starting with `.`) are skipped silently.

use crate::library::ignore::IgnoreRules;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct EnumerationResult {
    pub series_folders: Vec<PathBuf>,
    pub files_at_root: Vec<PathBuf>,
    pub empty_folders: Vec<PathBuf>,
}

#[derive(Debug, Default)]
pub struct ArchiveWalk {
    pub archives: Vec<PathBuf>,
    pub changed_since: bool,
}

/// Walk the immediate children of `root`. Returns folders (series candidates)
/// and any layout violations the spec calls out.
pub fn enumerate(root: &Path) -> std::io::Result<EnumerationResult> {
    enumerate_with(root, &IgnoreRules::default())
}

/// Same as [`enumerate`], but additionally applies user-configured ignore globs.
pub fn enumerate_with(root: &Path, ignore: &IgnoreRules) -> std::io::Result<EnumerationResult> {
    let mut series_folders = Vec::new();
    let mut files_at_root = Vec::new();
    let mut empty_folders = Vec::new();

    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Built-in ignore: dot-prefixed entries (spec §5.1).
        if name_str.starts_with('.') {
            continue;
        }
        // Built-in ignore patterns from spec §5.1.
        if matches!(
            name_str.as_ref(),
            "__MACOSX" | "Thumbs.db" | "desktop.ini" | "@eaDir"
        ) {
            continue;
        }

        // User globs apply *before* we classify file vs folder.
        if ignore.should_skip_user(&path) {
            continue;
        }

        let ft = entry.file_type()?;
        if ft.is_file() {
            // Spec §2.2: no archive files at the library root.
            files_at_root.push(path);
            continue;
        }
        if ft.is_dir() {
            // Spec §2.2 + §10.1 EmptyFolder.
            let is_empty = std::fs::read_dir(&path)
                .map(|mut it| it.next().is_none())
                .unwrap_or(true);
            if is_empty {
                empty_folders.push(path);
            } else {
                series_folders.push(path);
            }
        }
    }

    Ok(EnumerationResult {
        series_folders,
        files_at_root,
        empty_folders,
    })
}

/// Recursively enumerate `.cbz` (and Milestone-12 friends) under a series
/// folder. Sub-folders inside a series folder are allowed (spec §2.2,
/// "Annuals" / "Specials"). Returns absolute paths in directory traversal
/// order — caller may sort for stability.
pub fn list_archives(folder: &Path) -> Vec<PathBuf> {
    list_archives_with(folder, &IgnoreRules::default())
}

/// Same as [`list_archives`], but additionally honors user ignore globs.
pub fn list_archives_with(folder: &Path, ignore: &IgnoreRules) -> Vec<PathBuf> {
    use crate::library::ignore::is_recognized_archive_ext;
    use walkdir::WalkDir;
    let mut out = Vec::new();
    for entry in WalkDir::new(folder)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !ignore.should_skip(e.path()))
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase);
        if let Some(ext) = ext
            && is_recognized_archive_ext(&ext)
        {
            out.push(path.to_path_buf());
        }
    }
    out
}

pub fn list_archives_changed_since(
    folder: &Path,
    ignore: &IgnoreRules,
    since: chrono::DateTime<chrono::Utc>,
) -> ArchiveWalk {
    use crate::library::ignore::is_recognized_archive_ext;
    use walkdir::WalkDir;
    let mut out = ArchiveWalk::default();
    for entry in WalkDir::new(folder)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !ignore.should_skip(e.path()))
        .filter_map(Result::ok)
    {
        if let Ok(meta) = entry.metadata()
            && let Ok(m) = meta.modified()
        {
            let m: chrono::DateTime<chrono::Utc> = m.into();
            if m > since {
                out.changed_since = true;
            }
        }

        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase);
        if let Some(ext) = ext
            && is_recognized_archive_ext(&ext)
        {
            out.archives.push(path.to_path_buf());
        }
    }
    out
}

/// Recursive max mtime under `folder`. Used by spec §4.4 to skip unchanged
/// folders. Short-circuits as soon as a file newer than `since` is found.
pub fn folder_changed_since(folder: &Path, since: chrono::DateTime<chrono::Utc>) -> bool {
    use walkdir::WalkDir;
    for entry in WalkDir::new(folder)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if let Ok(meta) = entry.metadata()
            && let Ok(m) = meta.modified()
        {
            let m: chrono::DateTime<chrono::Utc> = m.into();
            if m > since {
                return true;
            }
        }
    }
    false
}
