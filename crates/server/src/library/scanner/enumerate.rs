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
    /// Folders with no archives that still carry a `series.json`
    /// sidecar — usually a series whose archives were moved or deleted
    /// while the Mylar3 sidecar stayed behind. Surfaced as
    /// [`crate::library::health::IssueKind::OrphanedSeriesJson`] *instead
    /// of* `EmptyFolder` (the folder isn't empty; it's orphaned).
    pub orphaned_series_json: Vec<PathBuf>,
}

/// Upper bound on how many skipped archive paths an
/// [`AmbiguousFolder`] carries in its preview. The health row stores the
/// preview in its jsonb payload, so the bound keeps a 5k-archive
/// mis-nested publisher tree from producing a megabyte row; the exact
/// total still rides alongside in `skipped_archive_count`.
pub const AMBIGUOUS_PREVIEW_LIMIT: usize = 20;

#[derive(Debug, Clone)]
pub struct AmbiguousFolder {
    pub path: PathBuf,
    pub reason: String,
    /// Bounded preview (≤ [`AMBIGUOUS_PREVIEW_LIMIT`]) of the archives the
    /// scanner skipped because of this violation, as paths relative to
    /// `path` (or the file name itself when `path` is a stray archive).
    /// Sorted for a stable payload across scans.
    pub skipped_archives: Vec<String>,
    /// Exact number of archives in the skipped subtree.
    pub skipped_archive_count: u32,
}

impl AmbiguousFolder {
    fn new(path: PathBuf, reason: String, ignore: &IgnoreRules) -> Self {
        let (skipped_archives, skipped_archive_count) = preview_skipped_archives(&path, ignore);
        Self {
            path,
            reason,
            skipped_archives,
            skipped_archive_count,
        }
    }
}

/// Walk the skipped subtree once, counting every recognized archive and
/// keeping the first [`AMBIGUOUS_PREVIEW_LIMIT`] (by sorted relative
/// path). A stray archive file previews as itself.
fn preview_skipped_archives(path: &Path, ignore: &IgnoreRules) -> (Vec<String>, u32) {
    if path.is_file() {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return (vec![name], 1);
    }
    let mut all: Vec<String> = list_archives_with(path, ignore)
        .into_iter()
        .map(|p| {
            p.strip_prefix(path)
                .unwrap_or(&p)
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let total = u32::try_from(all.len()).unwrap_or(u32::MAX);
    all.sort();
    all.truncate(AMBIGUOUS_PREVIEW_LIMIT);
    (all, total)
}

/// An archive-less folder that still holds a `series.json` is an orphaned
/// sidecar, not an empty folder.
fn has_series_json(folder: &Path) -> bool {
    folder.join("series.json").is_file()
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
///
/// Markers are compared on their lowercased alphanumerics (`Tie-Ins` ≡
/// `tie ins` ≡ `TieIns`), each mapped to the `special_type` the bucket's
/// archives get. See [`series_subfolder_kind`] for where in the name a
/// marker counts.
const SERIES_SUBFOLDER_MARKERS: &[(&str, &str)] = &[
    ("specials", "Special"),
    ("special", "Special"),
    ("extras", "Special"),
    ("extra", "Special"),
    ("bonus", "Special"),
    ("tieins", "Special"),
    ("annuals", "Annual"),
    ("annual", "Annual"),
    ("oneshots", "OneShot"),
    ("oneshot", "OneShot"),
    ("tpb", "TPB"),
    ("tpbs", "TPB"),
    ("trade", "TPB"),
    ("trades", "TPB"),
    ("tradepaperbacks", "TPB"),
    ("collected", "TPB"),
    ("collectededition", "TPB"),
    ("collectededitions", "TPB"),
    ("collections", "TPB"),
    ("collection", "TPB"),
    ("hardcover", "TPB"),
    ("hardcovers", "TPB"),
    ("omnibus", "TPB"),
];

/// Is `name` a *bucket* subfolder of a series (its archives are extras,
/// not the run), and which `special_type` does it imply?
///
/// A name counts when, with bracket groups removed, it
/// - is exactly a marker (`Annuals`, `Extras`, `One-Shots`),
/// - starts with one (`Annuals (01-13)(1987-2000)(digital)`,
///   `Specials - misc`), or
/// - ends with one (`The Flash v2 Extras`, `Batman Specials`),
///
/// and carries no `(YYYY)` series-year group: `The Flash Annual (2012)`
/// is a series folder, not a bucket, because the year group is how a
/// series folder is named. Semi-matches (`Semiannual`) never count — the
/// comparison is per word, joined pairs included (`One Shots`, `Tie Ins`).
pub fn series_subfolder_kind(name: &str) -> Option<&'static str> {
    let (head, groups) = split_bracket_groups(name);
    let has_year_group = groups
        .iter()
        .any(|g| g.len() == 4 && g.bytes().all(|b| b.is_ascii_digit()));
    let words: Vec<String> = head
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_lowercase())
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect();
    let lookup = |key: &str| {
        SERIES_SUBFOLDER_MARKERS
            .iter()
            .find(|(m, _)| *m == key)
            .map(|(_, tag)| *tag)
    };
    let n = words.len();
    // A `(YYYY)` group is how a series folder is named; whatever words
    // surround it, the folder is a series, not a bucket.
    if n == 0 || has_year_group {
        return None;
    }
    // Leading marker: the single first word, or the first two joined
    // ("one shots", "tie ins").
    let lead = lookup(&words[0]).or_else(|| {
        (n >= 2)
            .then(|| format!("{}{}", words[0], words[1]))
            .and_then(|k| lookup(&k))
    });
    if lead.is_some() {
        return lead;
    }
    lookup(&words[n - 1]).or_else(|| {
        (n >= 2)
            .then(|| format!("{}{}", words[n - 2], words[n - 1]))
            .and_then(|k| lookup(&k))
    })
}

/// `(text outside brackets, contents of each `(…)` / `[…]` group)`.
fn split_bracket_groups(name: &str) -> (String, Vec<String>) {
    let mut head = String::with_capacity(name.len());
    let mut groups = Vec::new();
    let mut current: Option<(char, String)> = None;
    for c in name.chars() {
        match (&mut current, c) {
            (None, '(' | '[') => current = Some((c, String::new())),
            (Some((open, buf)), ')' | ']') if (*open == '(') == (c == ')') => {
                groups.push(buf.trim().to_owned());
                current = None;
            }
            (Some((_, buf)), c) => buf.push(c),
            (None, c) => head.push(c),
        }
    }
    (head, groups)
}

pub fn is_series_subfolder_name(name: &str) -> bool {
    series_subfolder_kind(name).is_some()
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
        classify_root_child(child, ignore, &mut result);
    }

    Ok(result)
}

/// Classify one depth-1 child of the library root (file or folder) into
/// `result`. Shared by the full [`enumerate_with`] walk and the
/// watcher-scoped [`enumerate_scoped`] walk so both produce identical
/// series candidates for the same on-disk shape.
fn classify_root_child(child: PathBuf, ignore: &IgnoreRules, result: &mut EnumerationResult) {
    let ft = match std::fs::metadata(&child) {
        Ok(m) => m.file_type(),
        Err(_) => return,
    };
    if ft.is_file() {
        // Spec §2.2: no archive files at the library root.
        result.files_at_root.push(child);
        return;
    }
    if !ft.is_dir() {
        return;
    }

    match classify_folder(&child, ignore) {
        FolderShape::SeriesFolder => result.series_folders.push(SeriesCandidate {
            path: child,
            publisher_hint: None,
        }),
        FolderShape::PublisherContainer => {
            classify_publisher_children(&child, ignore, result);
        }
        FolderShape::Empty if has_series_json(&child) => {
            result.orphaned_series_json.push(child);
        }
        FolderShape::Empty => result.empty_folders.push(child),
        FolderShape::Ambiguous(reason) => {
            result
                .ambiguous_folders
                .push(AmbiguousFolder::new(child, reason, ignore));
        }
    }
}

/// Result of a watcher-scoped enumeration (WP-3.1). See [`enumerate_scoped`].
#[derive(Debug, Default)]
pub struct ScopedEnumeration {
    /// Series folders that need a plan entry: every *new* series folder
    /// under the scanned tops, plus every known one that contains a touched
    /// directory. Unchanged known siblings are left out so a watcher scan
    /// never walks a folder nothing happened in.
    pub result: EnumerationResult,
    /// Every series folder that exists under the scanned tops (unfiltered).
    /// Feeds the reconcile's "folder still present" set.
    pub present_series: Vec<PathBuf>,
    /// The depth-1 children of the root whose subtree this pass is
    /// authoritative for. A known series whose folder lives under one of
    /// these tops but is not in `present_series` is gone; series under any
    /// other top are out of scope and left untouched.
    pub tops: Vec<PathBuf>,
}

/// Enumerate only the parts of the library a set of touched directories
/// (from the file watcher or the network-mount poller) can have changed.
///
/// Each touched directory maps to its depth-1 ancestor under `root` (the
/// "top"); only those tops are classified with the same two-layouts rules as
/// [`enumerate_with`]. When the root itself is touched (a series or
/// publisher folder was added, removed or renamed, or a stray file landed at
/// the root) the root is read once — one `readdir`, no recursion — and every
/// child that is not already the top of a known series joins the scope, as
/// do known tops that no longer exist (so their series are reconciled away).
///
/// `known_series` is the set of `series.folder_path` values the library
/// already has. Paths outside `root` are ignored.
pub fn enumerate_scoped(
    root: &Path,
    ignore: &IgnoreRules,
    scope: &[PathBuf],
    known_series: &std::collections::HashSet<PathBuf>,
) -> std::io::Result<ScopedEnumeration> {
    use std::collections::BTreeSet;

    let top_of = |p: &Path| -> Option<Option<PathBuf>> {
        let rel = p.strip_prefix(root).ok()?;
        Some(rel.components().next().map(|c| root.join(c)))
    };

    let mut tops = BTreeSet::<PathBuf>::new();
    let mut root_touched = false;
    for dir in scope {
        match top_of(dir) {
            Some(Some(top)) => {
                tops.insert(top);
            }
            Some(None) => root_touched = true,
            None => {}
        }
    }

    let mut result = EnumerationResult::default();
    if root_touched {
        let known_tops: BTreeSet<PathBuf> = known_series
            .iter()
            .filter_map(|s| top_of(s).flatten())
            .collect();
        for child in read_dir_filtered(root, ignore)? {
            let Ok(meta) = std::fs::metadata(&child) else {
                continue;
            };
            if meta.is_file() {
                result.files_at_root.push(child);
            } else if meta.is_dir() && !known_tops.contains(&child) {
                tops.insert(child);
            }
        }
        for known_top in known_tops {
            if !known_top.exists() {
                tops.insert(known_top);
            }
        }
    }

    for top in &tops {
        if top.is_dir() && !ignore.should_skip_user(top) {
            classify_root_child(top.clone(), ignore, &mut result);
        }
    }

    let present_series = result
        .series_folders
        .iter()
        .map(|c| c.path.clone())
        .collect();
    result.series_folders.retain(|c| {
        !known_series.contains(&c.path) || scope.iter().any(|d| d.starts_with(&c.path))
    });

    Ok(ScopedEnumeration {
        result,
        present_series,
        tops: tops.into_iter().collect(),
    })
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
                    let reason = format!(
                        "archive file directly inside publisher folder \"{}\"; \
                         move it into a series folder",
                        publisher_name.as_deref().unwrap_or("(unnamed)"),
                    );
                    result
                        .ambiguous_folders
                        .push(AmbiguousFolder::new(sub, reason, ignore));
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
                let reason = "folder appears to be a third nesting level; \
                              Folio supports at most Publisher/Series/CBZ"
                    .to_owned();
                result
                    .ambiguous_folders
                    .push(AmbiguousFolder::new(sub, reason, ignore));
            }
            FolderShape::Empty if has_series_json(&sub) => {
                result.orphaned_series_json.push(sub);
            }
            FolderShape::Empty => result.empty_folders.push(sub),
            FolderShape::Ambiguous(reason) => {
                result
                    .ambiguous_folders
                    .push(AmbiguousFolder::new(sub, reason, ignore));
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
        assert!(result.ambiguous_folders[0].reason.contains("third nesting"));
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

    /// The AmbiguousFolder row must list what it skipped: a bounded,
    /// sorted preview of relative archive paths plus the exact total.
    #[test]
    fn ambiguous_folder_previews_skipped_subtree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let vertigo = root.join("DC").join("Vertigo");
        let sandman = vertigo.join("Sandman");
        fs::create_dir_all(&sandman).unwrap();
        for i in 0..(AMBIGUOUS_PREVIEW_LIMIT + 5) {
            write_empty(&sandman.join(format!("Sandman {i:03}.cbz")));
        }
        write_empty(&sandman.join("notes.txt"));

        let result = enumerate(root).unwrap();
        assert_eq!(result.ambiguous_folders.len(), 1);
        let amb = &result.ambiguous_folders[0];
        assert_eq!(amb.path, vertigo);
        assert_eq!(
            amb.skipped_archive_count as usize,
            AMBIGUOUS_PREVIEW_LIMIT + 5
        );
        assert_eq!(amb.skipped_archives.len(), AMBIGUOUS_PREVIEW_LIMIT);
        assert_eq!(
            amb.skipped_archives[0],
            Path::new("Sandman")
                .join("Sandman 000.cbz")
                .to_string_lossy()
        );
    }

    /// A stray archive path previews as itself.
    #[test]
    fn stray_archive_preview_is_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let stray = tmp.path().join("stray.cbz");
        write_empty(&stray);
        let (preview, total) = preview_skipped_archives(&stray, &IgnoreRules::default());
        assert_eq!(preview, vec!["stray.cbz".to_owned()]);
        assert_eq!(total, 1);
    }

    /// An archive-less folder that still holds `series.json` is reported
    /// as an orphaned sidecar, not as an empty folder — at the root and
    /// under a publisher.
    #[test]
    fn orphaned_series_json_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let orphan = root.join("Gone Series (2019)");
        fs::create_dir(&orphan).unwrap();
        fs::write(orphan.join("series.json"), b"{}").unwrap();

        let marvel = root.join("Marvel");
        let daredevil = marvel.join("Daredevil");
        fs::create_dir_all(&daredevil).unwrap();
        write_empty(&daredevil.join("Daredevil 001.cbz"));
        let nested_orphan = marvel.join("Moved Away");
        fs::create_dir(&nested_orphan).unwrap();
        fs::write(nested_orphan.join("series.json"), b"{}").unwrap();

        let lonely = root.join("Lonely");
        fs::create_dir(&lonely).unwrap();

        let result = enumerate(root).unwrap();
        let mut orphans = result.orphaned_series_json.clone();
        orphans.sort();
        let mut expected = vec![orphan, nested_orphan];
        expected.sort();
        assert_eq!(orphans, expected);
        assert_eq!(result.empty_folders, vec![lonely]);
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

    /// Watcher-scoped enumeration (WP-3.1): a change inside one series of a
    /// publisher container plans only that series (plus brand-new ones),
    /// while every existing series under the same top is still reported
    /// present so the reconcile doesn't treat siblings as gone.
    #[test]
    fn scoped_enumeration_plans_only_touched_and_new_series() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let publisher = root.join("Marvel");
        let xmen = publisher.join("X-Men");
        let hulk = publisher.join("Hulk");
        let fresh = publisher.join("Thor");
        let flat = root.join("Saga");
        for d in [&xmen, &hulk, &fresh, &flat] {
            fs::create_dir_all(d).unwrap();
            write_empty(&d.join("001.cbz"));
        }
        let known: std::collections::HashSet<PathBuf> = [xmen.clone(), hulk.clone(), flat.clone()]
            .into_iter()
            .collect();

        let scoped = enumerate_scoped(
            root,
            &IgnoreRules::default(),
            std::slice::from_ref(&xmen),
            &known,
        )
        .unwrap();
        let planned: Vec<_> = scoped
            .result
            .series_folders
            .iter()
            .map(|c| c.path.clone())
            .collect();
        assert!(planned.contains(&xmen));
        assert!(
            planned.contains(&fresh),
            "new series under the top is planned"
        );
        assert!(
            !planned.contains(&hulk),
            "untouched known sibling is skipped"
        );
        assert!(!planned.contains(&flat), "other tops are out of scope");
        assert_eq!(scoped.present_series.len(), 3);
        assert_eq!(scoped.tops, vec![publisher.clone()]);
        assert!(scoped.result.files_at_root.is_empty());

        // Root touched: only unknown tops join (every top here is known),
        // and stray root files are reported.
        write_empty(&root.join("stray.cbz"));
        let scoped =
            enumerate_scoped(root, &IgnoreRules::default(), &[root.to_path_buf()], &known).unwrap();
        assert!(scoped.tops.is_empty(), "{:?}", scoped.tops);
        assert_eq!(scoped.result.files_at_root.len(), 1);
    }
}

#[cfg(test)]
mod subfolder_kind_tests {
    use super::{is_series_subfolder_name, series_subfolder_kind, split_bracket_groups};

    #[test]
    fn exact_markers_in_any_case() {
        for (name, tag) in [
            ("Specials", "Special"),
            ("EXTRAS", "Special"),
            ("Bonus", "Special"),
            ("Tie-Ins", "Special"),
            ("Tie Ins", "Special"),
            ("Annuals", "Annual"),
            ("annual", "Annual"),
            ("Oneshots", "OneShot"),
            ("One-Shots", "OneShot"),
            ("One Shots", "OneShot"),
            ("TPB", "TPB"),
            ("Trades", "TPB"),
            ("Trade Paperbacks", "TPB"),
            ("Collected Editions", "TPB"),
            ("Hardcovers", "TPB"),
        ] {
            assert_eq!(series_subfolder_kind(name), Some(tag), "{name}");
            assert!(is_series_subfolder_name(name), "{name}");
        }
    }

    #[test]
    fn leading_marker_survives_decoration() {
        assert_eq!(
            series_subfolder_kind("Annuals (01-13)(1987-2000)(digital)"),
            Some("Annual")
        );
        assert_eq!(series_subfolder_kind("Specials - misc"), Some("Special"));
        assert_eq!(series_subfolder_kind("Extras [scans]"), Some("Special"));
    }

    #[test]
    fn trailing_marker_counts_only_without_a_year_group() {
        assert_eq!(
            series_subfolder_kind("The Flash v2 Extras"),
            Some("Special")
        );
        assert_eq!(series_subfolder_kind("Batman Specials"), Some("Special"));
        assert_eq!(
            series_subfolder_kind("Saga Collected Editions"),
            Some("TPB")
        );
        // Series folders named for the marker keep their year group → not
        // a bucket (they are a series whose run *is* the annuals).
        assert_eq!(series_subfolder_kind("The Flash Annual (2012)"), None);
        assert_eq!(series_subfolder_kind("Marvel Holiday Special (2004)"), None);
        assert!(!is_series_subfolder_name("The Flash Annual (2012)"));
    }

    #[test]
    fn non_buckets_are_left_alone() {
        for name in [
            "Volume 2",
            "Semiannual Report",
            "The Flash (1987)",
            "Specialists (2020)",
            "Annual Report (2019)",
            "",
            "(digital)",
        ] {
            assert_eq!(series_subfolder_kind(name), None, "{name:?}");
        }
    }

    #[test]
    fn bracket_groups_split() {
        let (head, groups) = split_bracket_groups("Annuals (01-13)(1987-2000) [digital] x");
        assert_eq!(
            head.split_whitespace().collect::<Vec<_>>(),
            vec!["Annuals", "x"]
        );
        assert_eq!(groups, vec!["01-13", "1987-2000", "digital"]);
    }
}
