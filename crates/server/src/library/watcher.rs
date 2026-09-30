//! File watcher (WP-3.1 — roadmap §3.6, decision D1).
//!
//! One watcher per library whose `file_watch_enabled` flag is on, run
//! in-process (single-instance deploy, D2 — no leases, no pub/sub). The
//! watcher never scans, parses or hashes anything itself: it only works out
//! *which directories changed* and hands that set to the ordinary scan path
//! as a **scoped** scan ([`crate::jobs::JobRuntime::coalesce_watch_scan`] →
//! [`crate::library::scanner::scan_library_scoped`]). The scan then applies
//! the same `list_archives_changed_since` folder fast path and per-file
//! size+mtime fingerprint as a cron scan, so an archive is only re-hashed
//! when its fingerprint actually moved.
//!
//! Two modes, chosen per library at watcher start by `statfs(2)` on the root:
//!
//! - **inotify** (local filesystems): `notify` + `notify-debouncer-full`,
//!   one recursive watch on the root. Debounced events are folded into a set
//!   of touched directories; the set is flushed after
//!   `scanner.watch_debounce_secs` of quiet (default 30 s), or after ten
//!   debounce windows of continuous activity so an endless trickle still
//!   scans. A 10k-file copy is therefore one scan.
//! - **poll** (NFS / SMB / CIFS / FUSE / 9p / Ceph / AFS …, where inotify
//!   never sees writes made by another host): a directory-mtime poll every
//!   `scanner.watch_poll_interval_secs` (default 300 s). Each tick `stat`s
//!   every known *directory* — never a file — and only `readdir`s a
//!   directory whose mtime moved (see [`poll_directories`]). A directory
//!   whose mtime advanced is a touched directory. `COMIC_WATCH_FORCE_POLL`
//!   forces this mode everywhere; a failed inotify setup (e.g. the
//!   `max_user_watches` limit) also falls back to it.
//!
//! Directory mtimes only move when an entry is added, removed or renamed, so
//! the poller does not notice an archive overwritten *in place* on a network
//! share — the scheduled scan still covers that. inotify does see it.
//!
//! Event storms coalesce twice: the debounce window above, then the scan
//! coalescer's Redis `scan:in_flight` / `scan:queued` keys (a trigger that
//! lands while a scan runs is unioned into the one queued follow-up).
//!
//! The supervisor ([`spawn_supervisor`]) reconciles running watchers
//! against the `library` table every 30 s and immediately when nudged
//! ([`WatcherRegistry::nudge`] — library create / update / delete and the
//! settings PATCH), so toggling the flag, moving the root, or changing the
//! timing takes effect without a restart. Per-library mode and last trigger
//! are kept in [`WatcherRegistry`] and served by
//! `GET /admin/server/watchers` for the admin scan dashboard.

use crate::library::ignore::{IgnoreRules, is_recognized_archive_ext};
use crate::state::AppState;
use chrono::{DateTime, Utc};
use entity::library;
use notify_debouncer_full::notify::{self, EventKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, new_debouncer};
use sea_orm::EntityTrait;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// How often the supervisor re-reads the `library` table when nobody nudges
/// it. Nudges make toggles immediate; this is the safety net.
const SUPERVISOR_RESYNC: Duration = Duration::from_secs(30);

/// Upper bound on the notify-debouncer's own per-path window. The debouncer
/// only de-duplicates rename/create/modify chatter per path; the real
/// collapse window is `WatchOptions::debounce`, applied over the whole tree.
const DEBOUNCER_TICK_CAP: Duration = Duration::from_secs(2);

/// Continuous activity flushes after this many debounce windows even if
/// events never go quiet, so a long-running copy still produces a scan.
const MAX_WAIT_WINDOWS: u32 = 10;

// ───────────────────────── public status types ─────────────────────────

/// How a library is being watched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WatchMode {
    /// Kernel change notifications (inotify) on a local filesystem.
    Inotify,
    /// Directory-mtime poll on a network / FUSE mount.
    Poll,
    /// Not watched: the library's toggle is off, or the watcher could not
    /// start (see `detail`).
    Disabled,
}

impl WatchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inotify => "inotify",
            Self::Poll => "poll",
            Self::Disabled => "disabled",
        }
    }
}

/// Live state of one library's watcher. In-memory only (single instance).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct WatcherStatus {
    pub mode: WatchMode,
    /// Filesystem type `statfs` reported for the root (`ext4`, `nfs`,
    /// `cifs`, …, or `0x…` for an unrecognised magic). `None` when the
    /// watcher is disabled or detection failed.
    pub filesystem: Option<String>,
    /// When the current watcher started.
    pub started_at: Option<DateTime<Utc>>,
    /// Most recent relevant filesystem change seen (inotify) or detected
    /// (poll).
    pub last_event_at: Option<DateTime<Utc>>,
    /// Most recent time the watcher handed a change set to the scanner.
    pub last_trigger_at: Option<DateTime<Utc>>,
    /// Number of touched directories in that change set (0 for a
    /// queue-overflow rescan, which scans the whole library).
    pub last_trigger_dirs: u32,
    /// Scan run the last trigger enqueued or joined.
    pub last_scan_id: Option<Uuid>,
    /// True when the last trigger joined an already-running scan (its
    /// directories ride the queued follow-up) instead of enqueuing one.
    pub last_trigger_coalesced: bool,
    /// Triggers since this watcher started.
    pub triggers_total: u64,
    /// Operator-facing note: why the watcher is disabled, why it fell back
    /// to polling, or the last watch error.
    pub detail: Option<String>,
}

impl WatcherStatus {
    fn disabled(detail: Option<String>) -> Self {
        Self {
            mode: WatchMode::Disabled,
            filesystem: None,
            started_at: None,
            last_event_at: None,
            last_trigger_at: None,
            last_trigger_dirs: 0,
            last_scan_id: None,
            last_trigger_coalesced: false,
            triggers_total: 0,
            detail,
        }
    }
}

/// Status for a library the registry has no entry for: the supervisor
/// hasn't evaluated it yet (just created, or the process is a test harness
/// that never started the supervisor).
pub fn not_running_status(enabled: bool) -> WatcherStatus {
    WatcherStatus::disabled(enabled.then(|| "watcher not running yet".to_owned()))
}

// ───────────────────────── mount detection ─────────────────────────

/// What `statfs` says about a library root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountInfo {
    /// Human name of the filesystem (`ext4`, `nfs`, …) or `0x<magic>`.
    pub filesystem: String,
    /// True for filesystems where inotify can't observe writes made by
    /// another host (or by the FUSE daemon's backend).
    pub network: bool,
}

/// `statfs(2)` the root and classify its filesystem magic.
pub fn detect_mount(root: &Path) -> std::io::Result<MountInfo> {
    let st = rustix::fs::statfs(root).map_err(std::io::Error::from)?;
    // `f_type` is signed on some targets; the magic numbers are 32-bit.
    Ok(classify_fs_magic(fs_magic(st.f_type)))
}

/// Normalise `f_type` (an `i64` on 64-bit Linux, narrower elsewhere) to the
/// 32-bit magic `linux/magic.h` defines. Generic so the conversion stays
/// portable across targets without a same-type `From` on 64-bit.
fn fs_magic<T: Into<i64>>(f_type: T) -> u64 {
    f_type.into().cast_unsigned() & 0xFFFF_FFFF
}

/// Map a `statfs` `f_type` magic to a [`MountInfo`]. Values from
/// `linux/magic.h` (plus the SMB2/CIFS magics `fs/smb` defines locally).
pub fn classify_fs_magic(magic: u64) -> MountInfo {
    let (name, network): (&str, bool) = match magic {
        // Network / remote — inotify is blind to other hosts' writes.
        0x6969 => ("nfs", true),
        0x517B => ("smb", true),
        0xFE53_4D42 => ("smb2", true),
        0xFF53_4D42 => ("cifs", true),
        0x6573_5546 => ("fuse", true),
        0x0102_1997 => ("9p", true),
        0x00C3_6400 => ("ceph", true),
        0x5346_414F => ("afs", true),
        0x6B41_4653 => ("kafs", true),
        0x7375_7245 => ("coda", true),
        0x564C => ("ncp", true),
        0x0BD0_0BD0 => ("lustre", true),
        0x4750_4653 => ("gpfs", true),
        0x0BAD_1DEA => ("glusterfs", true),
        // Common local filesystems (named for the dashboard only).
        0xEF53 => ("ext4", false),
        0x5846_5342 => ("xfs", false),
        0x9123_683E => ("btrfs", false),
        0x2FC1_2FC1 => ("zfs", false),
        0x0102_1994 => ("tmpfs", false),
        0x794C_7630 => ("overlayfs", false),
        0xF2F5_2010 => ("f2fs", false),
        0x4D44 => ("vfat", false),
        0x2011_BAB0 => ("exfat", false),
        0x5346_544E => ("ntfs", false),
        0x7366_746E => ("ntfs3", false),
        0x3434 => ("nilfs", false),
        0x5265_4973 => ("reiserfs", false),
        0x9660 => ("iso9660", false),
        0x7371_7368 => ("squashfs", false),
        _ => {
            return MountInfo {
                filesystem: format!("{magic:#x}"),
                network: false,
            };
        }
    };
    MountInfo {
        filesystem: name.to_owned(),
        network,
    }
}

// ───────────────────────── change collection ─────────────────────────

/// Touched directories accumulated over one debounce window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TouchedDirs {
    pub dirs: BTreeSet<PathBuf>,
    /// The kernel queue overflowed (or the backend asked for a rescan):
    /// events were lost, so the change set is unknown and a non-forced
    /// full scan is the only safe answer.
    pub rescan: bool,
}

impl TouchedDirs {
    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty() && !self.rescan
    }

    /// Fold one notify event in. Returns true when it touched anything.
    pub fn add_event(&mut self, root: &Path, ignore: &IgnoreRules, event: &notify::Event) -> bool {
        if event.need_rescan() {
            self.rescan = true;
            return true;
        }
        if matches!(event.kind, EventKind::Access(_)) {
            return false;
        }
        let mut any = false;
        for path in &event.paths {
            any |= self.add_path(root, ignore, path);
        }
        any
    }

    /// Fold one changed path in: the directory whose listing it lives in
    /// (and, for a directory, the directory itself). Only archives,
    /// `series.json` sidecars and directories count — temp files, `.bak`
    /// backups, dotfiles and ignored paths never trigger a scan.
    pub fn add_path(&mut self, root: &Path, ignore: &IgnoreRules, path: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(root) else {
            return false;
        };
        if rel.as_os_str().is_empty() {
            // An event on the root directory itself (attribute change) says
            // nothing about which entry moved; the child's own event does.
            return false;
        }
        for comp in rel.components() {
            let name = comp.as_os_str().to_string_lossy();
            if name.starts_with('.')
                || matches!(
                    name.as_ref(),
                    "__MACOSX" | "@eaDir" | "Thumbs.db" | "desktop.ini"
                )
            {
                return false;
            }
        }
        if ignore.should_skip(path) {
            return false;
        }
        let parent = path.parent().unwrap_or(root).to_path_buf();
        // `symlink_metadata` stats the changed entry itself. Only reached in
        // inotify mode (local disk); the network poller never stats files.
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => {
                self.dirs.insert(path.to_path_buf());
                self.dirs.insert(parent);
                true
            }
            Ok(meta) if meta.is_file() => {
                if is_relevant_file(path) {
                    self.dirs.insert(parent);
                    true
                } else {
                    false
                }
            }
            Ok(_) => false,
            // Gone (delete / rename-away): an archive, sidecar or an
            // extension-less name (almost always a directory) matters.
            Err(_) => {
                if is_relevant_file(path) || path.extension().is_none() {
                    self.dirs.insert(parent);
                    true
                } else {
                    false
                }
            }
        }
    }
}

fn is_relevant_file(path: &Path) -> bool {
    if path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("series.json"))
    {
        return true;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(is_recognized_archive_ext)
}

// ───────────────────────── directory poller ─────────────────────────

/// One entry of a directory listing, typed from the dirent alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirChild {
    pub name: std::ffi::OsString,
    pub is_dir: bool,
}

/// The poller's only window onto the filesystem. Split out so tests can
/// count calls and prove the network-mount path stats directories only.
pub trait DirProbe: Send + Sync + 'static {
    /// `stat(2)` one directory and return its mtime. Only ever called with
    /// directory paths.
    fn dir_mtime(&self, dir: &Path) -> std::io::Result<SystemTime>;
    /// `readdir(3)` one directory. Entries are typed from the dirent
    /// (`d_type`) — never a per-entry `stat`. Symlinks count as non-dirs
    /// (not followed). Only ever called with directory paths.
    fn list(&self, dir: &Path) -> std::io::Result<Vec<DirChild>>;
}

/// Real-filesystem [`DirProbe`].
#[derive(Debug, Default, Clone, Copy)]
pub struct FsDirProbe;

impl DirProbe for FsDirProbe {
    fn dir_mtime(&self, dir: &Path) -> std::io::Result<SystemTime> {
        std::fs::metadata(dir)?.modified()
    }

    fn list(&self, dir: &Path) -> std::io::Result<Vec<DirChild>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let Ok(entry) = entry else { continue };
            // `DirEntry::file_type` comes from `d_type`; std only falls back
            // to an lstat when the filesystem reports DT_UNKNOWN.
            let is_dir = entry.file_type().is_ok_and(|ft| ft.is_dir());
            out.push(DirChild {
                name: entry.file_name(),
                is_dir,
            });
        }
        Ok(out)
    }
}

#[derive(Debug, Clone)]
struct DirEntryState {
    mtime: SystemTime,
    /// When this mtime was observed. An mtime within the filesystem's
    /// timestamp granularity of the observation is "racy": an entry added
    /// in the same clock tick would leave it unchanged. A racy directory is
    /// re-listed and its entry names compared instead of trusting the mtime.
    observed_at: SystemTime,
    /// Order-independent hash of the entry names (files and dirs).
    names_hash: u64,
    children: Vec<PathBuf>,
}

/// Directory-mtime snapshot of one library tree.
#[derive(Debug, Clone, Default)]
pub struct DirSnapshot {
    dirs: HashMap<PathBuf, DirEntryState>,
}

impl DirSnapshot {
    pub fn len(&self) -> usize {
        self.dirs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }
}

/// Granularity slack for the racy-mtime check (NFS/SMB commonly report
/// 1-second — and FAT 2-second — timestamps).
const RACY_MTIME_SLACK: Duration = Duration::from_secs(2);

fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "__MACOSX" | "@eaDir")
}

/// List `dir` via the probe: (visible subdirectories, entry-name hash).
fn list_dir(probe: &dyn DirProbe, dir: &Path, ignore: &IgnoreRules) -> (Vec<PathBuf>, u64) {
    use std::hash::{Hash, Hasher};
    let entries = probe.list(dir).unwrap_or_default();
    let mut names_hash = 0_u64;
    let mut children = Vec::new();
    for e in entries {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        e.name.hash(&mut h);
        // XOR keeps the hash independent of readdir order.
        names_hash ^= h.finish();
        if e.is_dir && !is_hidden_name(&e.name.to_string_lossy()) {
            let path = dir.join(&e.name);
            if !ignore.should_skip(&path) {
                children.push(path);
            }
        }
    }
    (children, names_hash)
}

/// One poll pass. `stat`s every directory in the tree (via `probe`) — never
/// a file — and lists only directories that are new, whose mtime moved, or
/// whose mtime is too fresh to trust (racy). An unchanged directory's
/// subdirectory list is reused from `prev`. Returns the new snapshot and the
/// directories whose entries changed since `prev` (empty on the baseline
/// pass, when `prev` is `None`). Errors only when the root itself can't be
/// stat'ed.
pub fn poll_directories(
    probe: &dyn DirProbe,
    root: &Path,
    ignore: &IgnoreRules,
    prev: Option<&DirSnapshot>,
) -> std::io::Result<(DirSnapshot, Vec<PathBuf>)> {
    let now = SystemTime::now();
    let mut next = DirSnapshot::default();
    let mut changed = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mtime = match probe.dir_mtime(&dir) {
            Ok(m) => m,
            Err(e) if dir == root => return Err(e),
            // Vanished between the parent's listing and now; the parent's
            // mtime moved, so the change is still reported there.
            Err(_) => continue,
        };
        let prior = prev.and_then(|p| p.dirs.get(&dir));
        let state = match prior {
            Some(p) if p.mtime == mtime => {
                let racy = p
                    .observed_at
                    .duration_since(p.mtime)
                    .map_or(true, |d| d <= RACY_MTIME_SLACK);
                if racy {
                    let (children, names_hash) = list_dir(probe, &dir, ignore);
                    if names_hash != p.names_hash {
                        changed.push(dir.clone());
                    }
                    DirEntryState {
                        mtime,
                        observed_at: now,
                        names_hash,
                        children,
                    }
                } else {
                    p.clone()
                }
            }
            _ => {
                if prev.is_some() {
                    changed.push(dir.clone());
                }
                let (children, names_hash) = list_dir(probe, &dir, ignore);
                DirEntryState {
                    mtime,
                    observed_at: now,
                    names_hash,
                    children,
                }
            }
        };
        stack.extend(state.children.iter().cloned());
        next.dirs.insert(dir, state);
    }
    changed.sort();
    Ok((next, changed))
}

// ───────────────────────── registry + options ─────────────────────────

/// Which mode to run, before mount detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeChoice {
    /// `statfs` decides (the production default).
    Auto,
    /// Always poll (`COMIC_WATCH_FORCE_POLL`, and tests).
    ForcePoll,
}

/// Per-watcher knobs. [`WatchOptions::from_config`] is the production path;
/// tests build one directly to shorten the windows and swap the probe.
#[derive(Clone)]
pub struct WatchOptions {
    pub mode: ModeChoice,
    /// Quiet period before a flush (inotify).
    pub debounce: Duration,
    /// Longest a window may stay open under continuous activity.
    pub max_wait: Duration,
    /// Directory-mtime poll cadence (poll mode).
    pub poll_interval: Duration,
    pub probe: Arc<dyn DirProbe>,
}

impl std::fmt::Debug for WatchOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchOptions")
            .field("mode", &self.mode)
            .field("debounce", &self.debounce)
            .field("max_wait", &self.max_wait)
            .field("poll_interval", &self.poll_interval)
            .finish_non_exhaustive()
    }
}

impl WatchOptions {
    pub fn from_config(cfg: &crate::config::Config) -> Self {
        let debounce = Duration::from_secs(cfg.watch_debounce_secs.max(1));
        Self {
            mode: if cfg.watch_force_poll {
                ModeChoice::ForcePoll
            } else {
                ModeChoice::Auto
            },
            debounce,
            max_wait: debounce * MAX_WAIT_WINDOWS,
            poll_interval: Duration::from_secs(cfg.watch_poll_interval_secs.max(1)),
            probe: Arc::new(FsDirProbe),
        }
    }
}

/// Everything that, when it changes, requires restarting a watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchSpec {
    root: String,
    ignore_globs: serde_json::Value,
    debounce_secs: u64,
    poll_secs: u64,
    force_poll: bool,
}

struct RunningWatcher {
    spec: WatchSpec,
    handle: WatcherHandle,
}

/// Process-wide watcher registry, owned by [`AppState`].
#[derive(Default)]
pub struct WatcherRegistry {
    statuses: RwLock<HashMap<Uuid, WatcherStatus>>,
    running: Mutex<HashMap<Uuid, RunningWatcher>>,
    nudge: Notify,
}

impl WatcherRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the supervisor to re-sync now (library toggled / moved / deleted,
    /// watcher settings changed). Cheap and idempotent.
    pub fn nudge(&self) {
        self.nudge.notify_one();
    }

    /// Current status for one library, if a watcher ever ran or was
    /// evaluated for it.
    pub fn status(&self, library_id: Uuid) -> Option<WatcherStatus> {
        self.statuses
            .read()
            .ok()
            .and_then(|m| m.get(&library_id).cloned())
    }

    /// Snapshot of every tracked library's status.
    pub fn statuses(&self) -> HashMap<Uuid, WatcherStatus> {
        self.statuses.read().map(|m| m.clone()).unwrap_or_default()
    }

    fn set(&self, library_id: Uuid, status: WatcherStatus) {
        if let Ok(mut m) = self.statuses.write() {
            m.insert(library_id, status);
        }
    }

    fn update(&self, library_id: Uuid, f: impl FnOnce(&mut WatcherStatus)) {
        if let Ok(mut m) = self.statuses.write()
            && let Some(s) = m.get_mut(&library_id)
        {
            f(s);
        }
    }

    fn remove(&self, library_id: Uuid) {
        if let Ok(mut m) = self.statuses.write() {
            m.remove(&library_id);
        }
    }
}

/// Handle to one running library watcher. Dropping it cancels the watcher;
/// [`WatcherHandle::stop`] also waits for the task to finish.
#[derive(Debug)]
pub struct WatcherHandle {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
    mode: WatchMode,
}

impl WatcherHandle {
    pub fn mode(&self) -> WatchMode {
        self.mode
    }

    pub async fn stop(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

// ───────────────────────── watcher tasks ─────────────────────────

/// Start watching one library root. Detects the mount type (unless the mode
/// is forced), sets up inotify or the poller, records the status, and
/// returns a handle whose drop stops the watcher. Fails when the root can't
/// be inspected at all (missing / unreadable mount).
pub async fn start_library_watcher(
    state: &AppState,
    library_id: Uuid,
    root: PathBuf,
    ignore: IgnoreRules,
    opts: WatchOptions,
) -> Result<WatcherHandle, String> {
    let root_for_detect = root.clone();
    let mount = tokio::task::spawn_blocking(move || detect_mount(&root_for_detect))
        .await
        .map_err(|e| format!("mount detection task failed: {e}"))?;
    let mount = match (mount, opts.mode) {
        (Ok(m), _) => Some(m),
        (Err(e), ModeChoice::Auto) => {
            return Err(format!("library root unavailable: {e}"));
        }
        (Err(_), ModeChoice::ForcePoll) => None,
    };
    let filesystem = mount.as_ref().map(|m| m.filesystem.clone());

    let mut detail = None;
    let want_inotify = opts.mode == ModeChoice::Auto && mount.as_ref().is_some_and(|m| !m.network);
    if opts.mode == ModeChoice::ForcePoll {
        detail = Some("polling forced (COMIC_WATCH_FORCE_POLL)".to_owned());
    } else if let Some(m) = mount.as_ref().filter(|m| m.network) {
        detail = Some(format!(
            "network filesystem ({}); inotify can't see remote writes, polling directory mtimes",
            m.filesystem
        ));
    }

    let cancel = CancellationToken::new();
    if want_inotify {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DebounceEventResult>();
        let tick = opts
            .debounce
            .min(DEBOUNCER_TICK_CAP)
            .max(Duration::from_millis(50));
        let root_for_watch = root.clone();
        // Adding a recursive inotify watch walks the directory tree once;
        // keep that off the async runtime.
        let setup = tokio::task::spawn_blocking(move || {
            let mut debouncer = new_debouncer(tick, None, move |res: DebounceEventResult| {
                let _ = tx.send(res);
            })?;
            debouncer.watch(&root_for_watch, RecursiveMode::Recursive)?;
            Ok::<_, notify::Error>(debouncer)
        })
        .await
        .map_err(|e| format!("watch setup task failed: {e}"))?;
        match setup {
            Ok(debouncer) => {
                state.watchers.set(
                    library_id,
                    WatcherStatus {
                        mode: WatchMode::Inotify,
                        filesystem,
                        started_at: Some(Utc::now()),
                        ..WatcherStatus::disabled(None)
                    },
                );
                let task = tokio::spawn(run_inotify(
                    state.clone(),
                    library_id,
                    root,
                    ignore,
                    opts,
                    debouncer,
                    rx,
                    cancel.clone(),
                ));
                tracing::info!(library_id = %library_id, "file watcher started (inotify)");
                return Ok(WatcherHandle {
                    cancel,
                    task: Some(task),
                    mode: WatchMode::Inotify,
                });
            }
            Err(e) => {
                tracing::warn!(
                    library_id = %library_id,
                    error = %e,
                    "inotify watch failed; falling back to directory-mtime polling",
                );
                detail = Some(format!(
                    "inotify unavailable ({e}); polling directory mtimes instead"
                ));
            }
        }
    }

    state.watchers.set(
        library_id,
        WatcherStatus {
            mode: WatchMode::Poll,
            filesystem,
            started_at: Some(Utc::now()),
            ..WatcherStatus::disabled(detail)
        },
    );
    let task = tokio::spawn(run_poll(
        state.clone(),
        library_id,
        root,
        ignore,
        opts,
        cancel.clone(),
    ));
    tracing::info!(library_id = %library_id, "file watcher started (poll)");
    Ok(WatcherHandle {
        cancel,
        task: Some(task),
        mode: WatchMode::Poll,
    })
}

#[expect(clippy::too_many_arguments)]
async fn run_inotify<D: Send + 'static>(
    state: AppState,
    library_id: Uuid,
    root: PathBuf,
    ignore: IgnoreRules,
    opts: WatchOptions,
    // Held for the task's lifetime: dropping the debouncer stops the watch.
    debouncer: D,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<DebounceEventResult>,
    cancel: CancellationToken,
) {
    let mut pending = TouchedDirs::default();
    let mut window_opened: Option<Instant> = None;
    let mut last_event = Instant::now();
    loop {
        let deadline =
            window_opened.map(|opened| (last_event + opts.debounce).min(opened + opts.max_wait));
        let sleep_until = deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(3600));
        tokio::select! {
            () = cancel.cancelled() => break,
            msg = rx.recv() => {
                match msg {
                    None => break,
                    Some(Ok(events)) => {
                        let mut touched = false;
                        for ev in &events {
                            touched |= pending.add_event(&root, &ignore, ev);
                        }
                        if touched {
                            let now = Instant::now();
                            window_opened.get_or_insert(now);
                            last_event = now;
                            state.watchers.update(library_id, |s| s.last_event_at = Some(Utc::now()));
                        }
                    }
                    Some(Err(errors)) => {
                        for e in &errors {
                            tracing::warn!(library_id = %library_id, error = %e, "file watcher error");
                        }
                        if let Some(e) = errors.first() {
                            let msg = format!("watch error: {e}");
                            state.watchers.update(library_id, |s| s.detail = Some(msg));
                        }
                    }
                }
            }
            () = tokio::time::sleep_until(sleep_until), if deadline.is_some() => {
                let touched = std::mem::take(&mut pending);
                window_opened = None;
                if !touched.is_empty() {
                    fire(&state, library_id, touched).await;
                }
            }
        }
    }
    // Drop (stop) the debouncer off the runtime: its thread join can block.
    let _ = tokio::task::spawn_blocking(move || drop(debouncer)).await;
    tracing::info!(library_id = %library_id, "file watcher stopped (inotify)");
}

async fn run_poll(
    state: AppState,
    library_id: Uuid,
    root: PathBuf,
    ignore: IgnoreRules,
    opts: WatchOptions,
    cancel: CancellationToken,
) {
    let mut interval = tokio::time::interval(opts.poll_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut snapshot: Option<DirSnapshot> = None;
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            _ = interval.tick() => {
                let probe = Arc::clone(&opts.probe);
                let root_c = root.clone();
                let ignore_c = ignore.clone();
                let prev = snapshot.take();
                let res = tokio::task::spawn_blocking(move || {
                    let out = poll_directories(probe.as_ref(), &root_c, &ignore_c, prev.as_ref());
                    (out, prev)
                })
                .await;
                let Ok((out, prev)) = res else {
                    tracing::warn!(library_id = %library_id, "directory poll task panicked");
                    continue;
                };
                match out {
                    Ok((snap, changed)) => {
                        snapshot = Some(snap);
                        if !changed.is_empty() {
                            state.watchers.update(library_id, |s| s.last_event_at = Some(Utc::now()));
                            let touched = TouchedDirs {
                                dirs: changed.into_iter().collect(),
                                rescan: false,
                            };
                            fire(&state, library_id, touched).await;
                        }
                    }
                    Err(e) => {
                        // Root unreachable (share dropped). Keep the old
                        // snapshot so the next good pass diffs against it.
                        snapshot = prev;
                        let msg = format!("library root unreadable: {e}");
                        tracing::warn!(library_id = %library_id, error = %e, "directory poll failed");
                        state.watchers.update(library_id, |s| s.detail = Some(msg));
                    }
                }
            }
        }
    }
    tracing::info!(library_id = %library_id, "file watcher stopped (poll)");
}

/// Hand one change set to the scan coalescer and record the trigger.
async fn fire(state: &AppState, library_id: Uuid, touched: TouchedDirs) {
    let dir_count = u32::try_from(touched.dirs.len()).unwrap_or(u32::MAX);
    let outcome = if touched.rescan {
        tracing::info!(library_id = %library_id, "file watcher: event queue overflowed; full scan");
        state.jobs.coalesce_scan(library_id, false).await
    } else {
        let dirs = touched
            .dirs
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect();
        state.jobs.coalesce_watch_scan(library_id, dirs).await
    };
    match outcome {
        Ok(outcome) => {
            let mode = state
                .watchers
                .status(library_id)
                .map_or("unknown", |s| s.mode.as_str());
            metrics::counter!("folio_watcher_triggers_total", "mode" => mode).increment(1);
            tracing::info!(
                library_id = %library_id,
                dirs = dir_count,
                scan_id = %outcome.scan_id(),
                coalesced = outcome.was_coalesced(),
                "file watcher triggered a scoped scan",
            );
            state.watchers.update(library_id, |s| {
                s.last_trigger_at = Some(Utc::now());
                s.last_trigger_dirs = if touched.rescan { 0 } else { dir_count };
                s.last_scan_id = Some(outcome.scan_id());
                s.last_trigger_coalesced = outcome.was_coalesced();
                s.triggers_total += 1;
            });
        }
        Err(e) => {
            tracing::error!(library_id = %library_id, error = %e, "file watcher: enqueue scan failed");
            let msg = format!("enqueue scan failed: {e}");
            state.watchers.update(library_id, |s| s.detail = Some(msg));
        }
    }
}

// ───────────────────────── supervisor ─────────────────────────

/// Spawn the supervisor that keeps one watcher per enabled library running
/// until `shutdown` fires.
pub fn spawn_supervisor(state: AppState, shutdown: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            sync(&state).await;
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = state.watchers.nudge.notified() => {}
                () = tokio::time::sleep(SUPERVISOR_RESYNC) => {}
            }
        }
        let running: Vec<RunningWatcher> = state
            .watchers
            .running
            .lock()
            .await
            .drain()
            .map(|(_, r)| r)
            .collect();
        for r in running {
            r.handle.stop().await;
        }
        tracing::info!("file watcher supervisor stopped");
    })
}

/// Reconcile running watchers against the `library` table and the live
/// config. Public so tests (and a future admin "re-sync" button) can drive
/// it deterministically.
pub async fn sync(state: &AppState) {
    let libs = match library::Entity::find().all(&state.db).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "file watcher supervisor: library query failed");
            return;
        }
    };
    let cfg = state.cfg();
    let mut running = state.watchers.running.lock().await;

    let desired: HashMap<Uuid, (WatchSpec, library::Model)> = libs
        .iter()
        .filter(|l| l.file_watch_enabled)
        .map(|l| {
            (
                l.id,
                (
                    WatchSpec {
                        root: l.root_path.clone(),
                        ignore_globs: l.ignore_globs.clone(),
                        debounce_secs: cfg.watch_debounce_secs,
                        poll_secs: cfg.watch_poll_interval_secs,
                        force_poll: cfg.watch_force_poll,
                    },
                    l.clone(),
                ),
            )
        })
        .collect();

    // Stop watchers that are no longer wanted or whose spec changed.
    let stale: Vec<Uuid> = running
        .iter()
        .filter(|(id, r)| desired.get(id).is_none_or(|(spec, _)| *spec != r.spec))
        .map(|(id, _)| *id)
        .collect();
    for id in stale {
        if let Some(r) = running.remove(&id) {
            r.handle.stop().await;
        }
    }

    // Disabled / deleted libraries.
    let known: std::collections::HashSet<Uuid> = libs.iter().map(|l| l.id).collect();
    for (id, _) in state.watchers.statuses() {
        if !known.contains(&id) {
            state.watchers.remove(id);
        }
    }
    for lib in libs.iter().filter(|l| !l.file_watch_enabled) {
        state.watchers.set(lib.id, WatcherStatus::disabled(None));
    }

    // Start what's missing.
    for (id, (spec, lib)) in desired {
        if running.contains_key(&id) {
            continue;
        }
        let ignore = match IgnoreRules::for_library(&lib) {
            Ok(i) => i,
            Err(e) => {
                state.watchers.set(
                    id,
                    WatcherStatus::disabled(Some(format!("ignore_globs invalid: {e}"))),
                );
                continue;
            }
        };
        match start_library_watcher(
            state,
            id,
            PathBuf::from(&lib.root_path),
            ignore,
            WatchOptions::from_config(&cfg),
        )
        .await
        {
            Ok(handle) => {
                running.insert(id, RunningWatcher { spec, handle });
            }
            Err(e) => {
                let prior = state.watchers.status(id).and_then(|s| s.detail);
                let detail = format!("watcher could not start: {e}");
                if prior.as_deref() != Some(detail.as_str()) {
                    tracing::warn!(library_id = %id, error = %e, "file watcher could not start");
                }
                state
                    .watchers
                    .set(id, WatcherStatus::disabled(Some(detail)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_network_filesystems() {
        for magic in [0x6969_u64, 0xFF53_4D42, 0xFE53_4D42, 0x517B, 0x6573_5546] {
            assert!(classify_fs_magic(magic).network, "{magic:#x}");
        }
        for magic in [0xEF53_u64, 0x5846_5342, 0x9123_683E, 0x0102_1994] {
            assert!(!classify_fs_magic(magic).network, "{magic:#x}");
        }
        let unknown = classify_fs_magic(0x1234_5678);
        assert!(!unknown.network);
        assert_eq!(unknown.filesystem, "0x12345678");
    }

    #[test]
    fn touched_dirs_keep_only_relevant_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let series = root.join("Saga");
        std::fs::create_dir(&series).unwrap();
        std::fs::write(series.join("Saga 001.cbz"), b"x").unwrap();
        std::fs::write(series.join("Saga 001.cbz.tmp"), b"x").unwrap();
        let ignore = IgnoreRules::default();
        let mut t = TouchedDirs::default();
        assert!(t.add_path(root, &ignore, &series.join("Saga 001.cbz")));
        assert!(!t.add_path(root, &ignore, &series.join("Saga 001.cbz.tmp")));
        assert!(!t.add_path(root, &ignore, &series.join(".hidden.cbz")));
        assert!(!t.add_path(root, &ignore, root));
        assert!(!t.add_path(Path::new("/elsewhere"), &ignore, &series));
        assert_eq!(t.dirs.iter().collect::<Vec<_>>(), vec![&series]);
        // A new directory touches itself and its parent (the root).
        assert!(t.add_path(root, &ignore, &series));
        assert!(t.dirs.contains(&root.to_path_buf()));
        // A deleted archive still counts.
        let mut gone = TouchedDirs::default();
        assert!(gone.add_path(root, &ignore, &series.join("Saga 002.cbz")));
        assert!(gone.dirs.contains(&series));
    }

    #[test]
    fn poll_detects_added_and_removed_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let a = root.join("A");
        let b = root.join("B");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let ignore = IgnoreRules::default();
        let probe = FsDirProbe;
        let (base, changed) = poll_directories(&probe, root, &ignore, None).unwrap();
        assert!(changed.is_empty(), "baseline reports nothing");
        assert_eq!(base.len(), 3);
        // Fresh (racy) mtimes are re-listed, not blindly trusted — but an
        // unchanged listing still reads as unchanged.
        let (snap, changed) = poll_directories(&probe, root, &ignore, Some(&base)).unwrap();
        assert!(changed.is_empty(), "unchanged tree: {changed:?}");
        // Adding a file inside A changes A only (even within the same
        // mtime tick, via the racy re-list).
        std::fs::write(a.join("A 001.cbz"), b"x").unwrap();
        let (snap, changed) = poll_directories(&probe, root, &ignore, Some(&snap)).unwrap();
        assert_eq!(changed, vec![a.clone()]);
        // Removing B changes the root.
        std::fs::remove_dir(&b).unwrap();
        let (snap, changed) = poll_directories(&probe, root, &ignore, Some(&snap)).unwrap();
        assert_eq!(changed, vec![root.to_path_buf()]);
        assert_eq!(snap.len(), 2);
    }
}
