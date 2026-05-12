Library Scanner — Specification

Status: Draft v0.1 — companion to the main comic-reader-spec.md.
Scope: Defines the library scanning subsystem only. References the main spec for entity definitions (§5), parsers (§4), search indexing (§6), and ops behavior (§12).
Module location: crates/server/src/library/scanner/ (per main spec §14).


1. Purpose
The scanner is the bridge between the user's filesystem of .cbz files and the database. It must be:

Idempotent — running it twice produces identical state.
Incremental — only touches what has changed since the last scan.
Strict-by-default — the library type assumes well-tagged files (Mylar/ComicInfo-style); the scanner enforces structure and surfaces problems rather than silently coping.
Read-only — never modifies user files (per main spec §1.2).
Resumable — can be interrupted (crash, restart) and continue without data loss or duplication.


2. Library Type
This scanner targets a single library type: ComicInfo-driven. Users with this library type have embedded ComicInfo.xml in most or all files, typically produced by Mylar3, ComicTagger, or similar. This matches the structure expected by the CBL project and aligns with how library type works.
Other library types (manga without metadata, raw scans, mixed content) are out of scope for v1. A future "loose" library type with looser parsing rules is in the backlog (§13).
2.1 Required folder layout (note the starting volume years may be missing)
Library Root
├── Series Name (Starting Volume Year)/
│   ├── Series Name (Vol Year) #01.cbz
│   ├── Series Name (Vol Year) #02.cbz
│   └── Series Name (Vol Year) #03.cbz
├── Series Name 2 (Starting Volume Year)/
│   ├── Series Name 2 #01.cbz
│   ├── Series Name 2 #02.cbz
│   └── Series Name 2 #06.cbz
└── ...
2.2 Layout rules

Every series lives in a folder directly under the library root. Series folder name format: Series Name (Year).
No archive files at the library root. Files at root are flagged as errors and ignored.
No series split across sibling folders. Batman (2016) and Batman Vol 2 (2016) as separate top-level folders are two different series, not one.
Sub-folders inside a series folder are allowed but their contents are treated as belonging to the same series. Useful for "Annuals", "Specials", "Extras" sub-organization.
Multiple series in one folder is unsupported. If detected (multiple distinct ComicInfo Series values in one folder), the scanner logs a warning, picks the most common, and continues — but flags the folder for user review.

2.3 Layout violations
The scanner does not refuse to start on layout violations. Instead, it logs structured warnings and surfaces them in the admin UI under "Library health." See §10.

1. Scan Triggers
TriggerScopeWhenManual full scanAll series in a libraryUser clicks "Scan now" in admin UIManual series scanOne series folderUser clicks "Refresh" on a series pageScheduled scanAll series in a libraryCron-style schedule per library (default: every 6 hours)File watch eventAffected series folder(s)notify crate detects create/modify/deleteStartup scanAll libraries (optional)Server startup, only if COMIC_SCAN_ON_STARTUP=true (default false)
3.1 File watch debouncing

File events are debounced per-folder over a 30-second window. A burst of writes (e.g., copying 100 files) coalesces into one scan of the affected folder.
Watch is disabled if the library root is on a network mount where notify doesn't reliably work (NFS, SMB, rclone). Detected by trying to register a watch and falling back to schedule-only on failure. Logged at info.

3.2 Concurrency

Only one scan per library at a time. Subsequent triggers while a scan is running are coalesced (one queued; further triggers no-op).
Multiple libraries can scan in parallel.
Within a scan, series-level work runs in a bounded worker pool (configurable, default = min(cpu_count, 4)).


4. The Scan Loop
4.1 Overview
┌─────────────────────────────────────────────────┐
│ 1. Validate library                             │
│    - root exists, readable, non-empty           │
│    - if not: abort with structured error        │
└────────────────┬────────────────────────────────┘
                 ▼
┌─────────────────────────────────────────────────┐
│ 2. Enumerate series folders                     │
│    - list direct children of library root       │
│    - apply ignore globs (§5)                    │
│    - flag root-level files as errors            │
└────────────────┬────────────────────────────────┘
                 ▼
┌─────────────────────────────────────────────────┐
│ 3. Per series folder, in parallel:              │
│    a. Check folder mtime vs last scan           │
│    b. If unchanged: skip                        │
│    c. If changed: enumerate archive files       │
│    d. For each file: process (§6)               │
│    e. Resolve series identity & merge (§7)      │
│    f. Emit progress event over WebSocket        │
└────────────────┬────────────────────────────────┘
                 ▼
┌─────────────────────────────────────────────────┐
│ 4. Reconcile: handle deletions                  │
│    - issues whose paths no longer exist → mark  │
│      removed (soft delete; user confirms)       │
│    - empty series → mark removed                │
└────────────────┬────────────────────────────────┘
                 ▼
┌─────────────────────────────────────────────────┐
│ 5. Post-scan tasks (queued, async)              │
│    - thumbnail generation for new/changed       │
│    - search index refresh                       │
│    - relationship suggestion regeneration       │
│    - story arc auto-build                       │
│    - dictionary refresh (for "did you mean")    │
└─────────────────────────────────────────────────┘
4.2 Validation step (Step 1)

Library root exists and is readable.
Library root is non-empty (contains at least one entry).
Library root is not the same path as COMIC_DATA_PATH or any other configured library (prevents loops).
For series scans only: the series folder still exists; if not, the scan no-ops and logs at info (the library scan will handle deletion).

4.3 Enumeration step (Step 2)

Lists direct children of the library root.
Applies user-configured ignore globs (§5) and built-in ignore rules.
Files (not folders) at the root are flagged as LibraryHealthIssue::FileAtRoot { path }. They are not processed.
Hidden folders (starting with .) are ignored silently.

4.4 Per-folder change detection (Step 3a)

Compares folder's recursive last-modified timestamp against series.last_scanned_at in the database.
"Recursive last-modified" means: max(mtime) across all files and sub-folders. Calculated lazily, short-circuiting on the first newer-than-threshold entry.
Caveat surfaced to users: some operations (renaming, moving) don't update mtime on all systems. Admin UI exposes a "Force rescan this series" button that bypasses this check. The full-library equivalent is "Force rescan library" which sets every last_scanned_at to NULL.
A scan can be invoked with force: true programmatically to bypass mtime checks entirely.

4.5 Per-file processing (Step 3d)
See §6 for parsing details.
4.6 Series identity resolution (Step 3e)
See §7 for merging logic.
4.7 Reconciliation (Step 4)

After all folders are processed, the scanner queries Postgres for all issues belonging to scanned series and checks: does the file still exist on disk?
Missing files → soft-delete the issue: set removed_at, do not hard-delete. Surfaced in admin UI under "Removed since last scan" with a "Confirm deletion" / "Restore" choice. Auto-confirmed after 30 days.
A series with zero remaining issues is similarly soft-deleted.
This soft-delete approach prevents losing user progress, reviews, and bookmarks if a file is temporarily missing (e.g., NAS unmounted during scan).

4.8 Post-scan tasks (Step 5)

Enqueued as separate jobs in Redis (or in-process queue if Redis is disabled).
Run with lower priority than scan work so they don't block subsequent scans.
Each task is itself idempotent and incremental.


5. Ignore Rules
5.1 Built-in ignored patterns (always)
Applied to file and folder names at any depth:
^\.                  (dot files / dot folders)
^__MACOSX$
Thumbs\.db$
desktop\.ini$
\.DS_Store$
@eaDir              (Synology metadata)
Inside archives (during page enumeration), additionally:
\.xml$ \.json$ \.txt$ \.nfo$
(These are metadata files, not pages.)
5.2 User-configured ignore globs

Per-library ignore_globs setting (TOML/env or admin UI).
Standard glob syntax (via the globset crate). Examples:

  **/Annuals/**
  **/*.tmp
  **/Promos/*.cbz

Applied at enumeration time. Excluded files/folders never appear in any subsequent step.

5.3 Recognized archive extensions
Only files matching these extensions are considered for parsing:
.cbz .cbr .cb7 .cbt .epub
Plus folders containing only image files (folder-of-images, treated as a single issue).
Other files are ignored silently (not flagged — they're presumed to be incidental).

6. File Processing
6.1 Per-file pipeline
For each archive file in a changed series folder:
1. Compute content hash (BLAKE3 over file bytes, streamed)
2. Check DB: does an issue with this hash already exist?
   - Yes, same path        → update last_scanned_at, skip processing
   - Yes, different path   → file was moved; update path, log info
   - No                    → continue
3. Open archive, list entries, read ComicInfo.xml if present
4. Parse ComicInfo.xml (if present) — see main spec §4.2
5. Extract page list (image entries, naturally sorted)
6. Determine cover page (first image, or per-page metadata if specified)
7. Compute issue identity (Series, Volume, Number, etc.) — see §6.3
8. Insert or update issue row in DB
9. Enqueue thumbnail generation job
6.2 Hash computation

BLAKE3 over the full file bytes.
For large libraries on slow storage, hashing dominates scan time. Mitigated by:

Skipping files whose (path, size, mtime) triple matches a previously-hashed file.
Streaming hash (no full-file read into memory).


Hash mismatch with same path → file was modified in place; treat as new issue, archive the old issue's metadata under a superseded_by link for user review.

6.3 Issue identity (Title, Number, Volume)
Resolution order (first non-empty wins):

ComicInfo.xml — Series, Number, Volume fields.
Folder name — parsed as Series Name (Year).
Filename — parsed via the filename inference parser (main spec §4.5).

If steps 1 and 2 disagree (e.g., folder says Batman (2016) but ComicInfo says Batman: Rebirth), ComicInfo wins and the discrepancy is logged as LibraryHealthIssue::FolderNameMismatch.
6.4 Volume handling

ComicInfo Volume is expected to be a year (Mylar/CBL convention) but may legitimately be a small integer (1, 2, 3) for series with multiple volumes per year.
If Volume >= 1900, treat as year.
If Volume < 1900, treat as sequence number; combine with folder year for full identification (e.g., "Batman (2016) Vol 2").
If absent in ComicInfo, fall back to folder name's (Year) capture group.

6.5 Specials and one-shots
Detection heuristics, applied in order:

ComicInfo Format field equals Special, One-Shot, Annual, TPB, etc. → use that.
Filename contains _SP_ or SP or Special → mark as Special.
Filename contains Annual or _Annual_ → mark as Annual.
Filename has no recognizable issue number → mark as One-Shot.

Specials and annuals are stored as issues with a special_type enum field. They appear in the series in a separate "Specials" section of the UI (deferred to main spec phase 1+).
6.6 Page enumeration

Archive entries listed and filtered to image extensions (.jpg .jpeg .png .webp .avif .jxl .gif).
Sorted by natural sort (numeric-aware) on entry name, then case-insensitive lex tiebreaker.
Per-page metadata from ComicInfo <Pages> is matched to enumerated pages by index. Mismatch (e.g., ComicInfo says 22 pages, archive has 24) logs a warning and uses the actual archive contents.
Page list stored as JSONB on the Issue row (per main spec §5.1).

6.7 series.json

Read once per series folder (at the folder root, not inside archives).
Used to populate series-level metadata (publisher, year_began, year_end, status, etc.).
Precedence: per-issue ComicInfo > series.json > inferred (per main spec §4.3).

6.8 MetronInfo.xml

Read alongside ComicInfo.xml when present.
For overlapping fields, MetronInfo wins (per main spec §4.4).


7. Series Identity & Merging
7.1 Finding the series
For each processed file, the scanner needs to determine which series row it belongs to. Resolution:

By stable folder path — if the series folder path matches an existing series.folder_path, use that series. (Fast path; covers the common case.)
By ComicInfo LocalizedSeries tag — if any file in this folder has LocalizedSeries matching an existing series's name, treat as the same series.
By normalized name + year — normalize(series_name) + start_year matched against existing series.
None matched — create a new series.

7.2 Series merging

If multiple files in one folder have different Series values, they should not exist together (per §2.2) but the scanner tolerates it:

Pick the most common Series value as the folder's primary series.
Other files in the folder are still attributed to the primary series, with LibraryHealthIssue::MixedSeriesInFolder flagged.


If two folders resolve to the same series via §7.1.2 (LocalizedSeries), they are auto-merged. The merge is logged and reversible from the admin UI.

7.3 Renames and moves

A series folder renamed on disk: detected when the old series.folder_path no longer exists but a new folder with matching ComicInfo Series does. Auto-relinked; logged at info.
An issue file moved between series folders: detected by hash match. Issue row's series_id updated to the new series; user progress preserved.

7.4 Manual overrides (admin UI)

Admin can manually set series.match_key to override automatic matching. Useful for series with inconsistent metadata.
Manual matches are sticky: never overwritten by scan.


8. Progress Reporting
8.1 WebSocket events
Emitted to subscribed clients during a scan:
json{ "type": "scan.started", "library_id": "...", "scan_id": "..." }
{ "type": "scan.progress", "scan_id": "...", "phase": "enumerating", "completed": 40, "total": 200 }
{ "type": "scan.series_updated", "series_id": "...", "name": "..." }
{ "type": "scan.health_issue", "kind": "FileAtRoot", "path": "..." }
{ "type": "scan.completed", "scan_id": "...", "added": 12, "updated": 3, "removed": 1, "duration_ms": 42000 }
{ "type": "scan.failed", "scan_id": "...", "error": "..." }
8.2 Stored scan history

Each scan creates a scan_runs row: id, library_id, started_at, completed_at, status, added_count, updated_count, removed_count, error_message.
Last 50 scans per library retained; older ones pruned.
Surfaced in the admin UI as a sortable history with "details" expanding to the health issues found in that run.


9. Performance Requirements
(Aligned with main spec §18.)

Throughput: > 500 issues/min on local NVMe, > 100 issues/min on typical NAS over GbE.
Memory: scan worker pool capped at ~256 MB resident regardless of library size.
I/O: never reads more than the central directory of an unchanged ZIP. Full-file read only when hashing or extracting cover.
DB writes: batched (default batch size 100). One transaction per series, not per file, so a 50-issue series is one DB roundtrip set, not 50.

9.1 Optimization rules

If (path, size, mtime) matches the last scan, skip hash computation.
If hash matches an existing issue, skip ComicInfo parse.
If folder mtime is unchanged, skip the folder entirely (default; can be forced per §4.4).
Cover thumbnail generation deferred to post-scan queue (§4.8) to keep the scan loop tight.


10. Library Health
The scanner produces a structured catalog of issues found. Surfaced in the admin UI under "Library Health."
10.1 Issue types
rustenum LibraryHealthIssue {
    FileAtRoot { path: PathBuf },
    EmptyFolder { path: PathBuf },
    UnreadableFile { path: PathBuf, error: String },
    UnreadableArchive { path: PathBuf, error: String },
    MissingComicInfo { path: PathBuf },                        // info-level, configurable
    MalformedComicInfo { path: PathBuf, error: String },
    FolderNameMismatch { folder: String, comic_info_series: String },
    MixedSeriesInFolder { folder: PathBuf, series_values: Vec<String> },
    AmbiguousVolume { path: PathBuf, parsed: String },
    DuplicateContent { path_a: PathBuf, path_b: PathBuf },     // same hash, different paths
    PageCountMismatch { path: PathBuf, expected: u32, actual: u32 },
    OrphanedSeriesJson { folder: PathBuf },                    // series.json with no archives
}
10.2 Reporting

Stored in library_health_issues table with (library_id, scan_id, kind, payload, severity, first_seen_at, last_seen_at, resolved_at).
Issues persist across scans until they're no longer detected (auto-resolved) or the user manually dismisses them.
Severity: error (something is broken), warning (something looks wrong), info (FYI).


11. Configuration
Per-library configuration (admin UI + DB):
name                      string
root_path                 path
ignore_globs              list<string>
scan_schedule             cron expression (default: "0 */6 * * *")
file_watch_enabled        bool (default: true; auto-disabled if unsupported)
default_language          ISO 639-2 (default: "eng")
default_reading_direction ltr|rtl|ttb (default: "ltr")
dedupe_by_content         bool (default: false)
report_missing_comicinfo  bool (default: false; if true, generates info-level health issues)
Server-wide configuration (env vars, per main spec §12.3):
COMIC_SCAN_ON_STARTUP        bool, default false
COMIC_SCAN_WORKER_COUNT      int, default min(cpu_count, 4)
COMIC_SCAN_BATCH_SIZE        int, default 100
COMIC_SCAN_HASH_BUFFER_KB    int, default 64

12. Error Handling
12.1 Recoverable errors (per-file)

File can't be opened, archive is corrupt, ComicInfo is malformed, etc.
Logged as a LibraryHealthIssue (§10).
Scan continues with the next file. The series is still updated based on the files that did parse.

12.2 Recoverable errors (per-series)

Series folder disappeared mid-scan, permission denied on a sub-folder, etc.
Logged. The series is left in its prior state. Other series proceed.

12.3 Fatal errors (abort scan)

Library root inaccessible.
Database connection lost.
Out of disk space on /data (thumbnails can't be written).

In all fatal cases, the scan run is marked failed, an event is emitted, and admin is notified via the next sign-in (or via webhook if configured — backlog item).

13. Backlog
Things deferred from this scanner spec:

"Loose" library type for libraries without consistent ComicInfo. Requires more aggressive filename heuristics.
Folder-of-images as Issue support. Listed in §5.3 but full handling (cover detection, page sort, deletion semantics) deferred.
EPUB support as a comic. Listed in §5.3 extensions but treated as a stub until someone needs it.
Webhook notifications on scan completion / health issues.
External metadata enrichment during scan (Comic Vine, Metron API). Currently only ComicInfo / series.json / MetronInfo are read.
Smart folder restructuring suggestions ("you have 14 series with files at root — would you like me to propose a folder layout?").
Cover refresh as a separate job (Kavita-style "Refresh Covers" task). Currently covers are regenerated on first scan only and on hash change. A force-refresh button is sufficient for v1.
Word count / file analysis (Kavita-style "Analyze Files"). Not relevant for comics in v1.


14. Open Questions

Soft-delete window — 30 days before auto-confirming deletion. Is this right, or should it be configurable per-library?
Hash algorithm migration — BLAKE3 is the right choice now. If a future Postgres extension or hardware acceleration changes the calculus, how do we migrate existing hashes? (Probably: store algorithm version alongside hash.)
Scan progress granularity — emitting one WebSocket event per series might flood clients on a 5000-series library. Should we throttle to N events/sec?
Multi-tenant scan isolation — when multiple users share a server, should one user's "force rescan" trigger reflow for everyone? (Probably yes — the library is shared — but worth confirming.)