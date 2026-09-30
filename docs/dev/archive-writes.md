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
| Page editor (bulk selection) | `POST /issues/archive/bulk-edit` | Each issue whose library has the flag off is skipped with reason "archive writeback disabled for this library"; nothing is enqueued for it | `archive_edit_api.rs::bulk_edit_fans_out_and_reports_skips` |
| Restore from `.bak` | `POST /issues/{id}/archive/restore` → `preflight` | Same 422 as the editor | `archive_edit_api.rs::restore_rejected_when_writeback_disabled` |
| Provider metadata → ComicInfo/MetronInfo rewrite | `metadata::apply::{apply_issue,apply_series}` dispatch | Takes the XML-first path only when `metadata_writeback_enabled && allow_archive_writeback`; otherwise the DB-direct path (`apply.rs` dispatch). The job re-checks (`jobs::rewrite_sidecars` → `WritebackError::WritebackDisabled`). The settings API refuses to enable `metadata_writeback_enabled` while the master flag is off (`api/libraries.rs`, 422) | `metadata_apply_sidecar.rs::apply_issue_writeback_disabled_takes_legacy_path`, `::apply_series_writeback_disabled_takes_legacy_path` |
| CBR → CBZ conversion at scan | `library::scanner::process::cbr_conversion_eligible` | Requires `auto_convert_cbr_on_scan && allow_archive_writeback` (the settings API refuses the former without the latter); otherwise the file is flagged `UnsupportedArchiveFormat` and left untouched | `scanner_cbr_convert.rs::scan_skips_cbr_when_disabled` |
| Manual metadata edit → sidecar rewrite (WP-2.10) | `metadata::manual_writeback::{enqueue_issue_rewrite, enqueue_series_rewrite}` from the issue PATCH, bulk-metadata, and series PATCH handlers | Returns `NotWriteback` without composing anything unless `allow_archive_writeback && metadata_writeback_enabled`; `sidecar_refusal` gates the format; the job re-checks the flag | `manual_writeback.rs::issue_patch_in_non_writeback_library_enqueues_nothing`, `::cbr_without_conversion_is_refused_and_the_edit_stays_in_the_database` |
| `.bak` retention prune | `jobs::backup_prune` (daily) | Selects only libraries with `allow_archive_writeback = true` | `backup_prune.rs::libraries_without_archive_writeback_are_skipped` |
| Startup `.tmp` cleanup | `archive_rewrite::startup_cleanup` | Deletes only Folio's own `<name>.<rand>.tmp` leftovers older than the TTL — never a library file. No flag check needed because it cannot touch user data | `archive_rewrite/mod.rs` unit tests |

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
