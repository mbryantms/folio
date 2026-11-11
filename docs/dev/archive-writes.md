# Archive writes: the "files stay clean" invariant

Decision D4 (roadmap, 2026-09-29): **in a library whose
`allow_archive_writeback` flag is off, Folio never modifies a file under
the library root.** The database is the record for such libraries; the
archive is a source the scanner reads, never a target. Every code path
that can write into a library root is listed below with the gate that
enforces the flag and the test that proves it. When you add a writer,
add a row here and a test in the same PR (roadmap WP-2.4).

The companion rule for non-writeback libraries — on rescan, a file-tier
value never replaces a provider- or user-set database value — lives in
the scanner and is documented in
[metadata-sidecar-writeback.md](metadata-sidecar-writeback.md) (WP-2.5).

## Writers and their gates

| Writer | Entry point | Gate | Test |
|---|---|---|---|
| Page editor (single issue) | `POST /issues/{id}/archive/edit` → `api::archive_edit::preflight` | 422 `validation.archive_writeback_disabled` when the library flag is off; the job re-checks (`jobs::archive_edit::edit_one_issue` → `EditError::WritebackDisabled`) | `archive_edit_api.rs::edit_rejected_when_writeback_disabled` |
| Page editor (bulk selection) | `POST /archive/bulk-edit` | Each issue whose library has the flag off is skipped with reason "archive writeback disabled for this library"; nothing is enqueued for it | `archive_edit_api.rs::bulk_edit_fans_out_and_reports_skips` |
| Restore from `.bak` | `POST /issues/{id}/archive/restore` → `preflight` → `archive_rewrite::restore_latest_backup` (a swap: the replaced archive becomes the newest `.bak`, see [Restore](#restore-keeps-the-file-it-replaces)) | Same 422 as the editor | `archive_edit_api.rs::restore_rejected_when_writeback_disabled`, `archive_restore.rs::restore_keeps_the_replaced_archive_as_the_newest_backup` |
| Provider metadata → ComicInfo/MetronInfo rewrite | `metadata::apply::{apply_issue,apply_series}` dispatch | Takes the XML-first path only when `metadata_writeback_enabled && allow_archive_writeback`; otherwise the DB-direct path (`apply.rs` dispatch). The job re-checks (`jobs::rewrite_sidecars` → `WritebackError::WritebackDisabled`). The settings API refuses to enable `metadata_writeback_enabled` while the master flag is off (`api/libraries.rs`, 422) | `metadata_apply_sidecar.rs::apply_issue_writeback_disabled_takes_legacy_path`, `::apply_series_writeback_disabled_takes_legacy_path` |
| CBR → CBZ conversion at scan | `library::scanner::process::conversion_eligible(lib, Cbr)` | Requires `auto_convert_cbr_on_scan && allow_archive_writeback` on a writable mount (the settings API refuses the former without the latter); otherwise the file is flagged `UnsupportedArchiveFormat` and left untouched | `scanner_cbr_convert.rs::scan_skips_cbr_when_disabled` |
| CB7 → CBZ conversion at scan (WP-6.5) | `library::scanner::process::conversion_eligible(lib, Cb7)` → `scanner::cbr_convert::convert_cb7_to_cbz` | Requires its **own** flag, `auto_convert_cb7_on_scan && allow_archive_writeback`, on a writable mount (settings API refuses the flag without the master toggle, 422). Opting into CBR conversion does not extend to CB7. Otherwise — or when the 7z is encrypted / over a cap / malformed — the file is flagged `UnsupportedArchiveFormat` and left byte-for-byte untouched. On success the original is kept as `<name>.cb7.bak` | `scanner_cb7_convert.rs::scan_skips_cb7_when_only_cbr_conversion_enabled`, `::scan_leaves_encrypted_cb7_untouched`, `archive_edit_settings.rs::enable_cb7_conversion_requires_writeback` |
| CBR / CB7 → CBZ conversion before a sidecar rewrite | `jobs::rewrite_sidecars::rewrite_one_issue` | `sidecar_refusal` refuses `.cbr` / `.cb7` unless the library's matching `auto_convert_*_on_scan` flag is on (the apply then falls back to DB-direct); the job itself only runs for writeback libraries | `metadata_apply_sidecar.rs::apply_issue_cbr_without_conversion_falls_back_to_db_direct`, `::apply_issue_cb7_without_conversion_falls_back_to_db_direct` |
| Manual metadata edit → sidecar rewrite (WP-2.10) | `metadata::manual_writeback::{enqueue_issue_rewrite, enqueue_series_rewrite}` from the issue PATCH, bulk-metadata, series PATCH and external-id add / unlink handlers | Returns `NotWriteback` without composing anything unless `allow_archive_writeback && metadata_writeback_enabled`; `sidecar_refusal` gates the format; the job re-checks the flag | `manual_writeback.rs::issue_patch_in_non_writeback_library_enqueues_nothing`, `::cbr_without_conversion_is_refused_and_the_edit_stays_in_the_database` |
| Metadata drift flush | `POST /libraries/{slug}/metadata-drift/flush` → `api::health_issues::flush_metadata_drift` | 409 `writeback_disabled` unless `metadata_writeback_enabled && allow_archive_writeback`; composes both sidecars from the database for every drifted issue and enqueues one `RewriteIssueSidecarsJob` each; the job re-checks the flag | `metadata_drift.rs::flush_endpoint_409s_when_writeback_disabled`, `::flush_endpoint_enqueues_rewrite_jobs_and_audits` |
| Library-wide sidecar re-emit (audit 2026-11 A7) | `POST /libraries/{slug}/sidecar-reemit` (admin) → `jobs::sidecar_reemit` → `manual_writeback::enqueue_reemit_issue`, one `RewriteIssueSidecarsJob` per active issue | 409 `writeback_disabled` unless `allow_archive_writeback && metadata_writeback_enabled`; each tick re-checks both flags and aborts the run when either is off; `sidecar_refusal` archives are skipped and counted; the rewrite job re-checks the flag | `sidecar_reemit.rs::refused_unless_both_writeback_flags_are_on`, `::turning_writeback_off_aborts_the_run`, `::run_rewrites_every_archive_skips_refused_and_rescans_per_series` |
| `.bak` retention prune | `jobs::backup_prune` (daily) | Deletes only under libraries with `allow_archive_writeback = true` and `archive_backup_retain_days > 0`, and only Folio's backups made more than that many days ago (see [Backups](#backups-what-counts-and-how-old-they-are)). Every library is walked read-only for the health page's backup tally (`library_backup_stats`, see [`library::backup_stats`](../../crates/server/src/library/backup_stats.rs)); the walk under a non-writeback library counts and never removes | `backup_prune.rs::libraries_without_archive_writeback_are_skipped`, `::sweep_stores_a_backup_tally_for_every_library`, `::todays_sweep_keeps_a_fresh_backup_of_an_old_archive_and_foreign_bak_files`; `jobs::backup_prune` unit tests |
| Startup staging-file cleanup | `archive_rewrite::spawn_startup_cleanup` → `startup_cleanup`, started once at boot after the HTTP listener is bound, walking on the blocking pool in the background | Walks only libraries with `allow_archive_writeback = true`. Deletes only names `archive_rewrite::is_staging_name` matches — exactly what the stager (`temp_staging`) produces: a recognized archive file name (`.cbz` / `.cbt` / `.cbr` / `.cb7`), a dot, 32 lowercase hex characters, `.tmp` — and only when older than `STARTUP_CLEANUP_TTL` (10 min, cutoff fixed when the walk starts, so a rewrite that begins during the walk is never touched). Any other `.tmp` (`notes.tmp`, another tool's `x.cbz.tmp`) and every `.bak` are left alone | `archive_edit.rs::startup_cleanup_removes_only_own_staging_in_writeback_libraries`, `archive_rewrite/mod.rs` unit tests (`is_staging_name_matches_only_folio_staging_files`, `startup_cleanup_leaves_other_tmp_files`) |

## Backups: what counts, and how old they are

A rewrite keeps the original as `<archive>.bak` (older ones rotated to
`.bak.1` … `.bak.4`; see [How every writer touches the library](#how-every-writer-touches-the-library-through-a-folder-handle)).
Three surfaces read those files back: the daily retention prune
(`jobs::backup_prune`), the health page's backup tally
(`library::backup_stats`) and the series Metadata tab's backup count
(`api::series_metadata_tab::inspect_folder`). All three apply one rule
(ARC-BAK):

- **A backup is a regular file named `<archive>.bak` or
  `<archive>.bak.<n>`**, `n` in `1..=5` without leading zeros, where
  `<archive>` is a recognised comic-archive file name (`.cbz` / `.cbt` /
  `.cbr` / `.cb7`, any case, non-empty stem):
  `archive_rewrite::is_backup_name`. Pre-conversion backups match
  (`X.cbr.bak` / `X.cb7.bak` beside the converted `X.cbz`). A user's
  `notes.bak`, `photo.jpg.bak`, `X.cbz.bak.txt`, `X.cbz.bak.01`, a
  directory or a symbolic link with a backup's name is never deleted,
  counted or listed. The walk follows no link.
- Restore never searches: it names the issue's own `<file>.bak`
  (`archive_rewrite::backup_slot`) and accepts only a regular file there.
  The per-issue backups listing (`GET /issues/{id}/archive/backups`)
  names the issue's own `<file>.bak[.n]` first, then reads the archive's
  folder for the backups of the archive it was converted from
  (`X.cbr.bak[.n]` / `X.cb7.bak[.n]`, extension in any case, beside the
  issue's `X.cbz`: `archive_rewrite::pre_conversion_backup_slot`), marked
  `pre_conversion: true`, sorted by slot and name (never the directory's
  listing order). Restore does not use those yet (see
  [Restore](#restore-keeps-the-file-it-replaces)).
- Not distinguished: a file the user named exactly like a Folio backup
  (`cp X.cbz X.cbz.bak`). Only a record of the backups Folio made would
  tell those apart; the name rule cannot. Requiring the sibling archive
  to exist was considered and not taken: it does not separate that case
  either (the sibling is right there), and it would hide and never prune
  the backups of an archive that was later deleted or renamed.

**A backup's age counts from when Folio made it** (ARC-WRITE a), read as
the file's inode change time (`ctime`, `archive_rewrite::backup_made_at`),
never its mtime. The backup is a hard link to the original where the
filesystem allows one, and a hard link shares the original's inode,
mtime included; by mtime a backup of an archive last modified a year ago
was "a year old" the moment it was made, and the next sweep deleted it
with the default 30-day retention. How each path leaves the backup:

| Path | Backup made by | mtime | ctime |
|---|---|---|---|
| `rewrite_atomic` (page edit, sidecar rewrite, in-place conversion) | `linkat` archive → `.bak` | the original's (old) | the link (now) |
| same, hard links refused (SMB / some NAS / FAT) | copy into a new `.bak` | now | now |
| same, link made but its ctime is not "now" (sshfs) | link removed, then a copy into a new `.bak` | now | now |
| `convert_atomic` to a new name (`X.cbr` → `X.cbz`) | `linkat` / copy to a staging name, renamed over `X.cbr.bak` | as above | the link or copy, then the rename (now) |
| `rename_into_place` (a ZIP named `.cbr`) | no backup | | |
| slot rotation (`.bak` → `.bak.1`) | rename | unchanged | the rename (later) |

`link(2)` sets the inode's ctime (POSIX; ext4, btrfs, XFS, ZFS, tmpfs and
NFS servers do), a copy is a new file, and nothing in user space can set
a ctime back, so the ctime is never earlier than the backup. Anything
later (the rewrite unlinking the archive's old name, a rotation, a
`chmod` / `chown`, a torrent client dropping its own hard link to the
same inode) only moves it forward: a backup may be kept longer than the
retention, never deleted sooner. The tally's oldest / newest dates use the
same clock. Stamping the backup's mtime instead was rejected: for a hard
link it would also change the mtime of every other name of that inode
(a download client's hard link, or the archive itself before the swap,
which the scanner fingerprints by size + mtime).

**A filesystem that does not set ctime on a link is caught when the
backup is made.** sshfs is the known case: SFTP v3 has no ctime, so the
client reports the mtime as ctime, while hard links work through an
extension; a link there reads exactly as old as the original archive
(ctime == mtime, years ago). That cannot be told apart later: ctime ==
mtime is also what every file on FAT / exFAT reports, so equal times are
trusted. Instead `archive_rewrite::link_or_copy` looks at the new link
once, through the folder handle: when it is still the original's file
and its age (`backup_made_at`) is earlier than the staging file's mtime
by more than `LINK_AGE_SLACK` (2 s), it unlinks the backup name alone
(the archive keeps its own name; it is the same file under two names)
and makes a copy, whose times are now. The reference is the staging
file's mtime, written moments before by the same filesystem, so a
server clock that runs behind ours does not matter; ours (taken just
before the link) stands in only when the filesystem reports no mtime.
The slack covers timestamp granularity (whole seconds on sshfs and some
NAS filesystems, two on FAT). A failure part-way leaves either the link
(a valid backup; the error stops the rewrite before the swap) or no
backup, and the archive at its name in every case. The decision is
unit-tested with injected times (`a_link_must_read_as_made_now_against_the_reference`)
and the fallback by a reference the link cannot meet
(`a_link_that_does_not_age_from_now_is_replaced_by_a_copy`;
`backup_prune.rs::rewrite_checks_the_link_against_the_staging_files_time`
proves the rewrite passes the staging file's time); a filesystem that
really behaves like sshfs is not available to the tests. On a filesystem
whose links fail this check every backup is a full copy; the first time
that happens on a filesystem (device) the process logs one WARN saying
backups there are made by copy and why, and each file stays at debug.

**The backups listing reports the same date.** Each `BackupView` carries
`made_at` (RFC 3339, `backup_made_at`, null when the age is unknown) next
to `modified_at`, which keeps its old meaning (the file's mtime: for a
hard-linked backup, the original archive's old date). Show `made_at` as
"backed up on". Test: `archive_restore.rs::listing_reports_when_each_backup_was_made`
(a hard-linked backup of a 400-day-old archive, and a copy that carries
an old mtime).

**Unknown age is kept, counted and reported.** A backup whose ctime is
missing (zero) or earlier than its mtime has an unknown age and is
**kept**. A filesystem that keeps ctime produces "earlier than the mtime"
only for a file whose mtime was set into the future, for example an
archive tagged on a machine whose clock ran ahead: a hard link to it
shares that future mtime. New backups of such an archive are copies
(the link check above refuses a link of unknown age), so this concerns
backups made before that check, or touched later. Such a backup is
never pruned until its times change. The sweep counts them
(`PruneStats::kept_unknown_age`, only in a library it prunes) and logs
one WARN per library per sweep with the count (no paths). On the health
page's backup card they count toward the files and bytes but have no
date: a library whose backups all have unknown age shows its count and
size with both dates as "—".

**Clock skew at prune time.** The sweep compares a backup's change time,
stamped by the storage server's clock, with Folio's own clock; a server
clock that runs behind Folio's by more than the retention can still
prune early (a few minutes of skew against a retention in days does not
matter).

**The age is checked again just before the delete.** The walk decides
from its own look at the name, and a rewrite can rotate a fresh backup
into that name (or a user can copy a file over it) before the delete.
`backup_prune::remove_if_still_expired` looks at the name again with a
no-follow `stat` (`LibraryDir::stat`, `fstatat` with
`AT_SYMLINK_NOFOLLOW`) through the folder handle the no-follow walk from
the root opens, just before the unlink in that same handle, and deletes
only when it is still a regular file with the same device and inode and
its age still qualifies; otherwise the file is kept and counted in
`PruneStats::skipped_changed` (a vanished file is a skip too). The file
is never opened: a backup Folio may not read is still pruned, and no
file data is touched (an open can recall the file on tiered storage). The window
left is between that look and the unlink itself, which no portable call
closes. Test: `jobs::backup_prune::a_file_that_changes_between_the_walk_and_the_delete_is_kept`.

## How every writer touches the library: through a folder handle

No writer creates, renames, links or deletes a file under a library root
by a name the kernel resolves again. Each opens the archive's folder once
through the no-follow walk from the library root
([`library::no_follow`](../../crates/server/src/library/no_follow.rs):
one `openat(…, O_NOFOLLOW | O_DIRECTORY)` per folder, the root itself may
be a link) and does every later step relative to that folder handle
(`LibraryDir`: `openat`, `renameat`, `linkat` without
`AT_SYMLINK_FOLLOW`, `unlinkat`, `fsync` of the handle). A folder swapped
for a link after the walk cannot redirect a step, and no step follows a
link in the final name. The write sequence
([`archive_rewrite::rewrite_atomic`](../../crates/server/src/archive_rewrite/mod.rs)),
all relative to the folder handle:

1. Open the archive by name in the folder: no link followed, a regular
   file only. A link or anything else is refused (`RewriteError::Refused`)
   before anything is written; a missing archive is the additions path.
2. Create the staging file `<name>.<32 hex>.tmp` with `O_CREAT | O_EXCL |
   O_NOFOLLOW` (an unpredictable name; anything already there, a link
   included, fails the rewrite). The staging file gets the mode any new
   file gets (0o666 less the umask), as before handles were used.
3. The writer reads the original through the handle from step 1 and
   writes into the staging handle; validation re-opens the staging handle
   (never the name).
4. fsync the staging file (its handle) and the folder.
5. Confirm the archive name still holds the file read in step 1 (device
   and inode, not a link); otherwise stop with nothing replaced. Rotate
   the older `.bak.N` slots, then hard-link the archive into `.bak`
   (`linkat`; when links are refused, copy from the original's handle
   into a `.bak` created exclusively, with `copy_file_range` where the
   kernel offers it, and give it the original's permission bits).
6. Confirm again, then rename the staging file over the archive
   (`renameat`): the target is never missing.
7. fsync the folder. A failure of this one fsync is logged at ERROR and
   **not** returned: the new file is already in place and the original is
   in `.bak`, and an error would make the sidecar job retry. (A retry
   would now find the archive already holding its sidecars and skip the
   swap, see [Retries after the swap](#retries-after-the-swap); the fsync
   rule predates that and still saves the retry.)

**Staging files never outlive a failure.** The staging file (and a
backup copy being filled) is held by a guard that removes it when the
call returns an error or a panic unwinds through it (a `spawn_blocking`
task included), and only while the name still holds the file it created.
Only a crash that kills the process leaves one, for the startup cleanup.

### Crash points

What is on disk, and what the issue row names, if the process dies after
each step. The row is only ever changed by the caller, after the call.

| Writer | After | On disk | Row names |
|---|---|---|---|
| `rewrite_atomic` (page editor CBZ/CBT, sidecar, in-place conversion) | staging written (4) | original at its name, staging file | the archive: original |
| | backup made (5) | original at its name and at `.bak` | original |
| | rename (6) | new file at the name, original at `.bak` | new file |
| `convert_atomic` to a new name (page editor CBR, sidecar CBR/CB7, scan conversion) | staging written | original at its name, staging file | original |
| | backup made | original at its name and at `.bak` | original |
| | rename to the new name | original at its name and `.bak`, new file at the new name | original |
| | caller repointed the row | as above | new file |
| | `Converted::finish` | original at `.bak` only, new file at the new name | new file |
| `rename_into_place` (scan, a ZIP that wore `.cbr`) | hard link made | the same file at both names | original |
| | `finish` | the file at the new name only | new file |

While both files exist, the row names one of them: a conversion
removes the original's old name only in `Converted::finish`, which each
caller runs after its row is repointed (the page editor after its
transaction commits, the sidecar job after its row update; the scanner
has no row to repoint, it ingests the new `.cbz` and reconciles a row
for the old name as missing). When the row update reports an error, the
caller does not guess: the update (or its commit) may have been applied
on the server although the client saw an error, so it reads the row's
`file_path` again on a fresh statement (`settle_unclear_repoint`). If
the row names the new file, `finish`; if it still names the original,
`abandon` (the new file is removed and a retry starts from the same
state); if the re-read fails or the row names neither, both files are
kept and both paths are logged at ERROR. The page editor reports a
`Finish` as a **completed** edit with a `warning` (its row update is the
one transaction that also moves the anchors, so a row naming the new file
means all of it committed); reporting it failed invited the admin to
repeat a remove or reorder on the already-edited archive (test:
`archive_edit.rs::a_cbr_edit_whose_commit_landed_despite_an_error_is_reported_done`,
control `::a_cbr_edit_whose_commit_failed_is_undone_and_reported_failed`).
A `Converted` dropped without
either logs both paths at WARN and deletes nothing. The page editor
hashes the new archive's pages from the staged file before the rename,
so the time between the rename and the row update is short. It does so
only when the issue had anchors (markers, reading progress) when the edit
started; the bookkeeping transaction looks again before it moves them and,
when an anchor arrived while the rewrite ran, hashes the rewritten archive
then, so that anchor is re-stamped like the others (test:
`markers_archive_edit.rs::an_anchor_added_during_the_rewrite_follows_its_page_with_its_hash`).
The window left is between that look and the transaction's own reads of
the marker and progress rows (consecutive statements); an anchor
committed after those reads is not moved at all, as before.

One window remains in which the row names a file that is not there:
where the filesystem refuses hard links, `rename_into_place` renames
(no-replace) instead, and the row names the old path from that rename
until the caller's update.

**Recovering from an unfinished conversion.** A crash between the rename
and the row update leaves both files and the row on the original. The
next scan keeps that row (nothing destructive) and ingests the new file
as a *second* issue row; every retry of the conversion is refused with
`DestinationExists`. To recover, remove the **new** file (the `.cbz` that
Folio wrote during the conversion that did not finish; the original and
its `.bak` hold the original bytes) and retry. Do not remove the
original: its row carries the reading progress, markers and metadata,
and the duplicate row would be the one that survives. The refusal's
message, which reaches the failed run and the audit row, says the same.

### Retries after the swap

A writer that is run again after its swap must not rewrite the same
archive a second time: the second rotation would move the rewritten file
into `.bak` and, with one slot, delete the only copy of the original.
Every writer that rewrites, per path that can run it again:

| Path | Can run again after the swap? | Verdict |
|---|---|---|
| Sidecar job (`rewrite_sidecars::handle`): the issue-row update after the swap fails, `Err` to apalis, retried under `JOB_MAX_ATTEMPTS`; also an operator's Retry of a dead-lettered job (a fresh copy of the same payload) | yes | **Fixed (ARC-2).** Before rewriting, `rewrite_sidecars::archive_already_holds` opens the archive (no-follow, read-only) and compares: its Folio sidecars must be exactly a root `ComicInfo.xml` and a root `MetronInfo.xml` (no nested or other-case duplicate) holding byte-for-byte the XML the job carries. Then the swap is skipped (no staging file, no rotation, no new backup) and the job goes straight to the bookkeeping, the rescan and the deferred writes; the audit row and library event say `already_written: true`. The comparison is on bytes, not on who wrote them, so a different earlier rewrite cannot pass for this one: its XML differs (the composer's "Tagged with Folio on …" note carries the compose time, so even a re-apply of the same provider data differs), and the rewrite runs. Junk entries the rewrite would drop (`Thumbs.db`, dotfiles) do not block the skip: no reader or the scanner sees them. Test: `writeback_job_retry.rs::a_retry_after_a_post_swap_failure_keeps_the_original_backup` (1 and 3 slots), control `::a_rewrite_with_different_sidecars_still_rewrites_and_rotates` |
| Sidecar job, CBR / CB7 conversion first | yes | Safe before the fix: the row is repointed at the `.cbz` before the sidecar rewrite, so a retry starts from the `.cbz` (then the row above); a retry after an unsettled conversion is refused (`DestinationExists`). The original sits at `X.cbr.bak`, a name the `.cbz`'s rotation never touches |
| Series-scope apply, inline `rewrite_one_issue` per issue (`metadata::apply::apply_series_via_sidecar`) | not by apalis: `metadata_apply::handle_series` returns `Ok` whatever the apply outcome (only a Redis failure before any rewrite returns `Err`); an operator re-apply composes new XML (new timestamp) and is a new rewrite | not affected; `rewrite_one_issue` carries the same check anyway |
| Page editor (`jobs::archive_edit::handle`) | no: the handler always returns `Ok` (only a Redis failure before the claim returns `Err`), so apalis never re-runs an edit after its swap. The danger was the operator repeating an edit reported as failed | see item 4 / [Refusals](#refusals-and-what-the-user-sees): a CBR edit whose commit landed is reported done; an in-place edit whose bookkeeping failed says the archive was edited and must not be edited again (`EditError::NotRecorded`) |
| Busy-lock requeue (`jobs::busy_requeue`, sidecar job, page editor, metadata apply) | the requeue happens when the per-issue lock is busy, before anything is read or written | not affected |
| Scan conversion (`scanner::cbr_convert`) | a later scan sees the converted `.cbz` (a ZIP, not converted again) or, for an unsettled conversion, a `.cbz` twin (`SkipDuplicate`) | not affected |
| `.bak` prune, startup cleanup | do not rewrite | not affected |

### Restore keeps the file it replaces

`archive_rewrite::restore_latest_backup` swaps the archive and its `.bak`:
the backup takes the archive's name and the file it replaced becomes the
newest backup (`.bak`). Restoring again swaps them back, so a wrong click
is undone the same way. The response's `kept_as` names where the
replaced file went; the audit row carries it too.

- **Slots.** The restored backup leaves slot 0 and the replaced file
  takes it: the number of backups and every older slot stay as they
  were, nothing is rotated or deleted, and no slot is written that was
  not already there, whatever `archive_backup_retain_count` says. The next
  rewrite's rotation applies the slot count as usual.
- **Zero slots.** The swap still happens. The `.bak` being restored
  exists anyway (left from a higher setting, or a conversion's backup,
  which always keeps one), the swap leaves one file in its place, and
  throwing the replaced archive away would make the click unrecoverable,
  which is what restore is for. The retention days prune it later.
- **Age.** The kept file's change time is stamped at the restore (a
  `chmod` to its own mode), so the retention sweep ages it from the
  restore, not from the rewrite that made it.
- **Never missing.** On Linux the swap is one
  `renameat2(RENAME_EXCHANGE)` in the folder handle
  (`LibraryDir::exchange`). Where that is refused (`EINVAL` / `ENOSYS` /
  `EOPNOTSUPP`, some network filesystems) it is three steps: the archive
  gets a second name `<file>.bak.<32 hex>.tmp` (hard link, or a copy where
  links are refused), the backup is renamed over the archive (atomic
  replace), and the second name is renamed to `.bak`. A failure before the
  middle step removes the second name and changes nothing; a failure after
  it leaves the restored file at the name and the replaced one under the
  second name, which the error (and an ERROR log line) names. That name is
  neither a staging name (`is_staging_name`: the base `<file>.bak` is not
  an archive name) nor a backup name, so the startup cleanup never
  removes it and the prune never counts it. Tests:
  `archive_restore.rs::a_restore_that_fails_half_way_leaves_the_archive_in_place`,
  `::three_step_restore_ends_like_the_exchange`.
- **Pre-conversion backups are listed, not restored.** Restore takes only
  the issue's own `<file>.bak`. Restoring `X.cbr.bak` for the issue now at
  `X.cbz` is an open product decision (ARC-2 item 3): restoring the RAR
  under `X.cbr` leaves a file the scanner either converts again into a
  *new* issue row (auto-convert on: the current row, with its progress
  and markers, is reconciled as missing) or does not ingest at all
  (`UnsupportedArchiveFormat`, auto-convert off). Until that is decided
  the backups are listed (`pre_conversion: true`) and can be restored by
  hand.

### Never over another file

A conversion never replaces a file at its destination name. The name is
checked before anything is written (an existing file is
`RewriteError::DestinationExists`, a link `Refused`), and the final rename
is `renameat2(RENAME_NOREPLACE)` (`LibraryDir::rename_noreplace`), so a
file that appears at the name while the new archive is being written is
not replaced either. Where `RENAME_NOREPLACE` is not available (not
Linux, or a filesystem that answers `EINVAL` / `ENOSYS` / `EOPNOTSUPP`,
as some network filesystems do) it falls back to looking at the name
without following it and then renaming, which leaves the moment between
the two.

### Refusals and what the user sees

| Writer | Refusal (link, not a regular file, outside the root, name taken, multi-part RAR) |
|---|---|
| Page editor | the edit fails with the reason in the audit row; nothing is written. (Not a refusal, for contrast: when the bookkeeping after an in-place swap fails, the edit fails with `EditError::NotRecorded`, whose message says the archive *was* edited and must not be edited again; a CBR edit whose row update reported an error but whose row names the new `.cbz` is reported **done** with a `warning`, as a `warning`-severity `archive.updated` event.) |
| Sidecar rewrite | not retried; the triggering run is marked failed (`sidecar rewrite refused: …`), and an error library event says "refused". The older refusals (format gate, writeback off, issue gone) complete without failing the run, as before. |
| Restore from `.bak` | 422 `archive.restore_refused`; a missing `.bak` stays 404 `archive.no_backup` |
| Scan conversion | a `.cbz` twin skips the file quietly; a multi-part RAR is a malformed-archive finding with that reason |

A database error while a job looks up the library is a transient error
(the job is retried); a request answers 500. Neither is ever reported as
"not found" or "library gone".

Per writer:

- **Page editor** (single and bulk, `jobs::archive_edit::edit_one_issue`):
  CBZ / CBT through `rewrite_atomic`; CBR through `convert_atomic`
  to the sibling `.cbz`, settled with `finish` after the row commit
  (or undone with `abandon` when the commit failed). A link at the
  archive or on its folder path fails the edit
  (`EditError::Rewrite(Refused)`), and so does a `.cbz` that already
  exists (`DestinationExists`).
- **Restore from `.bak`** (`archive_rewrite::restore_latest_backup`):
  a swap of the archive and `.bak` in the folder handle
  (`renameat2(RENAME_EXCHANGE)`, else link + rename + rename, see
  [Restore](#restore-keeps-the-file-it-replaces)); a `.bak` or archive
  that is a link is refused.
- **Sidecar rewrite** (`jobs::rewrite_sidecars`, also behind manual
  edits, drift flush and re-emit): `rewrite_atomic`, with CBR / CB7
  conversion first through `convert_atomic` (row repointed, then
  `finish`).
- **CBR / CB7 → CBZ conversion at scan**
  (`scanner::cbr_convert`): the move-into-place of a renamed ZIP
  (`rename_into_place`) and the repack (`convert_atomic`) work in the
  folder handle; the source is read through the handle `convert_atomic`
  opened.
- **`.bak` retention prune** and **startup staging cleanup**: the walk
  only finds candidates (it never descends into a link); each delete
  goes through `no_follow::remove_library_file`, which re-walks from the
  root and refuses a link at the name or on the folder path.

Tests: `archive_rewrite` unit tests (`rewrite_refuses_*`,
`rewrite_stays_in_the_checked_folder_when_it_is_swapped_mid_way`,
`crash_at_every_step_leaves_target_readable`,
`convert_crash_at_every_step_keeps_the_original_at_its_name`,
`convert_refuses_an_existing_destination_and_changes_neither_file`,
`convert_never_replaces_a_destination_that_appears_mid_way`,
`convert_abandon_leaves_the_original_as_it_was`,
`a_failed_rewrite_leaves_no_staging_file`,
`a_failed_sync_after_the_swap_is_ok_and_keeps_the_backup`,
`copied_backup_keeps_the_original_mode_and_new_files_get_the_default`,
`staging_create_fails_closed_on_existing_file_or_link`,
`restore_refuses_a_backup_or_target_that_is_a_link`,
`convert_refuses_an_original_or_destination_that_is_a_link`,
`rename_into_place_refuses_links_and_keeps_an_existing_destination`,
`startup_cleanup_never_removes_through_a_link`),
`no_follow::rename_noreplace_never_replaces_an_existing_entry`,
`cbr_convert.rs::conversion_refuses_a_linked_source_or_folder`,
`symlinks.rs::page_editor_*` / `::restore_refuses_a_backup_that_is_a_link` /
`::cbr_and_cb7_issues_replaced_by_links_are_refused`,
and `writeback_job_retry.rs::sidecar_rewrite_*_is_a_refusal` /
`::a_failed_lookup_is_retried_not_taken_for_a_missing_library`.

## Paths that read but never write

Verified read-only (no `remove_file`, `create_dir`, `rename`, or `write`
calls against the library root): the scanner's ingest and health checks,
`deep_validate`, page-byte streaming and thumbnails (they write only under
`data_path/thumbs` and `data_path/cache`), the admin filesystem browser
(`api/admin_fs.rs`), library deletion (rows only; files are never removed),
and the reconcile / soft-delete sweeps.

## UI

The issue menu, the page editor, and the bulk "Edit archives" dialog only
render their affordances when the parent library has the flag on
(`IssueSettingsMenu.tsx`, `PageEditor.tsx`); the bulk dialog still relies
on the server's per-issue skip reporting for mixed-library selections. The
library settings form refuses to enable the dependent toggles without the
master flag (`LibrarySettingsForm.tsx`), mirroring the API.
