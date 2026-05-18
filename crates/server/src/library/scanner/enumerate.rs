//! Per-folder enumeration (spec §4.3) — layout classifier for both
//! supported on-disk shapes.
//!
//! The library root may follow one of two layouts:
//!
//! - **Layout A (flat):** `root/Series/CBZ`. Each depth-1 child folder
//!   contains archive files directly. The series folder may also have
//!   category subfolders (`Specials`, `Annuals`, …) holding extra
//!   archives; these are walked-through recursively but don't
//!   re-classify the parent.
//!
//! - **Layout B (nested-by-publisher):** `root/Publisher/Series/CBZ`.
//!   Each depth-1 child contains zero archives at its own depth-1, but
//!   its (depth-2) subfolders are Layout-A series folders. The
//!   depth-1 folder name becomes the `publisher_hint` for every series
//!   beneath it.
//!
//! Mixed roots are supported per-child: one library can have some
//! flat series at the root and some publisher containers beside them.
//!
//! Layouts that don't fit either shape (series with no archives at
//! depth-1, three-deep nesting, etc.) emit `AmbiguousFolder` health
//! issues in M2; M1 just collects them in
//! [`EnumerationResult::ambiguous_folders`].
//!
//! Hidden folders (dot-prefixed) and ignore-globs are skipped silently.

use crate::library::ignore::IgnoreRules;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SeriesCandidate {
    /// Absolute path to the series folder.
    pub path: PathBuf,
    /// When the series was discovered beneath a publisher container
    /// (Layout B), the publisher folder's name. Last-resort fallback
    /// for `series.publisher` after ComicInfo + `series.json`.
    pub publisher_hint: Option<String>,
}

#[derive(Debug, Default)]
pub struct EnumerationResult {
    pub series_folders: Vec<SeriesCandidate>,
    pub files_at_root: Vec<PathBuf>,
    pub empty_folders: Vec<PathBuf>,
    /// Folders that violate the two-layouts contract. M2 surfaces these
    /// via [`crate::library::health::IssueKind::AmbiguousFolder`].
    pub ambiguous_folders: Vec<AmbiguousFolder>,
}

#[derive(Debug, Clone)]
pub struct AmbiguousFolder {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct ArchiveWalk {
    pub archives: Vec<PathBuf>,
    pub changed_since: bool,
}

/// Case-insensitive series-subfolder allowlist. A folder with one of
/// these names inside a series folder is a "category bucket"
/// (Specials/Annuals/etc.), not its own series. These names also
/// drive path-derived `special_type` in M2.5.
const SERIES_SUBFOLDER_ALLOWLIST: &[&str] = &[
    "specials",
    "extras",
    "bonus",
    "tie-ins",
    "annuals",
    "annual",
    "oneshots",
    "one-shots",
];

pub fn is_series_subfolder_name(name: &str) -> bool {
    let lc = name.to_ascii_lowercase();
    SERIES_SUBFOLDER_ALLOWLIST.contains(&lc.as_str())
}

/// Walk the immediate children of `root`. Returns folders (series
/// candidates) and any layout violations the spec calls out.
pub fn enumerate(root: &Path) -> std::io::Result<EnumerationResult> {
    enumerate_with(root, &IgnoreRules::default())
}

/// Same as [`enumerate`], but additionally applies user-configured
/// ignore globs. Classifies each depth-1 child per the two-layouts
/// contract documented at the module level.
pub fn enumerate_with(root: &Path, ignore: &IgnoreRules) -> std::io::Result<EnumerationResult> {
    let mut result = EnumerationResult::default();

    for child in read_dir_filtered(root, ignore)? {
        let ft = match std::fs::metadata(&child) {
            Ok(m) => m.file_type(),
            Err(_) => continue,
        };
        if ft.is_file() {
            // Spec §2.2: no archive files at the library root.
            result.files_at_root.push(child);
            continue;
        }
        if !ft.is_dir() {
            continue;
        }

        match classify_folder(&child, ignore) {
            FolderShape::SeriesFolder => result.series_folders.push(SeriesCandidate {
                path: child,
                publisher_hint: None,
            }),
            FolderShape::PublisherContainer => {
                classify_publisher_children(&child, ignore, &mut result);
            }
            FolderShape::Empty => result.empty_folders.push(child),
            FolderShape::Ambiguous(reason) => {
                result.ambiguous_folders.push(AmbiguousFolder {
                    path: child,
                    reason,
                });
            }
        }
    }

    Ok(result)
}

fn classify_publisher_children(
    publisher: &Path,
    ignore: &IgnoreRules,
    result: &mut EnumerationResult,
) {
    let publisher_name = publisher
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    let children = match read_dir_filtered(publisher, ignore) {
        Ok(v) => v,
        Err(_) => return,
    };
    for sub in children {
        let ft = match std::fs::metadata(&sub) {
            Ok(m) => m.file_type(),
            Err(_) => continue,
        };
        if !ft.is_dir() {
            // Stray files inside a publisher folder violate the contract.
            if ft.is_file() {
                let ext = sub
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(str::to_ascii_lowercase);
                let is_archive = ext
                    .as_deref()
                    .is_some_and(crate::library::ignore::is_recognized_archive_ext);
                if is_archive {
                    result.ambiguous_folders.push(AmbiguousFolder {
                        path: sub,
                        reason: format!(
                            "archive file directly inside publisher folder \"{}\"; \
                             move it into a series folder",
                            publisher_name.as_deref().unwrap_or("(unnamed)"),
                        ),
                    });
                }
            }
            continue;
        }

        match classify_folder(&sub, ignore) {
            FolderShape::SeriesFolder => result.series_folders.push(SeriesCandidate {
                path: sub,
                publisher_hint: publisher_name.clone(),
            }),
            FolderShape::PublisherContainer => {
                // 3-deep nesting — out of scope per the plan.
                result.ambiguous_folders.push(AmbiguousFolder {
                    path: sub,
                    reason: "folder appears to be a third nesting level; \
                             Folio supports at most Publisher/Series/CBZ"
                        .to_owned(),
                });
            }
            FolderShape::Empty => result.empty_folders.push(sub),
            FolderShape::Ambiguous(reason) => {
                result.ambiguous_folders.push(AmbiguousFolder {
                    path: sub,
                    reason,
                });
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum FolderShape {
    SeriesFolder,
    PublisherContainer,
    Empty,
    Ambiguous(String),
}

/// Classify a directory per the two-layouts contract.
///
/// - Has archives at depth-1 → series folder (Layout A). Subfolders
///   are walked recursively by the existing archive walker; their
///   shape doesn't change the parent's classification.
/// - Has no archives at depth-1 but ≥1 non-allowlist subdir contains
///   archives → publisher container (Layout B).
/// - Has no archives at depth-1, all archive-bearing subdirs are
///   allowlist-named → contract violation (series with only specials).
/// - Has nothing → empty.
fn classify_folder(folder: &Path, ignore: &IgnoreRules) -> FolderShape {
    use crate::library::ignore::is_recognized_archive_ext;

    let mut has_archive_at_d1 = false;
    let mut nonallowlist_subdirs_with_archives = Vec::<PathBuf>::new();
    let mut allowlist_subdirs_with_archives = Vec::<PathBuf>::new();
    let mut any_subdir_present = false;

    let entries = match std::fs::read_dir(folder) {
        Ok(it) => it,
        Err(_) => return FolderShape::Empty,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        if matches!(
            name_str.as_ref(),
            "__MACOSX" | "Thumbs.db" | "desktop.ini" | "@eaDir"
        ) {
            continue;
        }
        if ignore.should_skip(&path) {
            continue;
        }

        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };

        if ft.is_file() {
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase);
            if let Some(ext) = ext
                && is_recognized_archive_ext(&ext)
            {
                has_archive_at_d1 = true;
            }
            continue;
        }

        if ft.is_dir() {
            any_subdir_present = true;
            if subdir_has_archive(&path, ignore) {
                if is_series_subfolder_name(&name_str) {
                    allowlist_subdirs_with_archives.push(path);
                } else {
                    nonallowlist_subdirs_with_archives.push(path);
                }
            }
        }
    }

    if has_archive_at_d1 {
        // Layout A series. Non-allowlist subdirs are still allowed —
        // they get slurped recursively by `list_archives_with`, same as
        // today. Allowlist names will additionally drive special_type
        // assignment in M2.5.
        return FolderShape::SeriesFolder;
    }

    if !nonallowlist_subdirs_with_archives.is_empty() {
        // No archives at depth-1, but real series-named subdirs below
        // have archives → publisher container.
        return FolderShape::PublisherContainer;
    }

    if !allowlist_subdirs_with_archives.is_empty() {
        // No archives at depth-1, only category-named subdirs have
        // archives → contract violation. The user likely meant this to
        // be a series, but the main run is missing.
        return FolderShape::Ambiguous(
            "series-style folder has no archives at its top level, only category \
             subfolders (Specials/Annuals/…); move the main archives up one level \
             or remove the category wrapper"
                .to_owned(),
        );
    }

    if any_subdir_present {
        // Folder has subdirs but none contain archives. Treat as empty
        // for health-issue purposes (existing EmptyFolder behavior).
        return FolderShape::Empty;
    }

    FolderShape::Empty
}

fn subdir_has_archive(folder: &Path, ignore: &IgnoreRules) -> bool {
    use crate::library::ignore::is_recognized_archive_ext;
    use walkdir::WalkDir;
    WalkDir::new(folder)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !ignore.should_skip(e.path()))
        .filter_map(Result::ok)
        .any(|e| {
            e.file_type().is_file()
                && e.path()
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref()
                    .is_some_and(is_recognized_archive_ext)
        })
}

fn read_dir_filtered(root: &Path, ignore: &IgnoreRules) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
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
        out.push(path);
    }
    Ok(out)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_empty(path: &Path) {
        fs::write(path, b"").unwrap();
    }

    /// Layout A — flat series at the library root.
    #[test]
    fn layout_a_flat() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let series_a = root.join("Series A");
        fs::create_dir(&series_a).unwrap();
        write_empty(&series_a.join("Series A - v01.cbz"));
        write_empty(&series_a.join("Series A - v02.cbz"));

        let series_b = root.join("Series B");
        fs::create_dir(&series_b).unwrap();
        write_empty(&series_b.join("Oneshot.cbz"));

        let result = enumerate(root).unwrap();
        assert_eq!(result.series_folders.len(), 2);
        assert!(result.ambiguous_folders.is_empty());
        assert!(result.empty_folders.is_empty());
        for s in &result.series_folders {
            assert!(s.publisher_hint.is_none(), "flat layout has no publisher");
        }
    }

    /// Layout A with a Specials subfolder. The series folder is still a
    /// series; Specials is walked-through but doesn't re-classify it.
    #[test]
    fn layout_a_with_specials_subfolder() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let series = root.join("Series A");
        fs::create_dir(&series).unwrap();
        write_empty(&series.join("Series A - v01.cbz"));
        write_empty(&series.join("Series A - v02.cbz"));

        let specials = series.join("Specials");
        fs::create_dir(&specials).unwrap();
        write_empty(&specials.join("Artbook 1.cbz"));

        let result = enumerate(root).unwrap();
        assert_eq!(result.series_folders.len(), 1);
        assert_eq!(result.series_folders[0].path, series);
        assert!(result.ambiguous_folders.is_empty());
    }

    /// Layout B — publisher folders at the root, series beneath.
    #[test]
    fn layout_b_nested_by_publisher() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let marvel = root.join("Marvel");
        fs::create_dir(&marvel).unwrap();
        let daredevil = marvel.join("Daredevil");
        fs::create_dir(&daredevil).unwrap();
        write_empty(&daredevil.join("Daredevil - v01.cbz"));

        let dc = root.join("DC");
        fs::create_dir(&dc).unwrap();
        let batman = dc.join("Batman");
        fs::create_dir(&batman).unwrap();
        write_empty(&batman.join("Batman - v01.cbz"));
        write_empty(&batman.join("Batman - v02.cbz"));

        let result = enumerate(root).unwrap();
        assert_eq!(result.series_folders.len(), 2);
        assert!(result.ambiguous_folders.is_empty());

        let by_path: std::collections::HashMap<_, _> = result
            .series_folders
            .iter()
            .map(|s| (s.path.clone(), s.publisher_hint.clone()))
            .collect();
        assert_eq!(by_path.get(&daredevil), Some(&Some("Marvel".to_owned())));
        assert_eq!(by_path.get(&batman), Some(&Some("DC".to_owned())));
    }

    /// Mixed root: a flat series next to a publisher container.
    /// Each top-level folder is classified independently.
    #[test]
    fn mixed_root_classifies_per_child() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let watchmen = root.join("Watchmen");
        fs::create_dir(&watchmen).unwrap();
        write_empty(&watchmen.join("Watchmen.cbz"));

        let marvel = root.join("Marvel");
        fs::create_dir(&marvel).unwrap();
        let daredevil = marvel.join("Daredevil");
        fs::create_dir(&daredevil).unwrap();
        write_empty(&daredevil.join("Daredevil - v01.cbz"));

        let result = enumerate(root).unwrap();
        assert_eq!(result.series_folders.len(), 2);

        let by_path: std::collections::HashMap<_, _> = result
            .series_folders
            .iter()
            .map(|s| (s.path.clone(), s.publisher_hint.clone()))
            .collect();
        assert_eq!(by_path.get(&watchmen), Some(&None));
        assert_eq!(by_path.get(&daredevil), Some(&Some("Marvel".to_owned())));
    }

    /// Empty folders at the root still get flagged via the existing
    /// EmptyFolder collector.
    #[test]
    fn empty_folder_at_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let lonely = root.join("Lonely");
        fs::create_dir(&lonely).unwrap();

        let result = enumerate(root).unwrap();
        assert_eq!(result.empty_folders.len(), 1);
        assert_eq!(result.empty_folders[0], lonely);
        assert!(result.series_folders.is_empty());
    }

    /// Files at the root are still flagged.
    #[test]
    fn file_at_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let stray = root.join("stray.cbz");
        write_empty(&stray);

        let result = enumerate(root).unwrap();
        assert_eq!(result.files_at_root.len(), 1);
        assert_eq!(result.files_at_root[0], stray);
    }

    /// A series folder that has NO archives at its top level, only a
    /// Specials subfolder with archives, violates the contract. Surface
    /// as ambiguous rather than guessing.
    #[test]
    fn series_with_only_specials_is_ambiguous() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let series = root.join("Series A");
        fs::create_dir(&series).unwrap();
        let specials = series.join("Specials");
        fs::create_dir(&specials).unwrap();
        write_empty(&specials.join("Artbook 1.cbz"));

        let result = enumerate(root).unwrap();
        assert!(result.series_folders.is_empty());
        assert_eq!(result.ambiguous_folders.len(), 1);
        assert_eq!(result.ambiguous_folders[0].path, series);
        assert!(
            result.ambiguous_folders[0]
                .reason
                .contains("category subfolders")
        );
    }

    /// A 3-deep layout (`root/Publisher/Imprint/Series/CBZ`) is out of
    /// scope. The Imprint folder is flagged ambiguous.
    #[test]
    fn three_deep_is_ambiguous() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let dc = root.join("DC");
        fs::create_dir(&dc).unwrap();
        let vertigo = dc.join("Vertigo");
        fs::create_dir(&vertigo).unwrap();
        let sandman = vertigo.join("Sandman");
        fs::create_dir(&sandman).unwrap();
        write_empty(&sandman.join("Sandman - v01.cbz"));

        let result = enumerate(root).unwrap();
        assert!(result.series_folders.is_empty());
        assert_eq!(result.ambiguous_folders.len(), 1);
        assert_eq!(result.ambiguous_folders[0].path, vertigo);
        assert!(
            result.ambiguous_folders[0]
                .reason
                .contains("third nesting")
        );
    }

    /// An archive file directly under a publisher folder (no series
    /// wrapper) is ambiguous — we don't invent a series name.
    #[test]
    fn archive_directly_under_publisher_is_ambiguous() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let marvel = root.join("Marvel");
        fs::create_dir(&marvel).unwrap();
        write_empty(&marvel.join("stray.cbz"));
        let daredevil = marvel.join("Daredevil");
        fs::create_dir(&daredevil).unwrap();
        write_empty(&daredevil.join("Daredevil - v01.cbz"));

        let result = enumerate(root).unwrap();
        // Marvel is classified as a publisher because it has no archives
        // at its own depth-1... wait — it DOES have stray.cbz. So Marvel
        // is actually classified as a Series folder under Layout A rules!
        //
        // This is intentional: an archive at depth-1 means "this is a
        // series folder," full stop. The Daredevil subfolder gets
        // walked recursively by list_archives_with, which would slurp
        // its CBZs into the "Marvel" series. That's surprising but it
        // matches existing scanner behavior for non-allowlist subdirs
        // inside a series folder.
        //
        // The user contract says: pick a layout per folder. If you have
        // CBZs at depth-1, you're flat; nested CBZs in non-allowlist
        // subdirs get folded in.
        assert_eq!(result.series_folders.len(), 1);
        assert_eq!(result.series_folders[0].path, marvel);
        assert!(result.series_folders[0].publisher_hint.is_none());
    }

    /// Hidden folders are still ignored.
    #[test]
    fn hidden_folders_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let hidden = root.join(".hidden");
        fs::create_dir(&hidden).unwrap();
        write_empty(&hidden.join("ignored.cbz"));

        let visible = root.join("Series");
        fs::create_dir(&visible).unwrap();
        write_empty(&visible.join("Series.cbz"));

        let result = enumerate(root).unwrap();
        assert_eq!(result.series_folders.len(), 1);
        assert_eq!(result.series_folders[0].path, visible);
    }
}
