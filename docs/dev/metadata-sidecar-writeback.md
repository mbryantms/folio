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
   content during partial provider applies). Elements with no DB column,
   or whose file value the scanner deliberately keeps out of the DB
   (`MainCharacterOrTeam`, `AlternateNumber` / `AlternateCount`, a
   year-shaped `<Volume>`), carry through from `issue.comic_info_raw`
   instead, so a rewrite never deletes them (see
   [ComicTagger parity](#comictagger-parity-roadmap-wp-64)).

The composer emits both formats every time — ComicInfo for tooling
compatibility, MetronInfo for the richer structured fields (per-credit
roles, `<ID source>` map, structured cast lists). That doubles the
write but the archive rewrite is the cheap step; what matters is that
the file stays in sync for whichever consumer reads it next.

**MetronInfo credits (WP-8.1).** `<Credits>` follows the MetronInfo XSD
(v1.0 / v1.1 `creditType`, [Metron-Project/metroninfo](https://github.com/Metron-Project/metroninfo/tree/master/schema)):
one `<Credit>` per creator, the name as `<Creator>` text, every role it
holds under `<Roles>`:

```xml
<Credit>
  <Creator>Fiona Staples</Creator>
  <Roles>
    <Role>Cover</Role>
    <Role>Penciller</Role>
  </Roles>
</Credit>
```

Role values come from the schema's `roleValues` enumeration
(`parsers::metroninfo::METRON_ROLES`): Folio's `CoverArtist` is written
as `Cover`, other names match case-/spacing-insensitively, and a role
outside the enumeration (`journalist`, `unknown`) becomes the schema's
`Other`. Before WP-8.1 Folio wrote a non-schema
`<Credit role="…"><Creator><Name>…</Name></Creator></Credit>` per
(role, creator) pair; the parser reads **both** shapes (and maps `Cover`
back to `CoverArtist`), so archives Folio already rewrote still ingest
unchanged, and the next rewrite upgrades them. `sidecar_parity.rs`
asserts the credit grammar structurally (no Rust XSD validator is a
dependency); the golden's `<Credits>` block also validates against both
XSD versions with `xmllint` (the 1.1 `xs:assert`s stripped). The rest of
Folio's MetronInfo document still uses its pre-schema element names
(`<Title>`, `<Year>/<Month>/<Day>`, flat `<Series>` / `<Publisher>`,
`<ID>` without `<IDS>`, `<StoryArcs>`) — tracked separately.

**`<Page DoublePage>` (WP-8.1).** `DoublePage="false"` is omitted (the
ComicInfo default): ComicTagger 1.5.5's page editor ticked "double page"
on the attribute's mere presence. The one exception is a landscape page
(declared `ImageWidth / ImageHeight` ≥ `SPREAD_ASPECT_RATIO`, 1.2): there
an absent attribute would make the next scan infer a spread, so a
declared `false` is written to survive the round-trip. `true` is always
written.

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
   **CB7** likewise (`convert_cb7_to_cbz`, `.cb7` kept as `.cb7.bak`, no
   `cbr_convert_confirmed_at` stamp) **only when the library has
   `auto_convert_cb7_on_scan`** (WP-6.5); otherwise — and for unknown
   extensions — the sidecar path is refused with a clear reason. The apply dispatch (`apply_issue`,
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

### Rescan ingest of provider values

Step 10 runs right after step 9 queues the rescan, so the rescan almost
always sees the apply's **provider** `field_provenance` rows. The
scanner's WP-2.5 tier gate (`process.rs` `protected()`) is what keeps a
non-writeback library's provider values safe from a retag; applied
unchanged to a writeback library it refused to ingest the very XML Folio
had just written — the archive got the new `<Summary>`, `issues.summary`
kept the old text, and every later apply of the same field was frozen the
same way (owner report: Chew #14, v0.33.0). It hit every column and
junction in `SIDECAR_ISSUE_PROVENANCE_FIELDS`.

The gate is now timestamp-aware in writeback libraries (both flags on):

- the rewrite job records the apply's provenance rows **at** the
  rewrite's `last_sidecar_rewrite_at` (`apply_post_rewrite_writes(…,
  rewritten_at)` → `writers::write_field_provenance_at`), i.e. "this
  value is in the archive as of that rewrite";
- a provider-tier row protects its column / junction only when it is
  **newer** than `last_sidecar_rewrite_at` (or the issue was never
  sidecar-rewritten): a value the XML doesn't carry — a DB-direct
  fallback for a refused CBR/CB7, or an apply from before writeback was
  switched on. Otherwise the rescan ingests the file, so the archive
  stays canonical (an external retag of a Folio-written provider field
  is ingested too);
- `user` rows protect unconditionally (user > provider > file); the
  composer already wrote the pinned value into the XML.

Non-writeback libraries are untouched: any provider row protects, as
before. Tests: `crates/server/tests/writeback_provider_ingest.rs`.

The preview pane's per-field opt-in is applied to the payload before
composing (`mask_unselected_issue_fields`): an unticked row is blanked so
the composer keeps the issue's own value for it. An absent or empty
`selected_fields` (legacy clients, one-click apply) composes everything,
as before. The composite path's merged payload already carries only the
kept fields. `mode` (fill-missing vs replace-all) is still not consulted
by the composer — a ticked field is written even when the issue already
has a value.

**Stuck issues from before the fix.** An issue applied on an affected
release has the provider value in its archive and the old one in its
column, with provenance rows a few milliseconds newer than
`last_sidecar_rewrite_at` — so a plain rescan still protects them.
Re-applying the issue heals it. For bulk repair, re-stamp the rows the
rewrite job wrote (provider tier, within a minute after the rewrite) to
the rewrite time, then force-rescan the affected series:

```sql
UPDATE field_provenance fp
SET set_at = i.last_sidecar_rewrite_at
FROM issues i
JOIN libraries l ON l.id = i.library_id
WHERE fp.entity_type = 'issue'
  AND fp.entity_id = i.id
  AND l.allow_archive_writeback AND l.metadata_writeback_enabled
  AND i.last_sidecar_rewrite_at IS NOT NULL
  AND fp.set_by NOT IN ('user', 'comicinfo', 'metroninfo', 'series_json',
                        'scanner_inference', 'scanner_folder_tag')
  AND fp.set_at > i.last_sidecar_rewrite_at
  AND fp.set_at <= i.last_sidecar_rewrite_at + interval '1 minute';
```

Steps 2–5 cover the two failure classes that used to be silent: a
refused format never reaches the job, and a failed rewrite is audited
(`archive.errored` library event) without any of step 10.

## Manual edits (roadmap WP-2.10)

Hand edits take the same path as provider applies. When a library has both
writeback flags on, `PATCH /series/{slug}/issues/{issue_slug}`, the bulk
metadata endpoint, and `PATCH /series/{slug}` (identity fields only: name,
year, volume, publisher, imprint, age rating, issue count, language) call
[`metadata::manual_writeback`](../../crates/server/src/metadata/manual_writeback.rs)
after their row + provenance transaction commits:

- the sidecars are composed from the **database alone** (empty provider
  payload — the composer's DB-wins path, shared with the drift flush);
- one `RewriteIssueSidecarsJob` is queued per issue with `post_apply =
  None` (the handler already wrote the user pins) and the editor as the
  audit actor; the series edit fans out one job per active issue with
  `skip_rescan = true` and then coalesces a single series-scoped rescan,
  exactly like `apply_series_via_sidecar`;
- `sidecar_refusal` (CBR without conversion, unsupported formats) skips the
  file; the database keeps the edit and drift surfacing covers the gap;
- the handler never fails on an enqueue error — the outcome lands on the
  audit row as `sidecar_rewrite` (`enqueued`, `not_writeback`, `refused: …`).

The scoped rescan re-ingests the XML; the user's `field_provenance` pins
(and the WP-2.5 tier gate) keep the values through it. Quick successive
edits queue one job each; the per-issue rewrite mutex serialises them and
the last one carries the final state.

## Series-scope fan-out

Series-scope apply ([`apply_series_via_sidecar`](../../crates/server/src/metadata/apply.rs))
walks every active issue in the series, composes XML per issue,
claims the per-issue mutex around each iteration, and calls the
`rewrite_one_issue` helper inline. Failures accumulate in
`ApplyOutcome.sidecar_skip_reasons` rather than abort the whole fan-out
— a single locked archive shouldn't strand the rest of the series.

A single series-scope rescan fires at the end so the scanner re-ingests
every freshly-written XML in one pass (the per-issue jobs use
`skip_rescan = true` here).

### What a series apply may write into an issue

The series apply fetches **one** provider record — the series (CV
volume, Metron series, GCD series) — and makes no per-issue provider
calls. The composer reads `provider.description` / `title` / dates /
credits / `source_url` as *issue* values, so each issue is composed from
[`sidecar_compose::series_payload_for_issue`](../../crates/server/src/metadata/sidecar_compose.rs),
an allowlist projection of the series record:

| Series-record field | Reaches the issue XML? |
|---|---|
| series name / sort name / type, volume, years, aliases, publisher, imprint, identifiers, source bookkeeping | yes — series identity |
| `format`, `language_code`, `age_rating`, `genres` | **fill-only**: only when the issue has no value of its own (run-wide attributes) |
| `description`, `deck`, `notes`, `title`, number, dates, page count, credits / characters / teams / locations / arcs / tags / concepts / objects / universes, reprints, variants, cover, ratings, price / sku, `source_url` | **never** — issue-level slots keep the issue's own value |

A field added to `GenericMetadata` later is dropped by default until
someone decides it is series-shaped.

The series description goes to **`series.summary`**, in both library
modes, through `apply::write_series_scalar_fields` (the same per-field
provenance + fill/replace + user-pin rules as every series scalar; a
user-pinned series description is never replaced). ComicInfo/MetronInfo
have no series-description slot, and the scanner's only series-summary
write (`reconcile_status`, from `series.json`) fills an empty column only,
so the rescan neither carries nor clobbers it. The DB-direct
`apply_series` path (`write_series_fields`) only ever wrote the series
row and was never affected.

**The bug this closes (v0.33.x).** Before the projection, the composer
got the series record verbatim: `prefer_user_opt_str(pinned,
issue.summary, provider.description)` is `provider.or(db)`, so the
series description replaced every un-pinned issue's `<Summary>`, the
series page URL replaced each `<Web>`, GCD series notes replaced
`<Notes>`, and Metron series genres replaced `<Genre>`. The rescan then
ingested the leaked `<Summary>` into `issues.summary` (file-tier
provenance, or — after #970's timestamp-aware gate — even over an older
provider row). Owner report: Fantastic Four (2001), all 173 issues
carried the CV volume description (raw CV HTML). Tests:
`crates/server/tests/series_apply_issue_descriptions.rs`.

### Repairing issues a series apply overwrote

Deploy the fix first: otherwise the next series apply — including an
auto-apply from the weekly refresh in a library with
`metadata_auto_apply_strong_matches` — re-corrupts them.

**1. Detect** (read-only). An issue whose description, HTML-stripped and
case-folded, equals its series' description — the `series` row or any
cached provider series record matched to it — and that at least one
other issue in the series shares (a one-shot's issue and series blurbs
can legitimately match), with no user pin:

```sql
WITH norm_issue AS (
  SELECT i.id AS issue_id, i.series_id, i.file_path,
         lower(btrim(regexp_replace(regexp_replace(i.summary, '<[^>]*>', ' ', 'g'), '\s+', ' ', 'g'))) AS txt
  FROM issues i
  WHERE i.removed_at IS NULL AND i.state = 'active' AND i.summary IS NOT NULL
),
series_texts AS (
  SELECT s.id AS series_id,
         lower(btrim(regexp_replace(regexp_replace(s.summary, '<[^>]*>', ' ', 'g'), '\s+', ' ', 'g'))) AS txt
  FROM series s WHERE s.summary IS NOT NULL
  UNION
  SELECT x.entity_id::uuid,
         lower(btrim(regexp_replace(regexp_replace(c.payload->>'description', '<[^>]*>', ' ', 'g'), '\s+', ' ', 'g')))
  FROM external_ids x
  JOIN metadata_cache c
    ON c.entity = 'series' AND c.provider = x.source AND c.external_id = x.external_id
  WHERE x.entity_type = 'series' AND c.payload->>'description' IS NOT NULL
),
dup AS (
  SELECT series_id, txt FROM norm_issue WHERE txt <> ''
  GROUP BY 1, 2 HAVING count(*) >= 2
)
SELECT n.issue_id, n.series_id, n.file_path
FROM norm_issue n
JOIN dup d ON d.series_id = n.series_id AND d.txt = n.txt
WHERE EXISTS (SELECT 1 FROM series_texts st
              WHERE st.series_id = n.series_id AND st.txt = n.txt)
  AND NOT EXISTS (
    SELECT 1 FROM field_provenance fp
    WHERE fp.entity_type = 'issue' AND fp.entity_id = n.issue_id
      AND fp.field IN ('description', 'summary') AND fp.set_by = 'user');
```

The `series` row may hold a *later* description than the one that
leaked (a second apply from another provider), and the cache row can
have expired, so also eyeball the looser signal — one text on ≥ 3
issues of a series. Folio's leak is the subset with a series-scope run
(`metadata_run.scope = 'series'`, `items_applied > 0`) and
`issues.last_sidecar_rewrite_at` set just after it; a repeated summary
with neither came from an external tagger, not from this bug. (Dev DB,
2026-10-03: the strict query finds Fantastic Four (2001), 173 issues;
the loose one adds Spawn (2016) 160, Ice Cream Man (2023) 38, Secret
Warriors (2010) 26 and S.H.I.E.L.D. (2011) 6, none of which were ever
sidecar-rewritten or series-applied, plus short runs sharing a
solicitation.)

**2. Restore.** In order of preference:

- **`.bak` restore**, where one survives next to the archive
  (`<file>.bak`, `.bak.N`): copy it back over the archive, then
  force-rescan the series. No provider quota. It also reverts anything
  else that rewrite changed, so only use a backup from that rewrite.
- **Per-issue re-fetch** — the default. Series page → ⋯ → *Fetch
  metadata* → **Only missing or partial**
  (`POST /api/series/{slug}/metadata/batch?scope=incomplete`), which
  selects the step-1 strict set (description = series description,
  shared, not user-pinned) plus incomplete issues; **All issues**
  (`scope=all`) also works. That queues one issue-scope search per
  active issue (cap `REFRESH_BATCH_CAP` = 200 per click) under one
  `metadata_batch`; then the toast's *Review* link
  (`/admin/metadata?tab=review&batch=<id>`) → **Accept all strong**
  and, for needs-review rows, **Fill missing**
  (`POST /api/metadata/batch/{id}/apply`, also capped at 200 with a
  `remainder` to re-trigger). In a writeback library the composer writes
  each picked issue description whatever the mode (see "Rescan ingest of
  provider values"), and with #970 the rescan ingests it. Quota per
  issue and enabled provider: with the series' provider series known
  (coverage accepted / applied), one issue-detail fetch and no search —
  the batch looks the issue up in the cached issue list (see
  `metadata-providers.md` § "Batch direct lookups") and the apply reuses
  that detail; otherwise one issue search (a second broad search when
  the series-narrowed one is empty) plus the detail at apply. ComicVine
  allows 200 requests per resource per hour; the limiter paces a
  173-issue run either way. Issues a provider doesn't describe keep the
  leaked text; clear those by hand.
- **Clearing the leaked text** is not a shortcut in a writeback library:
  the archive is canonical, so blanking `issues.summary` alone is undone
  by the next rescan, and the issue PATCH that would rewrite the archive
  records a **user** pin, which then blocks the provider fill. Only use
  it for issues no provider describes (and accept the pin), or as a
  DB-direct-library step before a fill-missing re-fetch.

Never run either against a production library before testing the recipe
on a copy.

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

## ComicTagger parity (roadmap WP-6.4)

ComicTagger is what most hand-tagged libraries (and Mylar3, Komga,
Kavita) carry ComicInfo.xml from, so "the archive stays portable" means
two things: a ComicTagger-tagged file reads into Folio without loss, and
Folio's rewrite of it reads back into ComicTagger without loss or
spurious diffs. One fixture pair and one test pin both directions.

**Fixture.** [`fixtures/comictagger/ct-1.5.5-tagged.cbz`](../../fixtures/comictagger/)
is a 3 KB CBZ: four real image pages (PNG cover, two JPEGs, an 80×60 PNG
spread) whose `ComicInfo.xml` was written by **ComicTagger 1.5.5** (the
latest stable release on PyPI; 1.6.x is still beta), offline, from `-m`
metadata overrides — no online lookup, no API key, a throwaway
`--config` dir. A second pass applies the page-list editor's edits
(`DoublePage` on the spread, page types, a bookmark) through ComicTagger's
own `comicapi` objects and CIX writer, since the CLI can't reach `<Pages>`
and the GUI needs Qt. It covers every element ComicTagger 1.5.5's CIX
writer emits: Title, Series, Number, Count, Volume, AlternateSeries /
Number / Count, StoryArc, SeriesGroup, Summary, Notes, Year / Month / Day,
the seven credit roles, Publisher, Imprint, Genre, Web, PageCount,
LanguageISO, Format, AgeRating, CommunityRating, BlackAndWhite, Manga,
Characters, Teams, Locations, ScanInformation and the page list (with
ComicTagger's computed ImageSize / ImageWidth / ImageHeight). Values
include XML specials, quotes and non-ASCII text. ComicTagger 1.5.5 does
not write Tags, StoryArcNumber, Translator, MainCharacterOrTeam, Review
or GTIN, so this fixture can't cover them; Folio's own round-trip tests
(`parsers::comicinfo`, `sidecar_compose`) do. Regenerate with:

```sh
uv run --no-project --with comictagger==1.5.5 \
    python fixtures/comictagger/make-fixture.py build
```

**Test.** [`crates/server/tests/sidecar_parity.rs`](../../crates/server/tests/sidecar_parity.rs)
(hermetic — no ComicTagger at test time) copies the fixture into a
writeback library, runs the real scanner, composes both sidecars from the
database (the manual-edit / drift-flush path, `manual_writeback::enqueue_issue_rewrite`),
runs `RewriteIssueSidecarsJob`, and then:

1. diffs Folio's `ComicInfo.xml` against ComicTagger's element by element
   with a plain quick-xml walk (not Folio's parser, so a parser blind spot
   can't hide a loss); every difference must be in the test's
   `KNOWN_DIFFERENCES` allowlist, and an allowlist entry that stops firing
   fails too;
2. compares every `<Page>` attribute (DoublePage as a boolean, see below);
3. checks the page bytes were stream-copied, not re-encoded;
4. checks `MetronInfo.xml` carries the same values for the fields both
   schemas share;
5. pins Folio's two rewritten sidecars as golden files
   (`fixtures/comictagger/folio-rewrite.{ComicInfo,MetronInfo}.xml`;
   re-bless with `FOLIO_PARITY_BLESS=1 cargo test -p server --test sidecar_parity`);
6. rescans and rewrites again and requires identical XML (a fixed point —
   no drift accumulates across scan → rewrite cycles).

**Reverse direction.** `make-fixture.py verify` reads the golden Folio
`ComicInfo.xml` with ComicTagger 1.5.5's own parser and compares it with
ComicTagger's reading of its own file: **0 fields differ** (run it after
every re-bless).

**Fixed by WP-6.4** (each was a lossy round-trip the fixture caught):

| Was | Now |
| --- | --- |
| `<Volume>2021</Volume>` deleted on rewrite. ComicTagger stores a ComicVine series' start year in `<Volume>`; the scanner keeps implausible volumes out of `series.volume` (`plausible_volume`) and the composer only read the series row. | The composer carries the archive's own `<Volume>` through from `comic_info_raw` when neither the series, the provider nor a user pin supplies one. MetronInfo's `<Volume>` (a volume *number*) stays empty for a year-shaped value. |
| `<Notes>` deleted by every DB-only rewrite (a hand edit of any other field, a drift flush). | `compose_notes` keeps `issue.notes` when there is no provider — nothing was re-tagged, so ComicTagger's notes (and the `[Issue ID n]` Mylar3 reads) are still true. A provider apply still replaces them with the Folio audit line (stale-tracer rule, unchanged). |
| `DoublePage="True"` (ComicTagger serializes a Python bool, 1.5.x and 1.6.x) parsed as *undeclared*, so the scanner re-guessed it. | `parsers::comicinfo` reads `DoublePage` case-insensitively (`true`/`yes`/`1`, `false`/`no`/`0`), as ComicTagger 1.6 does. |
| `<CommunityRating>4.5</…>` rewritten as `4.50`. | Shortest round-trip decimal with a trailing `.0` on whole numbers — ComicTagger's (`str(float)`) text. |
| A DB-only compose wrote `MetronInfo.xml` with **no** `<Credits>` while ComicInfo kept them. | `compose_metroninfo` falls back to the issue's per-role columns when the provider has no credits, mirroring `compose_role`. |

**Known intentional differences** (the test's allowlist; keep in sync):

| Element | ComicTagger 1.5.5 | Folio | Why |
| --- | --- | --- | --- |
| `<ComicVineID>` | absent (id only inside `<Web>`) | `123456` | Folio extracts the `4000-N` issue id from a ComicVine `<Web>` URL and writes the de-facto `<ComicVineID>` extension element (Metron-Tagger / Mylar3 spelling). ComicTagger ignores it on read. |
| `<Page DoublePage>` | only on pages marked double, as `"True"` | only on pages marked double, as `"true"` (plus a declared `"false"` on a landscape page, so a rescan doesn't infer a spread) | xs:boolean form. WP-8.1 stopped writing `DoublePage="false"` on ordinary pages: ComicTagger **1.5.5's GUI** page editor ticked "double page" on attribute *presence* (1.6.x checks the value). The test compares `DoublePage` as a boolean with absent = `false`, so it isn't an element difference. |
| Element order, XML declaration quoting, indentation | ComicTagger's tree order | Anansi schema order | Not semantic; the test compares a name → value map. |

What this does **not** cover: the provider-apply path (it replaces
`<Notes>` with the Folio audit line by design, and its values come from
the provider, not the file) and a ComicTagger 1.6.x fixture. ComicTagger
1.5.5 doesn't read MetronInfo; the test checks Folio's MetronInfo
against the ComicInfo values and asserts its `<Credits>` follow the
MetronInfo schema shape (WP-8.1, see above).

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
| **ComicTagger file loses data on a Folio rewrite** (WP-6.4, D9) | Real ComicTagger-tagged fixture → scan → DB-only compose → rewrite, diffed element by element against ComicTagger's XML with an explicit allowlist, golden-pinned, fixed-point checked; reverse check with ComicTagger's own parser. Test: `sidecar_parity::folio_rewrite_of_a_comictagger_file_agrees_on_every_shared_field`. |
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
- **Composing a series-level provider record into an issue**: reject.
  A series-scope apply hands each issue
  `sidecar_compose::series_payload_for_issue(&series_detail, issue)`,
  never the series record itself — the composer reads description /
  title / dates / credits / `source_url` as issue values.
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
| [`fixtures/comictagger/make-fixture.py`](../../fixtures/comictagger/make-fixture.py) | Regenerates the ComicTagger parity fixture (`build`) and runs the reverse check with ComicTagger's parser (`verify`). |
| [`server/api/health_issues.rs`](../../crates/server/src/api/health_issues.rs) — `flush_metadata_drift` | M6 flush endpoint: composer-only re-emit of DB state to XML.        |
