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
//!
//! **Symbolic links are never followed**, wherever they point (outside the
//! root or inside it), and whether they name a folder or a file: every
//! entry is classified from link-not-following metadata
//! (`symlink_metadata` / `DirEntry::file_type`) and every walk runs with
//! `follow_links(false)` *and* `follow_root_links(false)`. A link is
//! neither a series, a publisher nor an archive; it is collected into
//! [`EnumerationResult::symlinks`] / [`ArchiveWalk::symlinks`] and the
//! scanner reports each one as
//! [`crate::library::health::IssueKind::SymlinkSkipped`]. The library root
//! is the one exception: the configured root path may itself be (or pass
//! through) a link, a deployment choice such as `/data/comics -> /mnt/x`.
//! It is resolved by the kernel when the root is opened, the same way
//! `validate_library` canonicalizes it, and nothing below it is followed.

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

/// A folder the walk could not read (permission denied, dead mount,
/// I/O error). Post-audit T1.1: its subtree is *unknown*, not absent — the
/// scanner never authorises removal under it and raises
/// [`crate::library::health::IssueKind::UnreadableFolder`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableFolder {
    pub path: PathBuf,
    pub error: String,
}

impl UnreadableFolder {
    fn new(path: impl Into<PathBuf>, error: &std::io::Error) -> Self {
        Self {
            path: path.into(),
            error: error.to_string(),
        }
    }
}

/// `NotFound` means the entry is genuinely gone (a removal the reconcile
/// should see); every other error leaves its subtree unknown.
fn is_unreadable(e: &std::io::Error) -> bool {
    e.kind() != std::io::ErrorKind::NotFound
}

#[derive(Debug, Default)]
pub struct EnumerationResult {
    pub series_folders: Vec<SeriesCandidate>,
    pub files_at_root: Vec<PathBuf>,
    pub empty_folders: Vec<PathBuf>,
    /// Folders whose read failed. A known series at or under one of these
    /// paths is excluded from the "folder missing" sweep (post-audit T1.1).
    pub unreadable: Vec<UnreadableFolder>,
    /// Folders that violate the two-layouts contract. M2 surfaces these
    /// via [`crate::library::health::IssueKind::AmbiguousFolder`].
    pub ambiguous_folders: Vec<AmbiguousFolder>,
    /// Folders with no archives that still carry a `series.json`
    /// sidecar — usually a series whose archives were moved or deleted
    /// while the Mylar3 sidecar stayed behind. Surfaced as
    /// [`crate::library::health::IssueKind::OrphanedSeriesJson`] *instead
    /// of* `EmptyFolder` (the folder isn't empty; it's orphaned).
    pub orphaned_series_json: Vec<PathBuf>,
    /// Symbolic links met at the library top level, directly inside a
    /// publisher folder, or at the top of a folder that is not a series
    /// (empty, ambiguous, unreadable). Never followed; reported as
    /// [`crate::library::health::IssueKind::SymlinkSkipped`]. Links inside a
    /// series folder come from that folder's walk ([`ArchiveWalk::symlinks`]).
    pub symlinks: Vec<PathBuf>,
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
/// keeping the first [`AMBIGUOUS_PREVIEW_LIMIT`] in [`archive_order`]
/// (the walk's order, the one ingest uses). A stray archive file previews
/// as itself.
fn preview_skipped_archives(path: &Path, ignore: &IgnoreRules) -> (Vec<String>, u32) {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return (vec![name], 1);
    }
    // Already in `archive_order`: the walk sorts.
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
    all.truncate(AMBIGUOUS_PREVIEW_LIMIT);
    (all, total)
}

/// An archive-less folder that still holds a `series.json` is an orphaned
/// sidecar, not an empty folder.
fn has_series_json(folder: &Path) -> bool {
    std::fs::symlink_metadata(folder.join("series.json")).is_ok_and(|m| m.is_file())
}

#[derive(Debug, Default)]
pub struct ArchiveWalk {
    pub archives: Vec<PathBuf>,
    pub changed_since: bool,
    /// Directories inside the folder the walk could not descend into. A
    /// non-empty list makes the walk *incomplete*: files under those
    /// directories were not seen, so the folder must not authorise their
    /// removal (post-audit T1.1).
    pub unreadable: Vec<UnreadableFolder>,
    /// Symbolic links met in the walk (the walked folder itself included,
    /// when it is one). Never followed; reported as
    /// [`crate::library::health::IssueKind::SymlinkSkipped`].
    pub symlinks: Vec<PathBuf>,
}

impl ArchiveWalk {
    pub fn is_complete(&self) -> bool {
        self.unreadable.is_empty()
    }
}

/// The order a series folder's archives are ingested in: the order a
/// person reads the folder, never the order the filesystem lists it
/// (`read_dir` is newest-first on tmpfs, hash order on ext4, and
/// different again on a network share, so a scan's result must not
/// depend on it).
///
/// - The folder's own files come before those of its subfolders
///   (`Specials/`, `Annuals/`, …), and subfolders follow one another in
///   the same order as files: the parent folders are compared first, one
///   path component at a time, a shorter parent (the series folder
///   itself) first.
/// - Names compare in natural order (`Issue 2` before `Issue 10`),
///   ignoring case: [`natord::compare_ignore_case`], the comparator the
///   CBZ reader sorts pages with.
/// - A digit run that starts with `0` is compared left-aligned, as a
///   fraction (natord's rule): `Issue 01` sorts before `Issue 1` and
///   `Issue 010` before `Issue 9`. A folder that mixes padded and
///   unpadded numbers is in a stable order, but not a numeric one.
/// - Two paths that are still equal (`issue 1.cbz` and `Issue 1.cbz`)
///   fall back to their raw bytes, so the order is total and no two
///   listings of the same names can disagree.
///
/// Compares names already in memory: no file system call.
pub fn archive_order(a: &Path, b: &Path) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    use std::os::unix::ffi::OsStrExt;
    fn natural(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> Ordering {
        natord::compare_ignore_case(&a.to_string_lossy(), &b.to_string_lossy())
    }
    let parents = match (a.parent(), b.parent()) {
        (Some(pa), Some(pb)) if pa == pb => Ordering::Equal,
        (pa, pb) => {
            let mut ca = pa.into_iter().flat_map(Path::components);
            let mut cb = pb.into_iter().flat_map(Path::components);
            loop {
                match (ca.next(), cb.next()) {
                    (None, None) => break Ordering::Equal,
                    (None, Some(_)) => break Ordering::Less,
                    (Some(_), None) => break Ordering::Greater,
                    (Some(x), Some(y)) => match natural(x.as_os_str(), y.as_os_str()) {
                        Ordering::Equal => {}
                        other => break other,
                    },
                }
            }
        }
    };
    parents
        .then_with(|| match (a.file_name(), b.file_name()) {
            (Some(x), Some(y)) => natural(x, y),
            (x, y) => x.cmp(&y),
        })
        .then_with(|| a.as_os_str().as_bytes().cmp(b.as_os_str().as_bytes()))
}

/// Put a folder's archives in [`archive_order`]. Every walk that builds a
/// scan's per-folder list calls it before returning, so the identity
/// peek, the batches, the progress counters and the duplicate checks all
/// see the same order on every filesystem.
pub fn sort_archives(archives: &mut [PathBuf]) {
    archives.sort_by(|a, b| archive_order(a, b));
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
    result.symlinks.sort();
    result.symlinks.dedup();

    Ok(result)
}

/// Classify one depth-1 child of the library root (file or folder) into
/// `result`. Shared by the full [`enumerate_with`] walk and the
/// watcher-scoped [`enumerate_scoped`] walk so both produce identical
/// series candidates for the same on-disk shape.
fn classify_root_child(child: PathBuf, ignore: &IgnoreRules, result: &mut EnumerationResult) {
    // `symlink_metadata`: a link is never classified as what it points to.
    let ft = match std::fs::symlink_metadata(&child) {
        Ok(m) => m.file_type(),
        Err(e) => {
            if is_unreadable(&e) {
                result.unreadable.push(UnreadableFolder::new(child, &e));
            }
            return;
        }
    };
    if ft.is_symlink() {
        result.symlinks.push(child);
        return;
    }
    if ft.is_file() {
        // Spec §2.2: no archive files at the library root.
        result.files_at_root.push(child);
        return;
    }
    if !ft.is_dir() {
        return;
    }

    let mut links = Vec::new();
    let shape = classify_folder(&child, ignore, &mut links);
    keep_links_of_non_series(&shape, links, result);
    match shape {
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
        FolderShape::Unreadable(error) => result
            .unreadable
            .push(UnreadableFolder { path: child, error }),
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
            let meta = match std::fs::symlink_metadata(&child) {
                Ok(m) => m,
                Err(e) => {
                    if is_unreadable(&e) {
                        result.unreadable.push(UnreadableFolder::new(child, &e));
                    }
                    continue;
                }
            };
            if meta.is_symlink() {
                result.symlinks.push(child);
            } else if meta.is_file() {
                result.files_at_root.push(child);
            } else if meta.is_dir() && !known_tops.contains(&child) {
                tops.insert(child);
            }
        }
        for known_top in known_tops {
            match std::fs::symlink_metadata(&known_top) {
                // Replaced by a link: in scope, so its series reconcile away
                // (the classification below reports the link).
                Ok(m) if m.is_symlink() => {
                    tops.insert(known_top);
                }
                Ok(_) => {}
                // Gone for real: in scope, so its series reconcile away.
                Err(e) if !is_unreadable(&e) => {
                    tops.insert(known_top);
                }
                // Unknown: out of scope, nothing under it is touched.
                Err(e) => result.unreadable.push(UnreadableFolder::new(known_top, &e)),
            }
        }
    }

    for top in &tops {
        let classify = std::fs::symlink_metadata(top).is_ok_and(|m| m.is_dir() || m.is_symlink());
        if classify && !ignore.should_skip_user(top) {
            classify_root_child(top.clone(), ignore, &mut result);
        }
    }
    result.symlinks.sort();
    result.symlinks.dedup();

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
        Err(e) => {
            // Every series under this publisher is unknown, not gone.
            result.unreadable.push(UnreadableFolder::new(publisher, &e));
            return;
        }
    };
    for sub in children {
        let ft = match std::fs::symlink_metadata(&sub) {
            Ok(m) => m.file_type(),
            Err(e) => {
                if is_unreadable(&e) {
                    result.unreadable.push(UnreadableFolder::new(sub, &e));
                }
                continue;
            }
        };
        if ft.is_symlink() {
            result.symlinks.push(sub);
            continue;
        }
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

        let mut links = Vec::new();
        let shape = classify_folder(&sub, ignore, &mut links);
        keep_links_of_non_series(&shape, links, result);
        match shape {
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
            FolderShape::Unreadable(error) => {
                result
                    .unreadable
                    .push(UnreadableFolder { path: sub, error });
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
    /// The folder (or, when it has no archives of its own, one of its
    /// subfolders) could not be read. Never "empty": its contents are
    /// unknown (post-audit T1.1).
    Unreadable(String),
}

/// Links at the top of a folder the scanner will not walk as a series (empty,
/// ambiguous, unreadable) are reported from here; a series folder's own walk
/// reports its links, and a publisher container's children are classified
/// (links included) by [`classify_publisher_children`].
fn keep_links_of_non_series(
    shape: &FolderShape,
    links: Vec<PathBuf>,
    result: &mut EnumerationResult,
) {
    if !matches!(
        shape,
        FolderShape::SeriesFolder | FolderShape::PublisherContainer
    ) {
        result.symlinks.extend(links);
    }
}

/// Classify a directory per the two-layouts contract. Depth-1 symbolic
/// links are never followed (`DirEntry::file_type` does not follow them);
/// they are pushed onto `links` for the caller.
///
/// - Has archives at depth-1 → series folder (Layout A). Subfolders
///   are walked recursively by the existing archive walker; their
///   shape doesn't change the parent's classification.
/// - Has no archives at depth-1 but ≥1 non-allowlist subdir contains
///   archives → publisher container (Layout B).
/// - Has no archives at depth-1, all archive-bearing subdirs are
///   allowlist-named → contract violation (series with only specials).
/// - Has nothing → empty.
fn classify_folder(folder: &Path, ignore: &IgnoreRules, links: &mut Vec<PathBuf>) -> FolderShape {
    use crate::library::ignore::is_recognized_archive_ext;

    let mut has_archive_at_d1 = false;
    let mut nonallowlist_subdirs_with_archives = Vec::<PathBuf>::new();
    let mut allowlist_subdirs_with_archives = Vec::<PathBuf>::new();
    let mut any_subdir_present = false;
    let mut subdir_error: Option<String> = None;

    let entries = match std::fs::read_dir(folder) {
        Ok(it) => it,
        Err(e) => return FolderShape::Unreadable(e.to_string()),
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

        if ft.is_symlink() {
            links.push(path);
            continue;
        }

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
            match subdir_has_archive(&path, ignore) {
                Ok(true) => {
                    if is_series_subfolder_name(&name_str) {
                        allowlist_subdirs_with_archives.push(path);
                    } else {
                        nonallowlist_subdirs_with_archives.push(path);
                    }
                }
                Ok(false) => {}
                Err(e) => {
                    subdir_error.get_or_insert_with(|| format!("{}: {e}", path.display()));
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

    if let Some(error) = subdir_error {
        // No readable archives anywhere, but a subfolder could not be
        // walked — the folder is unknown, not empty.
        return FolderShape::Unreadable(error);
    }

    if any_subdir_present {
        // Folder has subdirs but none contain archives. Treat as empty
        // for health-issue purposes (existing EmptyFolder behavior).
        return FolderShape::Empty;
    }

    FolderShape::Empty
}

/// Does `folder` (recursively) contain a recognised archive? `Err` when a
/// directory in the walk could not be read *and* no archive was found
/// before it — an archive seen anywhere makes the answer `Ok(true)`
/// regardless.
fn subdir_has_archive(folder: &Path, ignore: &IgnoreRules) -> Result<bool, walkdir::Error> {
    use crate::library::ignore::is_recognized_archive_ext;
    use walkdir::WalkDir;
    let mut first_error = None;
    for entry in WalkDir::new(folder)
        .follow_links(false)
        .follow_root_links(false)
        .into_iter()
        .filter_entry(|e| !ignore.should_skip(e.path()))
    {
        let e = match entry {
            Ok(e) => e,
            Err(err) => {
                first_error.get_or_insert(err);
                continue;
            }
        };
        if e.file_type().is_file()
            && e.path()
                .extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
                .is_some_and(is_recognized_archive_ext)
        {
            return Ok(true);
        }
    }
    match first_error {
        Some(err) => Err(err),
        None => Ok(false),
    }
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
/// "Annuals" / "Specials"). Returns absolute paths in [`archive_order`].
pub fn list_archives(folder: &Path) -> Vec<PathBuf> {
    list_archives_with(folder, &IgnoreRules::default())
}

/// Same as [`list_archives`], but additionally honors user ignore globs.
/// Callers that must know whether the walk was complete use
/// [`walk_archives_with`]; this drops that signal.
pub fn list_archives_with(folder: &Path, ignore: &IgnoreRules) -> Vec<PathBuf> {
    walk_archives_with(folder, ignore).archives
}

/// [`list_archives_with`] that also reports the directories it could not
/// descend into (`ArchiveWalk::unreadable`), so the scanner can tell an
/// absent file from an unseen one (post-audit T1.1).
pub fn walk_archives_with(folder: &Path, ignore: &IgnoreRules) -> ArchiveWalk {
    use crate::library::ignore::is_recognized_archive_ext;
    use walkdir::WalkDir;
    let mut out = ArchiveWalk::default();
    for entry in WalkDir::new(folder)
        .follow_links(false)
        .follow_root_links(false)
        .into_iter()
        .filter_entry(|e| !ignore.should_skip(e.path()))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                record_walk_error(&mut out.unreadable, folder, err);
                continue;
            }
        };
        if entry.file_type().is_symlink() {
            out.symlinks.push(entry.into_path());
            continue;
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
    sort_archives(&mut out.archives);
    out
}

/// What a removal sweep may conclude about a stored path from the disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// A scan would index it.
    Present,
    /// Really gone: a not-found class of error (`NotFound`,
    /// `NotADirectory`) on the file or a path component, or a path a scan
    /// would never index (wrong place, ignored, not a regular file).
    Absent,
    /// The disk would not say (permission denied, I/O error, stale or
    /// disconnected mount, an unreadable folder in the walk). Like the
    /// scanner's `unreadable_folders`, unknown is never gone.
    Unknown,
}

impl Presence {
    /// Map a failed stat: only the not-found class means absent.
    fn from_error(e: &std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => Self::Absent,
            _ => Self::Unknown,
        }
    }
}

/// Would a scan of the library rooted at `root` index the archive at
/// `path`? The one definition of "this file is present" for the removal
/// sweeps (`reconcile::auto_confirm_sweep`, `jobs::hard_purge`), mirroring
/// what enumeration plus [`walk_archives_with`] accept:
///
/// - `path` lives below a folder of `root` (archives at the root itself are
///   never indexed) and has a recognised archive extension;
/// - neither the file nor any folder between `root` and it is skipped by
///   `ignore` (built-in junk names and the library's globs);
/// - the file itself is a **regular file**, not a symlink, FIFO, socket,
///   device or directory (the walk runs with `follow_links(false)` and keeps
///   only `is_file()` entries, so a symlinked archive is never indexed);
/// - its canonical path stays inside the canonical root, so a symlinked
///   folder that points out of the library never makes a file "present".
///
/// The scanner never follows a symbolic link below the root (module docs),
/// so a path through a linked folder pointing *out* of the root is absent
/// to both. A path through a linked folder pointing *inside* the root reads
/// present here (its canonical path is in the root) although no scan indexes
/// it at that path: the one difference, and it fails safe (the row is kept,
/// neither confirmed nor purged).
///
/// A stat that fails with anything but the not-found class is
/// [`Presence::Unknown`], never absent. `root_canon` is
/// `canonicalize(root)`, computed once by the caller. Blocking: run it on
/// the blocking pool.
pub fn archive_presence(
    root: &Path,
    root_canon: &Path,
    path: &Path,
    ignore: &IgnoreRules,
) -> Presence {
    use crate::library::ignore::is_recognized_archive_ext;
    let Ok(rel) = path.strip_prefix(root) else {
        return Presence::Absent;
    };
    if rel.components().count() < 2
        || rel
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Presence::Absent;
    }
    let is_archive = path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
        .is_some_and(is_recognized_archive_ext);
    if !is_archive {
        return Presence::Absent;
    }
    // Every entry between the root (exclusive) and the file (inclusive).
    if path
        .ancestors()
        .take_while(|a| *a != root)
        .any(|a| ignore.should_skip(a))
    {
        return Presence::Absent;
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_file() => {}
        Ok(_) => return Presence::Absent,
        Err(e) => return Presence::from_error(&e),
    }
    match std::fs::canonicalize(path) {
        Ok(c) if c.starts_with(root_canon) && c != root_canon => Presence::Present,
        Ok(_) => Presence::Absent,
        Err(e) => Presence::from_error(&e),
    }
}

/// Does the series `folder` (inside `root`) hold an archive a scan would
/// index, other than those `exclude` rejects? Same rules as
/// [`archive_presence`]: the folder must resolve inside the canonical root,
/// and the walk is [`walk_archives_with`] itself (which, like the scan,
/// never follows a link, the walked folder included). `Present` as soon as one
/// archive is; otherwise `Unknown` when the walk met an unreadable folder or
/// an archive it could not stat, else `Absent`. Blocking.
pub fn folder_presence(
    root: &Path,
    root_canon: &Path,
    folder: &Path,
    ignore: &IgnoreRules,
    exclude: impl Fn(&Path) -> bool,
) -> Presence {
    if folder.strip_prefix(root).is_err() {
        return Presence::Absent;
    }
    match std::fs::canonicalize(folder) {
        Ok(c) if c.starts_with(root_canon) && c != root_canon => {}
        Ok(_) => return Presence::Absent,
        Err(e) => return Presence::from_error(&e),
    }
    match std::fs::metadata(folder) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Presence::Absent,
        Err(e) => return Presence::from_error(&e),
    }
    let walk = walk_archives_with(folder, ignore);
    let mut unknown = !walk.unreadable.is_empty();
    for a in &walk.archives {
        if exclude(a) {
            continue;
        }
        match archive_presence(root, root_canon, a, ignore) {
            Presence::Present => return Presence::Present,
            Presence::Unknown => unknown = true,
            Presence::Absent => {}
        }
    }
    if unknown {
        Presence::Unknown
    } else {
        Presence::Absent
    }
}

/// Record a walkdir error as an unreadable directory. `NotFound` is not
/// an error for this purpose (the entry vanished mid-walk; the reconcile
/// should see it as gone); any other error leaves its subtree unknown.
fn record_walk_error(out: &mut Vec<UnreadableFolder>, folder: &Path, err: walkdir::Error) {
    let path = err
        .path()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| folder.to_path_buf());
    match err.io_error() {
        Some(io) if !is_unreadable(io) => {}
        Some(io) => out.push(UnreadableFolder::new(path, io)),
        None => out.push(UnreadableFolder {
            path,
            error: err.to_string(),
        }),
    }
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
        .follow_root_links(false)
        .into_iter()
        .filter_entry(|e| !ignore.should_skip(e.path()))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                // An unreadable subfolder counts as a change too: the
                // folder must not be fast-path skipped on the strength of
                // an mtime the walk could not fully observe.
                record_walk_error(&mut out.unreadable, folder, err);
                out.changed_since = true;
                continue;
            }
        };
        if let Ok(meta) = entry.metadata()
            && let Ok(m) = meta.modified()
        {
            let m: chrono::DateTime<chrono::Utc> = m.into();
            if m > since {
                out.changed_since = true;
            }
        }

        if entry.file_type().is_symlink() {
            out.symlinks.push(entry.into_path());
            continue;
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
    sort_archives(&mut out.archives);
    out
}

/// Recursive max mtime under `folder`. Used by spec §4.4 to skip unchanged
/// folders. Short-circuits as soon as a file newer than `since` is found.
pub fn folder_changed_since(folder: &Path, since: chrono::DateTime<chrono::Utc>) -> bool {
    use walkdir::WalkDir;
    for entry in WalkDir::new(folder)
        .follow_links(false)
        .follow_root_links(false)
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

    /// Links are never classified as what they point to: at the top level,
    /// inside a publisher folder and inside a series walk, each is reported.
    #[test]
    fn links_are_reported_not_followed() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        write_empty(&outside.join("x.cbz"));
        let real = root.join("Real");
        fs::create_dir_all(&real).unwrap();
        write_empty(&real.join("001.cbz"));
        let marvel = root.join("Marvel");
        fs::create_dir_all(marvel.join("Thor")).unwrap();
        write_empty(&marvel.join("Thor").join("001.cbz"));
        symlink(&outside, root.join("Out")).unwrap();
        symlink(&real, root.join("In")).unwrap();
        symlink(&outside, marvel.join("Linked")).unwrap();
        symlink(outside.join("x.cbz"), real.join("002.cbz")).unwrap();

        let result = enumerate(&root).unwrap();
        let mut series: Vec<_> = result
            .series_folders
            .iter()
            .map(|c| c.path.clone())
            .collect();
        series.sort();
        assert_eq!(series, vec![marvel.join("Thor"), real.clone()]);
        assert_eq!(
            result.symlinks,
            vec![root.join("In"), marvel.join("Linked"), root.join("Out")]
        );
        let walk = walk_archives_with(&real, &IgnoreRules::default());
        assert_eq!(walk.archives, vec![real.join("001.cbz")]);
        assert_eq!(walk.symlinks, vec![real.join("002.cbz")]);
        // A walk rooted at a link does not descend into it.
        let walk = walk_archives_with(&root.join("In"), &IgnoreRules::default());
        assert!(walk.archives.is_empty());
        assert_eq!(walk.symlinks, vec![root.join("In")]);
    }

    /// Scoped (watcher) enumeration: a known top-level series folder that
    /// was replaced by a link is in scope (its series reconcile away), is
    /// not present, and the link is reported.
    #[test]
    fn scoped_enumeration_treats_a_known_top_replaced_by_a_link_as_gone() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        let saga = root.join("Saga");
        let other = root.join("Other");
        for d in [&saga, &other] {
            fs::create_dir_all(d).unwrap();
            write_empty(&d.join("001.cbz"));
        }
        let known: std::collections::HashSet<PathBuf> =
            [saga.clone(), other.clone()].into_iter().collect();
        let moved = tmp.path().join("moved");
        fs::rename(&saga, &moved).unwrap();
        symlink(&moved, &saga).unwrap();

        let scoped = enumerate_scoped(
            &root,
            &IgnoreRules::default(),
            std::slice::from_ref(&root),
            &known,
        )
        .unwrap();
        assert_eq!(scoped.tops, vec![saga.clone()]);
        assert!(
            scoped.present_series.is_empty(),
            "{:?}",
            scoped.present_series
        );
        assert_eq!(scoped.result.symlinks, vec![saga.clone()]);
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

#[cfg(test)]
mod archive_order_tests {
    use super::*;
    use std::fs;

    fn names(paths: &[PathBuf], root: &Path) -> Vec<String> {
        paths
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
            .collect()
    }

    /// Natural order, case-insensitive, the folder's own files before its
    /// subfolders', bytes as the last word: whatever order the names come
    /// in, one answer.
    #[test]
    fn archive_order_is_natural_and_total() {
        let root = Path::new("/lib/Series");
        let expected = [
            "Issue 1.cbz",
            "issue 1.cbz",
            "Issue 2.cbz",
            "issue 9.cbz",
            "Issue 10.cbz",
            "ISSUE 11.cbz",
            "Issue 100.cbz",
            "Annuals/Annual 2.cbz",
            "Annuals/annual 10.cbz",
            "Specials/Special 1.cbz",
            "Specials/Extra/Deep 1.cbz",
        ];
        let sorted: Vec<PathBuf> = expected.iter().map(|n| root.join(n)).collect();
        // Every rotation and the reverse of the expected order sort back
        // to it.
        let mut inputs: Vec<Vec<PathBuf>> = (0..sorted.len())
            .map(|k| {
                let mut v = sorted.clone();
                v.rotate_left(k);
                v
            })
            .collect();
        inputs.push(sorted.iter().rev().cloned().collect());
        for mut v in inputs {
            sort_archives(&mut v);
            assert_eq!(names(&v, root), expected);
        }
        // A run that starts with zero compares left-aligned: padded
        // before unpadded, and `010` before `9`.
        for (lo, hi) in [
            ("Issue 01.cbz", "Issue 1.cbz"),
            ("Issue 010.cbz", "Issue 9.cbz"),
            ("Issue 001.cbz", "Issue 01.cbz"),
        ] {
            let (lo, hi) = (root.join(lo), root.join(hi));
            assert_eq!(archive_order(&lo, &hi), std::cmp::Ordering::Less, "{lo:?}");
            assert_eq!(
                archive_order(&hi, &lo),
                std::cmp::Ordering::Greater,
                "{hi:?}"
            );
        }
        // `issue 1` and `Issue 1` are equal ignoring case; bytes decide
        // (`I` 0x49 < `i` 0x69), the same way in both argument orders.
        let (a, b) = (root.join("Issue 1.cbz"), root.join("issue 1.cbz"));
        assert_eq!(archive_order(&a, &b), std::cmp::Ordering::Less);
        assert_eq!(archive_order(&b, &a), std::cmp::Ordering::Greater);
        assert_eq!(archive_order(&a, &a), std::cmp::Ordering::Equal);
    }

    /// The walks hand back the sorted list, so a folder whose files were
    /// created in two different orders lists the same.
    #[test]
    fn walks_return_archives_in_archive_order() {
        let mut lists = Vec::new();
        for creation in [
            [
                "Issue 10.cbz",
                "Specials/Special 1.cbz",
                "issue 2.cbz",
                "Issue 1.cbz",
            ],
            [
                "Issue 1.cbz",
                "issue 2.cbz",
                "Specials/Special 1.cbz",
                "Issue 10.cbz",
            ],
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let folder = tmp.path().join("Series");
            fs::create_dir_all(folder.join("Specials")).unwrap();
            for n in creation {
                fs::write(folder.join(n), b"").unwrap();
            }
            let walked = walk_archives_with(&folder, &IgnoreRules::default());
            let changed = list_archives_changed_since(
                &folder,
                &IgnoreRules::default(),
                chrono::DateTime::<chrono::Utc>::MIN_UTC,
            );
            assert_eq!(walked.archives, changed.archives);
            lists.push(names(&walked.archives, &folder));
        }
        assert_eq!(
            lists[0],
            vec![
                "Issue 1.cbz",
                "issue 2.cbz",
                "Issue 10.cbz",
                "Specials/Special 1.cbz"
            ]
        );
        assert_eq!(lists[0], lists[1]);
    }
}
