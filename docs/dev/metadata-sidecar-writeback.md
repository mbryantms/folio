# Metadata sidecar writeback

`metadata-sidecar-writeback-1.0` inverts the metadata-providers pipeline:
instead of writing provider data straight into the DB, the apply path
**writes XML into the archive** (ComicInfo.xml + MetronInfo.xml), then
enqueues a scoped rescan. The scanner ingests the freshly-written XML
and the DB cache catches up via the same path used for every other scan.
The result is a system where the archive XML is the canonical source of
truth — downstream consumers (OPDS readers, ComicTagger, Komga, Mylar3,
KOReader Sync) see the same data Folio sees.

This document is the architecture reference for the writeback subsystem.
For the upstream provider pipeline see
[`metadata-providers.md`](metadata-providers.md). For operator-side
tunables (per-library toggles, drift dashboard, flush button) see
[`metadata-operator-guide.md`](metadata-operator-guide.md).

## The architectural inversion

Before writeback (metadata-providers-1.0):

```
provider → orchestrator → apply → writers::set_* → DB (canonical)
                                                     │
                                                     └─→ XML never updated
```

After writeback (this plan):

```
provider → orchestrator → apply → composer → ComicInfo.xml + MetronInfo.xml
                                              │
                                              └─→ rewrite job → archive
                                                                  │
                                                                  └─→ scoped rescan → scanner ingest → DB cache
```

The DB still holds the read-cache (CSV columns + junction tables), but
it's downstream of the XML — same shape as a manually-tagged file the
user dropped into the library. Provider apply, manual edits in the
sheet, scanner-derived defaults all flow through one path.

## Per-library opt-in

> **Invariant.** With `allow_archive_writeback` off, Folio never writes a
> file under the library root — see
> [archive-writes.md](archive-writes.md) for every writer, its gate, and
> the test that proves it.

Writeback is **per-library** behind two flags on the `libraries` row:

| Flag                          | Purpose                                                                                  |
| ----------------------------- | ---------------------------------------------------------------------------------------- |
| `allow_archive_writeback`     | Master kill-switch. False = Folio is read-only on this library's archives.               |
| `metadata_writeback_enabled`  | Routes provider apply through the composer instead of `writers::set_*`. Requires the master flag. |

Both default to `false` so an existing deployment keeps the legacy
DB-direct behaviour on upgrade. Flipping just the master flag is safe —
manual edits (e.g. archive-rewrite-1.0) become possible, but metadata
apply still writes the DB directly. Flipping both enables full
XML-first apply.

`apply_issue` / `apply_series` in [`metadata/apply.rs`](../../crates/server/src/metadata/apply.rs)
check the per-library flag and dispatch:

```rust
if lib.metadata_writeback_enabled && lib.allow_archive_writeback {
    return apply_issue_via_sidecar(state, &args, &row, source, detail).await;
}
// Legacy DB-direct path follows...
```

Once every library has been migrated and the
`folio_metadata_writeback_libraries_remaining` gauge stays at zero
(M7), the follow-up cleanup PR drops the legacy branch entirely.

## The composer

[`metadata/sidecar_compose.rs`](../../crates/server/src/metadata/sidecar_compose.rs)
builds the `ComicInfo` + `MetronInfo` structs from a `ComposeContext`:

- `provider` — the `GenericMetadata` returned by the provider apply.
- `issue` / `series` — current DB rows (the read-cache).
- `issue_external_ids` / `series_external_ids` — the typed-ID rows
  from the `external_ids` table.
- `issue_user_pins` / `series_user_pins` — the set of field keys whose
  `field_provenance.set_by = 'user'`. The composer **prefers DB values
  over provider values for these fields** unless the caller passes
  `override_user_edits = true` (audited as `metadata_apply_force`).

For each field the composer picks one of three sources:

1. **User-pinned**: read the current DB value, ignore provider.
2. **Provider has it**: use provider value.
3. **Provider blank**: fall back to DB value (preserves existing XML
   content during partial provider applies).

The composer emits both formats every time — ComicInfo for tooling
compatibility, MetronInfo for the richer structured fields (per-credit
roles, `<ID source>` map, structured cast lists). That doubles the
write but the archive rewrite is the cheap step; what matters is that
the file stays in sync for whichever consumer reads it next.

## The rewrite job

`RewriteIssueSidecarsJob` (apalis worker, [`jobs/rewrite_sidecars.rs`](../../crates/server/src/jobs/rewrite_sidecars.rs))
takes pre-serialized XML strings and performs the atomic swap:

1. **Mutex**: Redis `SET NX EX` on `archive:rewrite:<issue_id>` (TTL
   120s) so a concurrent edit can't race the apply. **Busy** → the job
   re-enqueues itself with `attempt + 1` after a 5 s pace, up to 40
   attempts (well past the longest lock TTL), then gives up with an
   `archive.errored` library event — the same policy as the page editor's
   `requeue_busy`. **Redis error** → the handler returns `Err` so apalis
   retries it (`JOB_MAX_ATTEMPTS`) and dead-letters it if Redis stays
   down. Pre-fix both cases returned `Ok` and silently dropped the write
   (audit DI-13). While the rewrite runs, a
   [`mutex::Heartbeat`](../../crates/server/src/archive_rewrite/mutex.rs)
   re-arms the TTL every 30 s (compare-and-`EXPIRE` on the claim token),
   so a slow NAS rewrite can't lose the lock mid-swap; the TTL is only
   the crash safety net.
2. **Format gate + open**: `sidecar_refusal` decides what the archive
   can take. CBZ rewrites in place via `archive::cbz::Cbz`; **CBT**
   rewrites in place via `archive::cbt::Cbt` + `cbt_write::write_entries`
   (every entry re-written under its original name — page ordinals are
   unchanged); **CBR** is converted to a sibling `.cbz` first
   (`scanner::cbr_convert::convert_cbr_to_cbz`, `.cbr` kept as
   `.cbr.bak`, `issue.file_path` repointed, `cbr_convert_confirmed_at`
   stamped) **only when the library has `auto_convert_cbr_on_scan`**;
   otherwise — and for CB7 / unknown extensions — the sidecar path is
   refused with a clear reason. The apply dispatch (`apply_issue`,
   composite) runs the same gate *before* choosing the path, so a refused
   archive takes the DB-direct apply with the reason in
   `ApplyOutcome.sidecar_skip_reasons` instead of queueing a job that
   fails at open (audit DI-10). The series fan-out reports it as a
   per-issue skip reason.
3. **Plan**: `cbz_write::RebuildPlan` with `set_entry("ComicInfo.xml",
   …)` + `set_entry("MetronInfo.xml", …)`. Every other entry defaults
   to `Keep` → bytes are stream-copied, never re-encoded. What is kept
   is one policy for every writer
   ([`archive::rewrite_policy`](../../crates/archive/src/rewrite_policy.rs)):
   junk (dotfiles, `Thumbs.db`, `__MACOSX`) and the Folio-managed
   sidecars (root pair regenerated, stale nested copies dropped) are the
   *only* entries a rewrite drops — `CoMet.xml`, `notes.txt`, an embedded
   `.json`, fonts, anything else foreign is carried through
   byte-for-byte by the sidecar rewrite, the page editor and the CBR
   converter alike (audit DI-11). Nested images are pages to every
   reader and follow the page path.
4. **Validate + atomic swap**: `archive_rewrite::rewrite_atomic` writes
   `<path>.<random>.tmp`, then the closure re-opens it and validates the
   rebuild (every kept source entry preserved + both sidecars present +
   archive re-opens) **before** any swap. On success it rotates the older
   `.bak.N` slots per the library's `archive_backup_retain_count`
   setting, **hard-links** the original into `<path>.bak` (byte copy when
   the filesystem refuses links), and `rename(2)`s the staging file over
   the original — an atomic replace, so **the target is never missing**:
   a crash at any step leaves the old or the new bytes at the path
   (pre-fix the original was renamed away before the staging file was
   renamed in, and a crash in between left only the `.bak` plus a `.tmp`
   that `startup_cleanup` later deleted — audit OP-8). Then `fsync`s the
   parent directory.
   `archive_backup_retain_count = 0` keeps **no** `.bak` — the
   validate-before-swap step is the safety net, so the original is never
   replaced by a corrupt rewrite, and the library doesn't transiently
   double in size from full-size backups. `1..=5` keep that many
   rollback slots. A daily sweep
   ([`jobs/backup_prune.rs`](../../crates/server/src/jobs/backup_prune.rs),
   04:45 UTC) walks every library with `allow_archive_writeback = true`
   and deletes `.bak` / `.bak.N` files whose mtime is older than
   `archive_backup_retain_days` (default 30; `0` = keep forever), then
   records one `archive.removed` library event per swept library.
5. **Invalidate caches**: zip-LRU drops the entry; thumbnail stamps
   (`thumbnails_generated_at = NULL`, `thumbnail_version = 0`) clear
   so the catch-up sweep regenerates them on the next post-scan pass.
6. **Bookkeeping**: `issue.last_rewrite_at = now`,
   `issue.last_rewrite_kind = 'sidecar'` (the UI's "Sidecar metadata
   refreshed N ago" badge, shared with page edits) **and**
   `issue.last_sidecar_rewrite_at = now` — the drift-detection stamp
   that only this path sets (see below).
7. **Audit**: `record_admin_action!("admin.issue.sidecar_writeback", …)`
   captures the actor, run id, suppressed user pins, and the exact XML
   bytes that landed.
8. **Mutex release** (heartbeat dropped first).
9. **Rescan**: scoped per-issue rescan enqueued so the scanner re-ingests
   the freshly-written XML. The series-scope apply path overrides this
   with `skip_rescan = true` and fires a single series-scoped rescan
   after the loop completes — saves N redundant rescans on a big series
   apply.
10. **Deferred metadata-only writes** (`job.post_apply`, applied by
    `apply_post_rewrite_writes`): the per-field `field_provenance` rows,
    the variant-cover rows, and the `last_metadata_sync_at` stamp the
    apply decided on. These are the metadata-only rows the XML schemas
    can't carry, and they land **only after the XML is actually in the
    archive** — a failed rewrite (open error, validation abort) writes
    none of them, so the DB never attributes provider values that never
    reached the file (audit DI-10). `apply_issue_via_sidecar` itself
    still writes no entity rows; it only builds the payload. Drift-flush
    jobs carry no payload.

Steps 2–5 cover the two failure classes that used to be silent: a
refused format never reaches the job, and a failed rewrite is audited
(`archive.errored` library event) without any of step 10.

## Series-scope fan-out

Series-scope apply ([`apply_series_via_sidecar`](../../crates/server/src/metadata/apply.rs))
walks every active issue in the series, composes XML per issue (using
the series-level provider detail merged with each issue's DB row),
claims the per-issue mutex around each iteration, and calls the
`rewrite_one_issue` helper inline. Failures accumulate in
`ApplyOutcome.sidecar_skip_reasons` rather than abort the whole fan-out
— a single locked archive shouldn't strand the rest of the series.

A single series-scope rescan fires at the end so the scanner re-ingests
every freshly-written XML in one pass (the per-issue jobs use
`skip_rescan = true` here).

## User-edit drift (M6)

User PATCH edits (via the Edit sheet) write directly to the DB and
stamp `field_provenance.set_by = 'user'`. They do **not** trigger a
sidecar rewrite — Q3 of the plan locked this: "Only write a sidecar
file from an API pull from Metron or Comicvine and those should be only
when the user chooses to do so." This means user edits sit DB-only
until the next provider apply (which the composer respects via the
user-pin set, so the next XML carries the user value forward).

The gap between "pin landed in DB" and "XML carries the pin" is called
**drift**. M6 surfaces it admin-only:

- `GET /libraries/{slug}/health-issues` synthesizes a virtual row of
  kind `MetadataDriftFromXml` (severity `info`) when the library is in
  writeback mode AND at least one issue has
  `field_provenance.set_at > issue.last_sidecar_rewrite_at` (NULL =
  never sidecar-rewritten). The predicate deliberately ignores
  `last_rewrite_at`: page edits and restores stamp that one too but
  carry the *old* XML through verbatim, so pre-fix a page edit after a
  user pin made the drift row vanish without the XML ever receiving the
  edit (audit DI-14). Payload carries
  the drifted issue + series counts plus a capped list of affected
  series ids. Not persisted — re-computed per request; dismiss/resolve
  don't apply.
- `POST /libraries/{slug}/metadata-drift/flush` enumerates the drifted
  series, composes XML from current DB state (the composer's empty-
  provider branch falls through to DB values, which already carry the
  pins), and enqueues a per-issue rewrite job. Returns
  `{ enqueued_rewrites, skipped }`. 409s when writeback is disabled.
- The synth row is hidden from non-writeback libraries since the
  concept doesn't apply (DB is canonical there).

The legacy "Locally edited fields: …" footer on the issue page was
replaced with a per-row inline release icon inside the Edit sheet — see
the issue-page docs in `metadata-operator-guide.md` for the UX details.

## Migration recipe

To migrate a single library from DB-direct to XML-first apply:

1. Flip `allow_archive_writeback = true` (or use the admin sheet's
   master toggle).
2. Flip `metadata_writeback_enabled = true`.
3. Pick a low-stakes series and run **Fetch metadata** from its detail
   page. Apply the candidate.
4. Open one of the rewritten archives with `unzip -p path/to/issue.cbz
   ComicInfo.xml` and eyeball the result. Confirm the XML carries the
   expected provider fields + any user pins.
5. Watch the `/admin/libraries/{id}/health` page for the
   `MetadataDriftFromXml` row — if it appears unexpectedly, click
   **Flush pins to archives**.
6. Once you're confident, repeat on the rest of the libraries. The
   `folio_metadata_writeback_libraries_remaining` gauge will tick down.

There's no automatic backfill — pre-existing files keep their original
XML until the next apply touches them. That's intentional: writeback is
the "next time you apply, the archive gets updated" behaviour, not a
sweep of every archive in the library.

## Risk matrix

| Risk                                | Mitigation                                                                                                                                            |
| ----------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Drift**: user edits don't reach XML   | M6 health row + `metadata-drift/flush` endpoint. Operator-visible Prometheus gauge.                                                                  |
| **Drift masked by a page edit** (WP-2.6 g, DI-14) | The predicate compares pins against `issue.last_sidecar_rewrite_at`, which only the sidecar job stamps; page edits / restores move `last_rewrite_at` alone. Test: `metadata_drift::page_edit_stamp_does_not_clear_drift`. |
| **Partial fan-out failure**: some issues in a series fail to rewrite | Per-issue mutex + `ApplyOutcome.sidecar_skip_reasons`. The series-scope rescan still fires for the issues that succeeded.                              |
| **Rescan latency**: UI shows stale data between apply and rescan | The MetadataMatchDialog subscribes to `/ws/scan-events` after apply and waits for `scan.completed` (30s timeout). On timeout it closes anyway — next scan picks it up.|
| **Archive corruption from a botched rewrite** | Atomic temp → fsync → **validate-before-swap** (entries preserved + sidecars present + re-opens) → `.bak` rotation → rename → fsync-parent. Validation aborts the swap with the original intact, so `archive_backup_retain_count = 0` (no `.bak`, no size doubling) is safe; `1..=5` add rollback slots. |
| **Crash between the two renames loses the file** (WP-2.6 a, OP-8) | There are no longer two renames: the original is hard-linked (or copied) into `.bak` *while still at the target*, then the staging file is `rename(2)`d over it atomically. `startup_cleanup` can only ever reap a `.tmp` whose bytes are also at the target or superseded. Test: `archive_rewrite::crash_at_every_step_leaves_target_readable`. |
| **Foreign sidecars dropped on rewrite** (WP-2.6 b, DI-11) | One `archive::rewrite_policy` for the sidecar rewrite, page editor and CBR converter: only junk + the Folio pair are dropped; `CoMet.xml` / `.txt` / `.json` / anything else streams through byte-for-byte and the validator requires it to survive. Tests: `cbz_write::rebuild_preserves_foreign_sidecars_and_drops_junk`, `archive_edit::page_edit_preserves_foreign_sidecars_cbz_and_cbt`. |
| **Unmodelled XML elements deleted by the composer** (WP-2.6 c+d, DI-11) | `issues.metron_info_raw` persists the parsed MetronInfo so its `raw` map passes through (`<MangaVolume>`, vendor `<X-…>`); `MainCharacterOrTeam` / `AlternateNumber` / `AlternateCount` carry through from `comic_info_raw`. Tests: `sidecar_compose::compose_metroninfo_passes_unknown_elements_through`, `compose_carries_unmodelled_comicinfo_fields_through`, `processing::scan_persists_metron_info_raw_with_unmodelled_elements`. |
| **Lock-busy job silently dropped** (WP-2.6 e, DI-13) | Busy → bounded requeue with `attempt` counter + 5 s pace (40 attempts), then a loud `archive.errored` event; Redis error → `Err` to apalis for retry / dead-letter. Same policy in the page editor. Test: `metadata_apply_sidecar::sidecar_job_requeues_when_rewrite_lock_is_busy`. |
| **CBR/CBT apply fails at open after the run was marked applied** (WP-2.6 f, DI-10) | Format gate at dispatch (`sidecar_refusal`): CBT rewrites in place, CBR converts first when `auto_convert_cbr_on_scan` allows it, otherwise the apply falls back DB-direct with the reason on the outcome. Provenance / variants / `last_metadata_sync_at` are deferred to the job and written only after a successful rewrite. Tests: `sidecar_rewrite_handles_cbt_in_place`, `sidecar_rewrite_converts_cbr_when_library_allows`, `apply_issue_cbr_without_conversion_falls_back_to_db_direct`, `failed_rewrite_writes_no_provenance_variants_or_sync_stamp`. |
| **Mutex stuck after worker crash**      | TTL on the Redis key (120s sidecar / 180s edit). Worker also releases explicitly on every exit path.                                                 |
| **Slow rewrite outlives the lock TTL** (WP-2.6 h, DI-13) | `mutex::Heartbeat` re-arms the TTL every 30 s (compare-and-`EXPIRE` on the claim token, never resurrects a lost lock) for as long as the blocking rewrite runs, in the sidecar job, the page editor and the series fan-out. A crashed holder stops beating and the TTL reaps the key as before. |
| **User pin clobbered by provider apply** | Composer reads `field_provenance.set_by='user'` rows and prefers DB values. Bypass requires the admin-only `override_user_edits` flag + `metadata_apply_force` audit. |
| **XML round-trip data loss**            | Round-trip tests for both `comicinfo.rs` (17 tests) and `metroninfo.rs` (~12 tests). Quick-xml 0.40 `GeneralRef` event handling fixed in M8 (was silently dropping `&lt;` / `&gt;`). MetronInfo `raw` holds top-level elements only, so a nested leaf can never be hoisted to the root on serialize. |

## Reviewer heuristics

When reviewing PRs that touch the metadata apply path:

- **Adding a new metadata field**: changes must land in (1) the parser
  struct, (2) the serializer, (3) the composer, (4) the scanner ingest
  (`process.rs` + `metadata_rollup.rs`). They must **not** touch the
  apply job — the apply path runs the composer and that's it.
- **New direct `writers::set_*` call inside `apply_*` for entity-row
  writes**: reject. The writeback path is composer + scanner; if the
  scalar needs to land in the DB, add it to `process.rs` so the rescan
  picks it up. The sanctioned **metadata-only** exceptions (rows the
  XML schemas can't carry) are: variant covers
  (`set_issue_variants`), `last_metadata_sync_at` (`bump_issue_sync`),
  and per-field provenance (`write_field_provenance` over
  `SIDECAR_ISSUE_PROVENANCE_FIELDS` — the XML can't say "ComicVine set
  this", so the apply records it; the scanner's own file-tier writes
  are guarded and won't downgrade those rows on the follow-up rescan).
- **`MetadataField::iter()` without an `is_junction()` / `is_cover()`
  guard**: reject. Junctions go through `writers::set_issue_*` (cache
  rebuild side effect); variants go through `set_issue_variants`;
  scalar columns through `apply_issue_updates`.
- **`INSERT INTO field_provenance` from a non-writers caller**: reject.
  Always go through `writers::set_external_id` / the per-field write
  helpers so the precedence rule fires. Scanner-side callers use the
  guarded tier — `writers::write_file_field_provenance` /
  `delete_file_field_provenance` — whose `ON CONFLICT … WHERE set_by
  IN (file codes)` clause makes attribution strength
  (**user > provider > file**) hold atomically even when a
  writeback-triggered rescan races the apply job.

The cleanup PR after M7 will physically remove the legacy DB-direct
branch from `apply_issue` / `apply_series` once the
`folio_metadata_writeback_libraries_remaining` gauge stays at zero.

## File map

| Module                                                            | Role                                                                |
| ----------------------------------------------------------------- | ------------------------------------------------------------------- |
| [`metadata/sidecar_compose.rs`](../../crates/server/src/metadata/sidecar_compose.rs) | Build `ComicInfo` + `MetronInfo` structs from a `ComposeContext`.   |
| [`parsers/comicinfo.rs`](../../crates/parsers/src/comicinfo.rs)         | Parse + serialize ComicInfo.xml. Handles quick-xml 0.40 `GeneralRef` events. |
| [`parsers/metroninfo.rs`](../../crates/parsers/src/metroninfo.rs)       | Parse + serialize MetronInfo.xml.                                  |
| [`archive/cbz_write.rs`](../../crates/archive/src/cbz_write.rs)         | `RebuildPlan` + `rebuild()` for stream-copy-preserving CBZ rewrite. |
| [`server/archive_rewrite/mod.rs`](../../crates/server/src/archive_rewrite/mod.rs)        | Atomic temp + .bak rotation + rename + fsync-parent.                |
| [`server/archive_rewrite/mutex.rs`](../../crates/server/src/archive_rewrite/mutex.rs)    | Per-issue rewrite mutex (Redis SET NX EX).                          |
| [`server/jobs/rewrite_sidecars.rs`](../../crates/server/src/jobs/rewrite_sidecars.rs)  | apalis worker: open → rebuild → atomic swap → cache invalidate → audit → rescan. |
| [`server/metadata/apply.rs`](../../crates/server/src/metadata/apply.rs) — `apply_issue_via_sidecar` + `apply_series_via_sidecar` | XML-first apply dispatch; gated on the per-library flag.            |
| [`server/metadata/drift.rs`](../../crates/server/src/metadata/drift.rs) | M6 drift query: count issues where `pin.set_at > last_rewrite_at`.  |
| [`server/metadata/writeback_progress.rs`](../../crates/server/src/metadata/writeback_progress.rs) | M7 rollout gauge: count libraries with writeback disabled.          |
| [`server/api/health_issues.rs`](../../crates/server/src/api/health_issues.rs) — `flush_metadata_drift` | M6 flush endpoint: composer-only re-emit of DB state to XML.        |
