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
`comic_metadata_writeback_libraries_remaining` gauge stays at zero
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
   120s) so a concurrent edit can't race the apply.
2. **Open**: `archive::cbz::Cbz::open` reads the source.
3. **Plan**: `cbz_write::RebuildPlan` with `set_entry("ComicInfo.xml",
   …)` + `set_entry("MetronInfo.xml", …)`. Page entries default to
   `Keep` → bytes are stream-copied, never re-encoded.
4. **Validate + atomic swap**: `archive_rewrite::rewrite_atomic` writes
   `<path>.cbz.tmp`, then the closure re-opens it and validates the
   rebuild (every source entry preserved + both sidecars present +
   archive re-opens) **before** any swap. On success it rotates `.bak`
   slots per the library's `archive_backup_retain_count` setting and
   renames over the original; `fsync`s the parent directory.
   `archive_backup_retain_count = 0` keeps **no** `.bak` — the
   validate-before-swap step is the safety net, so the original is never
   replaced by a corrupt rewrite, and the library doesn't transiently
   double in size from full-size backups. `1..=5` keep that many
   rollback slots (pruned after `archive_backup_retain_days`).
5. **Invalidate caches**: zip-LRU drops the entry; thumbnail stamps
   (`thumbnails_generated_at = NULL`, `thumbnail_version = 0`) clear
   so the catch-up sweep regenerates them on the next post-scan pass.
6. **Bookkeeping**: `issue.last_rewrite_at = now`,
   `issue.last_rewrite_kind = 'sidecar'`. Surfaces in the Edit sheet's
   "Sidecar metadata refreshed N ago" badge.
7. **Audit**: `record_admin_action!("admin.issue.sidecar_writeback", …)`
   captures the actor, run id, suppressed user pins, and the exact XML
   bytes that landed.
8. **Rescan**: scoped per-issue rescan enqueued so the scanner re-ingests
   the freshly-written XML. The series-scope apply path overrides this
   with `skip_rescan = true` and fires a single series-scoped rescan
   after the loop completes — saves N redundant rescans on a big series
   apply.
9. **Mutex release**.

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
  `field_provenance.set_at > issue.last_rewrite_at`. Payload carries
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
   `comic_metadata_writeback_libraries_remaining` gauge will tick down.

There's no automatic backfill — pre-existing files keep their original
XML until the next apply touches them. That's intentional: writeback is
the "next time you apply, the archive gets updated" behaviour, not a
sweep of every archive in the library.

## Risk matrix

| Risk                                | Mitigation                                                                                                                                            |
| ----------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Drift**: user edits don't reach XML   | M6 health row + `metadata-drift/flush` endpoint. Operator-visible Prometheus gauge.                                                                  |
| **Partial fan-out failure**: some issues in a series fail to rewrite | Per-issue mutex + `ApplyOutcome.sidecar_skip_reasons`. The series-scope rescan still fires for the issues that succeeded.                              |
| **Rescan latency**: UI shows stale data between apply and rescan | The MetadataMatchDialog subscribes to `/ws/scan-events` after apply and waits for `scan.completed` (30s timeout). On timeout it closes anyway — next scan picks it up.|
| **Archive corruption from a botched rewrite** | Atomic temp → fsync → **validate-before-swap** (entries preserved + sidecars present + re-opens) → `.bak` rotation → rename → fsync-parent. Validation aborts the swap with the original intact, so `archive_backup_retain_count = 0` (no `.bak`, no size doubling) is safe; `1..=5` add rollback slots. |
| **Mutex stuck after worker crash**      | TTL on the Redis key (120s). Worker also releases explicitly on every exit path.                                                                      |
| **User pin clobbered by provider apply** | Composer reads `field_provenance.set_by='user'` rows and prefers DB values. Bypass requires the admin-only `override_user_edits` flag + `metadata_apply_force` audit. |
| **XML round-trip data loss**            | Round-trip tests for both `comicinfo.rs` (17 tests) and `metroninfo.rs` (~12 tests). Quick-xml 0.40 `GeneralRef` event handling fixed in M8 (was silently dropping `&lt;` / `&gt;`).|

## Reviewer heuristics

When reviewing PRs that touch the metadata apply path:

- **Adding a new metadata field**: changes must land in (1) the parser
  struct, (2) the serializer, (3) the composer, (4) the scanner ingest
  (`process.rs` + `metadata_rollup.rs`). They must **not** touch the
  apply job — the apply path runs the composer and that's it.
- **New direct `writers::set_*` call inside `apply_*` for entity-row
  writes**: reject. The writeback path is composer + scanner; if the
  scalar needs to land in the DB, add it to `process.rs` so the rescan
  picks it up.
- **`MetadataField::iter()` without an `is_junction()` / `is_cover()`
  guard**: reject. Junctions go through `writers::set_issue_*` (cache
  rebuild side effect); variants go through `set_issue_variants`;
  scalar columns through `apply_issue_updates`.
- **`INSERT INTO field_provenance` from a non-writers caller**: reject.
  Always go through `writers::set_external_id` / the per-field write
  helpers so the precedence rule fires.

The cleanup PR after M7 will physically remove the legacy DB-direct
branch from `apply_issue` / `apply_series` once the
`comic_metadata_writeback_libraries_remaining` gauge stays at zero.

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
