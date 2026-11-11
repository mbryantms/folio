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
| Restore from `.bak` | `POST /issues/{id}/archive/restore` → `preflight` | Same 422 as the editor | `archive_edit_api.rs::restore_rejected_when_writeback_disabled` |
| Provider metadata → ComicInfo/MetronInfo rewrite | `metadata::apply::{apply_issue,apply_series}` dispatch | Takes the XML-first path only when `metadata_writeback_enabled && allow_archive_writeback`; otherwise the DB-direct path (`apply.rs` dispatch). The job re-checks (`jobs::rewrite_sidecars` → `WritebackError::WritebackDisabled`). The settings API refuses to enable `metadata_writeback_enabled` while the master flag is off (`api/libraries.rs`, 422) | `metadata_apply_sidecar.rs::apply_issue_writeback_disabled_takes_legacy_path`, `::apply_series_writeback_disabled_takes_legacy_path` |
| CBR → CBZ conversion at scan | `library::scanner::process::conversion_eligible(lib, Cbr)` | Requires `auto_convert_cbr_on_scan && allow_archive_writeback` on a writable mount (the settings API refuses the former without the latter); otherwise the file is flagged `UnsupportedArchiveFormat` and left untouched | `scanner_cbr_convert.rs::scan_skips_cbr_when_disabled` |
| CB7 → CBZ conversion at scan (WP-6.5) | `library::scanner::process::conversion_eligible(lib, Cb7)` → `scanner::cbr_convert::convert_cb7_to_cbz` | Requires its **own** flag, `auto_convert_cb7_on_scan && allow_archive_writeback`, on a writable mount (settings API refuses the flag without the master toggle, 422). Opting into CBR conversion does not extend to CB7. Otherwise — or when the 7z is encrypted / over a cap / malformed — the file is flagged `UnsupportedArchiveFormat` and left byte-for-byte untouched. On success the original is kept as `<name>.cb7.bak` | `scanner_cb7_convert.rs::scan_skips_cb7_when_only_cbr_conversion_enabled`, `::scan_leaves_encrypted_cb7_untouched`, `archive_edit_settings.rs::enable_cb7_conversion_requires_writeback` |
| CBR / CB7 → CBZ conversion before a sidecar rewrite | `jobs::rewrite_sidecars::rewrite_one_issue` | `sidecar_refusal` refuses `.cbr` / `.cb7` unless the library's matching `auto_convert_*_on_scan` flag is on (the apply then falls back to DB-direct); the job itself only runs for writeback libraries | `metadata_apply_sidecar.rs::apply_issue_cbr_without_conversion_falls_back_to_db_direct`, `::apply_issue_cb7_without_conversion_falls_back_to_db_direct` |
| Manual metadata edit → sidecar rewrite (WP-2.10) | `metadata::manual_writeback::{enqueue_issue_rewrite, enqueue_series_rewrite}` from the issue PATCH, bulk-metadata, series PATCH and external-id add / unlink handlers | Returns `NotWriteback` without composing anything unless `allow_archive_writeback && metadata_writeback_enabled`; `sidecar_refusal` gates the format; the job re-checks the flag | `manual_writeback.rs::issue_patch_in_non_writeback_library_enqueues_nothing`, `::cbr_without_conversion_is_refused_and_the_edit_stays_in_the_database` |
| Metadata drift flush | `POST /libraries/{slug}/metadata-drift/flush` → `api::health_issues::flush_metadata_drift` | 409 `writeback_disabled` unless `metadata_writeback_enabled && allow_archive_writeback`; composes both sidecars from the database for every drifted issue and enqueues one `RewriteIssueSidecarsJob` each; the job re-checks the flag | `metadata_drift.rs::flush_endpoint_409s_when_writeback_disabled`, `::flush_endpoint_enqueues_rewrite_jobs_and_audits` |
| Library-wide sidecar re-emit (audit 2026-11 A7) | `POST /libraries/{slug}/sidecar-reemit` (admin) → `jobs::sidecar_reemit` → `manual_writeback::enqueue_reemit_issue`, one `RewriteIssueSidecarsJob` per active issue | 409 `writeback_disabled` unless `allow_archive_writeback && metadata_writeback_enabled`; each tick re-checks both flags and aborts the run when either is off; `sidecar_refusal` archives are skipped and counted; the rewrite job re-checks the flag | `sidecar_reemit.rs::refused_unless_both_writeback_flags_are_on`, `::turning_writeback_off_aborts_the_run`, `::run_rewrites_every_archive_skips_refused_and_rescans_per_series` |
| `.bak` retention prune | `jobs::backup_prune` (daily) | Deletes only under libraries with `allow_archive_writeback = true` and `archive_backup_retain_days > 0`. Every library is walked read-only for the health page's backup tally (`library_backup_stats`, see [`library::backup_stats`](../../crates/server/src/library/backup_stats.rs)); the walk under a non-writeback library counts and never removes | `backup_prune.rs::libraries_without_archive_writeback_are_skipped`, `::sweep_stores_a_backup_tally_for_every_library` |
| Startup staging-file cleanup | `archive_rewrite::spawn_startup_cleanup` → `startup_cleanup`, started once at boot after the HTTP listener is bound, walking on the blocking pool in the background | Walks only libraries with `allow_archive_writeback = true`. Deletes only names `archive_rewrite::is_staging_name` matches — exactly what the stager (`temp_staging`) produces: a recognized archive file name (`.cbz` / `.cbt` / `.cbr` / `.cb7`), a dot, 32 lowercase hex characters, `.tmp` — and only when older than `STARTUP_CLEANUP_TTL` (10 min, cutoff fixed when the walk starts, so a rewrite that begins during the walk is never touched). Any other `.tmp` (`notes.tmp`, another tool's `x.cbz.tmp`) and every `.bak` are left alone | `archive_edit.rs::startup_cleanup_removes_only_own_staging_in_writeback_libraries`, `archive_rewrite/mod.rs` unit tests (`is_staging_name_matches_only_folio_staging_files`, `startup_cleanup_leaves_other_tmp_files`) |

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
   in `.bak`, and an error would make the sidecar job retry, whose
   rotation would delete `.bak` (with one slot, the only copy of the
   original) and link the rewritten file in its place.

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

There is no window in which the row names a file that is not there: a
conversion removes the original's old name only in
`Converted::finish`, which each caller runs after its row is repointed
(the page editor after its transaction commits, the sidecar job after
its row update; the scanner has no row to repoint, it ingests the new
`.cbz` and reconciles a row for the old name as missing). A caller that
could not repoint its row calls `Converted::abandon`, which removes the
new file so the original is again the only copy at its name and a retry
starts from the same state. A crash between the rename and the row
update leaves both names; the next attempt then finds the new name taken
and is refused (`DestinationExists`, below) until the operator removes
one of the two. One window remains where the filesystem refuses hard
links: `rename_into_place` then renames (no-replace) instead, and the row
names the old path from that rename until the caller's update.

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
| Page editor | the edit fails with the reason in the audit row; nothing is written |
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
  `renameat(.bak → archive)` in the folder handle; a `.bak` or archive
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
