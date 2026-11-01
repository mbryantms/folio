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
- `issue` / `series` — current DB rows (scalar columns; the comma-joined
  list columns on `issue` are only a fallback, see next item).
- `issue_junctions` — the issue's junction-table content (credits per
  role, characters, teams, locations, story arcs, genres, tags) loaded by
  `sidecar_compose::load_issue_junctions`, the same reader the issue
  detail endpoint uses for `IssueDetailView.credits` / `cast` / `genres`
  / `tag_list`. **Every list field in the composed XML comes from the
  junctions**, never from the CSV read-cache columns: a cached
  `"Mike Deodato, Jr."` is one `issue_credits` row and stays one
  `<Creator>` (pre-fix the composer split the column on every comma).
  The columns are read only for a row whose junctions were never derived
  (legacy rows), and then through the scanner's own
  `metadata_rollup::split_csv`, so the two splitters can't disagree.
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
roles, `<IDS>` map, structured cast lists). That doubles the
write but the archive rewrite is the cheap step; what matters is that
the file stays in sync for whichever consumer reads it next.

### MetronInfo schema (v1.2, audit 2026-11 WP A4)

`parsers::metroninfo::serialize` writes the **MetronInfo v1.2** schema
([Metron-Project/metroninfo](https://github.com/Metron-Project/metroninfo/tree/master/schema),
`schema/v1.2/MetronInfo.xsd`, vendored verbatim at
`fixtures/metroninfo/MetronInfo-v1.2.xsd`) — the version Codex 2.5.x and
Metron-Tagger (darkseid) read and write. The root carries
`xsi:noNamespaceSchemaLocation` pointing at that file. Elements follow the
XSD's listing order (`metroninfoType` is an `xs:all`, so order is not
validated; a stable order keeps rewrites diff-clean).

| Folio value | MetronInfo v1.2 element | Notes |
| --- | --- | --- |
| `external_ids` (issue) | `<IDS><ID source="…" primary="true">` | `source` is the schema's `informationSource` name (`Comic Vine`, `Metron`, `Grand Comics Database`, `Marvel`, `League of Comic Geeks`, `MyAnimeList`, `AniList`, `MangaUpdates`; `parsers::metroninfo::metron_id_source`). `primary` marks the provider the apply came from, else the first of Metron / ComicVine / GCD present. |
| `series.publisher` / `imprint` | `<Publisher><Name/><Imprint/></Publisher>` | Written only with a publisher name (the schema requires `<Name>`). |
| series name, `sort_name`, `volume`, `series_type` / `format`, `year` / `year_end`, `status`, `total_issues`, `summary`; `language_code`; the primary source's series id | `<Series id lang><Name/><SortName/><Volume/><Format/><StartYear/><EndYear/><Status/><IssueCount/><Summary/>` | `Format` and `Status` are written only when they map onto `formatType` / `seriesStatusType` (`metron_series_format`: `Ongoing Series` → `Single Issue`, `TPB` → `Trade Paperback`, …; `metron_series_status`: `continuing` → `Ongoing`, `ended` → `Completed`). The serializer honours the schema's asserts: an end year before the start year is dropped, `Ongoing` gives way to a written `EndYear`. `lang` is written when it is a BCP 47 tag. |
| `number_raw` | `<Number>` | |
| `title` | `<Stories><Story>` | One `<Story>`; several read back joined with `; `. |
| `summary`, `notes` | `<Summary>`, `<Notes>` | |
| `year` / `month` / `day` | `<CoverDate>YYYY-MM-DD</CoverDate>` | `xs:date` needs a full date: a missing month / day is written as `01` (Metron and ComicVine date every issue on the 1st). |
| `store_date` | `<StoreDate>` | |
| `page_count` | `<PageCount>` | |
| junctions: genres, tags, arcs (+ `position_in_arc`), characters, teams, universes, locations, reprints | `<Genres>`, `<Tags>`, `<Arcs><Arc><Name/><Number/></Arc>`, `<Characters>`, `<Teams>`, `<Universes><Universe><Name/>`, `<Locations>`, `<Reprints>` | Junctions are the truth (N-M2); `IssueJunctions` now loads `arc_numbers`, `universes` and `reprints` (a reprint linked to a local issue is spelled `Series #N`). |
| `isbn` / `upc` ids (and a pre-schema `gtin`, classified by shape) | `<GTIN><ISBN/><UPC/></GTIN>` | |
| `age_rating` | `<AgeRating>` | `ageRatingType` only (`metron_age_rating`): a Metron value passes through, ComicInfo's vocabulary maps as Metron-Tagger maps it (`Mature 17+` → `Mature`, `MA15+` → `Teen Plus`, `Everyone 10+` → `Everyone`, `X18+` → `Explicit`, …); anything else is omitted (the schema default is `Unknown`). |
| `web_url` (or the provider's `source_url`), `additional_links` | `<URLs><URL primary="true"/><URL/>` | |
| credits | `<Credits>` | Unchanged since WP-8.1 (below). |
| `provider.fetched_at`, else `last_metadata_sync_at` | `<LastModified>` | Omitted for a DB-only compose of a never-synced issue, so the parity fixed point holds. |
| `issue.metron_info_raw` top-level unknowns | passthrough after `<LastModified>` | `<MangaVolume>`, vendor `<X-…>`; a pre-schema typed name (`<Title>`, `<Year>`) in `raw` is never re-emitted. |

**Dropped from MetronInfo** (no slot in the schema; ComicInfo still
carries them where it has one): `<Manga>` (`Yes` / `YesAndRightToLeft` /
`No`); external ids with source `asin` / `doi` or outside the enumeration;
an age rating that maps onto no `ageRatingType` value; a series format or
status outside the enumerations; an imprint without a publisher name; a
cover date with no year.

**Known limitations** (accepted in WP A4): `raw` passthrough covers
top-level leaves only, so schema containers Folio has no slot for in a
file another tool wrote (`<Prices>`, `<CommunityRating>`,
`<Series><AlternativeNames>` / `<VolumeCount>`, `<Universe><Designation>`,
per-element `id` attributes) do not survive a Folio rewrite — unchanged
from before, when the same elements were dropped. Several `<Story>`
elements read back as one title joined with `; ` and are rewritten as one
`<Story>`. `<Series>` is required by the
schema; the composer always has a series name, but `serialize` on a
struct without one writes no `<Series>`.

**Parser tolerance.** `parsers::metroninfo::parse` reads three shapes
onto one struct: the v1.2 shape; the pre-schema Folio shape (`<Title>`,
`<Year>/<Month>/<Day>`, flat `<Series>` / `<Publisher>` / `<Imprint>` /
`<Language>` / `<Manga>` text, a bare `<GTIN>`, top-level
`<ID source="metron">`, `<StoryArcs><StoryArc>`, `<Credit role="…">`);
and the bare top-level `<ID>` other tools wrote. `source` names are
accepted in the schema spelling, the Folio key and the usual aliases;
an unknown source is kept (lower-cased) and the scanner skips it as
before. Every production archive carries the pre-schema shape until it is
rewritten (WP A7 bulk re-emit), so this is what keeps a rescan lossless.

**Scanner merge.** MetronInfo still wins over ComicInfo on overlap
(`merge_metron_into_comicinfo`), with two agreements the schema's coarser
types force: a `<CoverDate>` on the 1st beside a ComicInfo
`<Year>/<Month>` of the same month with no `<Day>` keeps the day
undeclared, and an `<AgeRating>` that ComicInfo's value maps onto keeps
ComicInfo's finer spelling — so a rescan of a Folio-rewritten archive
changes nothing. New slots: `<URLs>` sets `web_url`; `<Series><Format>`
fills an issue with no `<Format>` of its own (except `Single Issue`);
`<GTIN><ISBN/><UPC/>` ride the ids map into `external_ids` as `isbn` /
`upc`. `<Universes>`, `<Reprints>`, `<StoreDate>`, `<PageCount>`,
`<LastModified>` and the `<Series>` scalars are parsed but not ingested
(the apply path writes universes / reprints / store date directly, the
archive's real page count is the truth, series identity is folder-pinned).

**ComicInfo `<GTIN>` and `<StoryArcNumber>` (audit 2026-11 WP A8).**
ComicInfo has one barcode field where MetronInfo has `<UPC>` / `<ISBN>`.
`compose_comicinfo` fills `<GTIN>` from the issue's ids in the order
`gtin` → `upc` → `isbn` (`sidecar_compose::comicinfo_gtin`): a `gtin` row
is the field's own value (a file's ComicInfo or a user edit), so it
round-trips first; otherwise the UPC with its 5-digit add-on names this
issue and cover, while an ISBN names the book edition. Spaces and hyphens
are dropped and the value stays a string (leading zeros kept). Because
one barcode now sits in both sidecars under different names, the scanner
drops the echo before writing ids (`process::dedupe_barcodes`): a
MetronInfo `upc` / `isbn` whose value the issue already holds under
another barcode source is skipped, and ComicInfo's `<GTIN>` is skipped
when the issue holds its value under another source or the same
archive's MetronInfo carries it as `upc` / `isbn` (so a first ingest of a
Folio-written archive records one `upc` row, not `upc` + `gtin`). Values
compare without spaces or hyphens; a barcode new to the issue still
lands under its own source. Rows that already exist twice (a `gtin` row
and the `upc` echo an earlier rescan added) are left alone and no third
appears.

`<StoryArc>` and `<StoryArcNumber>` are parallel comma-separated lists
built together from the winning arc list (the provider's unless
`story_arcs` is pinned, else the `issue_arcs` junction in
`position_in_arc` order): slot N of the numbers is arc N's position
(`arc_number`: the provider's first when its list won, else the
junction's, each falling back to the other; the same lookup feeds
MetronInfo's `<Arc><Number>`), joined with the names' separator (`; `
once a name has a comma). An unnumbered arc leaves its slot empty
(`1, , 3`), trailing empty slots are dropped, and a list with no number
omits the element. For file-owned arcs (no `story_arcs` provenance above
the file tier, `IssueJunctions::arcs_from_file`, the rollup's own
`arcs_file_owned` predicate) an arc with no position takes the number the
file paired with *that arc*, read from the parsed `comic_info_raw`
(`file_arc_slots`): the scanner derives a position only for a single-arc
issue and never updates one, so a file's multi-arc `<StoryArcNumber>`
lives there alone, and the junction lists unpositioned arcs by name, so
zipping the column against it would re-pair the numbers. A user-owned
issue with no position keeps the `story_arc_number` it typed (the PATCH
handler positions a single integer arc only). The flat columns are
otherwise read verbatim only when the issue has no junction rows. On
rescan, provider- and user-owned `issue_arcs` are never touched, so their
positions survive; a multi-arc file's numbers still reach the junction
only through `comic_info_raw` (follow-up: ingest parallel positions in
the rollup and the PATCH handler).

**Validation.** `sidecar_parity.rs` validates Folio's MetronInfo.xml
against the vendored XSD with `xmllint --noout --schema` (libxml2 is XSD
1.0, so the 1.1 `<xs:assert>`s are stripped from a temp copy; the
serializer honours them itself): the ComicTagger-parity golden, a rich
seeded issue exercising every block
(`rich_metroninfo_validates_against_the_v1_2_schema`), and the legacy-shape
round trip (`legacy_shape_archive_rescans_without_loss_after_the_schema_rewrite`,
fixture `fixtures/metroninfo/folio-legacy-shape.MetronInfo.xml`). The
check skips without `xmllint` locally and fails under `CI`, where the Rust
test job installs `libxml2-utils`.

**MetronInfo credits (WP-8.1).** `<Credits>` follows the MetronInfo XSD
`creditType` ([Metron-Project/metroninfo](https://github.com/Metron-Project/metroninfo/tree/master/schema)):
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
asserts the credit grammar structurally and validates the whole document
against the v1.2 XSD with `xmllint` (see "MetronInfo schema" above).

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
   attempts (well past the longest lock TTL) through the shared
   [`jobs::busy_requeue`](../../crates/server/src/jobs/busy_requeue.rs)
   helper, then gives up with an `archive.errored` library event and —
   when the job came from a metadata run — marks that run `failed` (the
   same policy as the page editor and the apply job). **Redis error** →
   the handler returns `Err` so apalis retries it (`JOB_MAX_ATTEMPTS`)
   and dead-letters it if Redis stays down. Pre-fix both cases returned
   `Ok` and silently dropped the write (audit DI-13). While the rewrite
   runs, a
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
    the variant-cover and reprint rows, the provider primary cover
    (`primary_cover`: fetched now, stored via `writers::apply_cover`), and
    the `last_metadata_sync_at` stamp the apply decided on. These are the metadata-only rows the XML schemas
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

One column sits outside the gate on purpose: `issues.review` has no
`MetadataField` slot and no provider writes it, so every scan re-reads it
from the file (`process.rs`, `am.review = Set(info.review.clone())`) in
every library, writeback or not. The archive's `<Review>` is its only
source; a value written into the database by hand is replaced on the
next scan.

The preview pane's per-field opt-in is applied to the payload before
composing (`mask_unselected_issue_fields`): an unticked row is blanked so
the composer keeps the issue's own value for it. An absent or empty
`selected_fields` (legacy clients, one-click apply) composes everything,
as before. The composite path's merged payload already carries only the
kept fields. The mask covers every row the preview pane shows (the
`merge::scope_fields` issue set: title, deck, description, age rating,
page count, SKU, cover date, the seven junctions, primary and variant
covers). Fields with no preview row — number, notes, web URL, language,
format, publisher / imprint / volume, community rating, scan
information, store date, universes, the series identity, and reprints by
design — can't be unticked; `mode` governs them.

**Apply mode (audit 2026-11 N-M4, decision D-5).** `fill_missing` is
honoured on the sidecar path. Before composing,
`sidecar_compose::fill_missing_payload` blanks every provider value for a
field the issue already has, so the composer's "provider, else the
issue's own value" fallback keeps it and takes the provider's value only
where the issue is empty — the DB-direct `should_apply` matrix applied to
the payload. "Has a value" is what the composer would otherwise write
from the DB: a non-blank issue column for scalars (the cover date only
when year, month and day form a valid date, as in the DB-direct apply
and the preview); the series row for the series-shaped fields (series name, sort
name, type, years, volume, publisher, imprint — so the provider's series
name never replaces `<Series>` in this mode); junction rows for credits,
characters, teams, locations, story arcs, genres, tags, universes and
reprints, with the read-cache column standing in only for a row whose
junctions were never derived, and such a column carrying tagger ids
counting as empty. Credits are one field: existing credits are kept
whole, never merged per role. `<Notes>` keeps the archive's own text
(`ComposeContext::fill_missing`) instead of being replaced by the Folio
audit line — unless that text is a tagger trace (`Tagged with …`, or
tagger ids such as `[CVDB n]`; `sidecar_compose::notes_are_a_value`),
which is bookkeeping about the last tagging run and is replaced, so a
stale id never sits beside the re-matched ones. Deck, FOC date, price
and SKU reach neither sidecar in either mode. Because the masked fields carry no provider value,
the provenance loop (which skips provider-empty fields) stamps provider
rows only for the fields actually filled; a kept value keeps its
file-tier (or user) row. User pins win in both modes. The preview pane locks a *Has value* row
in fill-missing mode (`rowIsToggleable`), since neither apply path would
write it; *Replace all* is how to overwrite one. `replace_all`
never calls the mask and is provider-wins as before. The series fan-out
masks each issue's payload the same way, before a range's series
identity is overlaid. Identifiers, variant covers and the primary cover
are not masked (external ids are additive under their own conflict
rule; covers follow the overwrite policy). Tests:
`crates/server/tests/writeback_fill_missing_cover.rs`.

**Primary cover (audit 2026-11 N-M3).** With `apply_cover` and a
candidate cover URL, the issue-scope sidecar apply adds a
`PrimaryCoverWrite` (URL, provider, the provider's issue identifier, the
apply's `cover_overwrite_policy`) to `PostRewriteWrites`, and the
outcome reports `cover_deferred`. After the rewrite has succeeded, step
10 fetches it through the shared provider-cover path
(`writers::fetch_cover_bytes`: https only, SSRF-vetted, magic-sniffed —
every provider's `fetch_cover` is that call) and stores it with
`writers::apply_cover`, the writer the DB-direct path uses: an active
`issue_cover` primary row plus its file under `data_path`, pHash / dHash
/ aHash computed at write time (soft-fail on decode). The cover never
goes into the archive — pages stay byte-identical, exactly as on a
DB-direct library. A failed or refused rewrite fetches and writes
nothing; a retry that lands writes it. The job payload carries the URL,
not the bytes (Redis), so the fetch happens in the job; a fetch failure
or a host behind a bot challenge is logged, not retried, as on the
DB-direct path. The cover follows `apply_cover`, not the preview
selection (the single-candidate preview has no cover row), matching
`write_issue_fields`. A series-scope apply has no issue covers. A primary
an admin chose ("Use as cover" on a variant, a `cover.primary` user pin;
see `metadata-providers.md`, "Chosen primary cover") is kept: the apply
defers no cover over it and the writer refuses one queued before the
pick, unless `override_user_edits`. The pick itself is an `issue_cover`
row only, so it needs no rewrite.

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

### Failure handling: retry or refuse

The handler's return value is apalis's only signal, so it is chosen per
error class (`FailureDisposition` in `rewrite_sidecars.rs`, audit N-M7):

| Error | Handler returns | What the operator sees |
|---|---|---|
| **Refusal** — `WritebackError::UnsupportedFormat` (`sidecar_refusal`: CBR / CB7 without conversion, unknown extension), `WritebackDisabled` (library master toggle off), `IssueGone` | `Ok` — retrying cannot change the answer | one `archive.errored` event (`error`), run untouched (the dispatch gate already reported the refusal on `ApplyOutcome`) |
| **Transient** — I/O, archive write / validation, CBR conversion, DB (`Rewrite`, `Archive`, `Convert`, `Db`) with budget left | `Err` — apalis reschedules the task | one `archive.errored` event per attempt at `warning` severity ("retrying (attempt n of 5)") |
| **Transient on attempt `JOB_MAX_ATTEMPTS`** | `Err` — `with_retry_budget_attempt` turns it into an `AbortError`, the task is dead-lettered | `archive.errored` at `error` ("failed after 5 attempts"); when `triggering_run_id` is set the run is marked `failed` with the error in `error_summary` and `metadata.failed` is emitted, so the batch view and the match dialog stop showing it as applied |
| **Busy lock budget spent** (40 requeues) | `Ok` | `archive.errored` ("rewrite lock busy") + run marked `failed` |
| **Requeue push failed** | `Err` — apalis retries the original task | nothing dropped |

The handler takes the task's `Attempt` for this (registered with the
`attempt:` arm of `JobRuntime::run`'s `register!`), which is why the
integration tests call it as `handle(job, Data(state), Attempt::new_with_value(n))`.
Pre-fix every failure returned `Ok`: a NAS hiccup consumed the apply's
deferred writes with no retry, and the run stayed `completed`. Guards:
`writeback_job_retry.rs`.

The **apply job** (`jobs/metadata_apply.rs`) follows the same rule for its
own per-entity mutex (`metadata:apply:{series|issue}:<id>`): a busy lock
requeues the job through `busy_requeue` with the job's `attempt` counter
(the job names the stored candidate by `run_id` + `ordinal`, and the
detail comes from `metadata_cache`, so a requeue re-spends no provider
quota); once the 40-requeue budget is spent the run is marked `failed`
("apply lock still held"), a `metadata.errored` event is written and
`metadata.failed` is emitted. A Redis error on the claim returns `Err` for
apalis to retry, and an `ApplyError` from the apply itself now also marks
the run `failed` (it stays `Ok` to apalis: the error is audited and a
provider fault must not re-spend quota on a blind retry). Pre-fix the busy
path returned `Ok` with a log line saying "caller will retry" — no caller
did, so a batch child whose series was still held by an earlier apply was
dropped with its run left looking complete.

A run the rewrite job marked `failed` is **recovered** when a later
attempt of the same rewrite lands (the operator's Retry from Background
work, or a requeued copy): the job recognises its own `error_summary`
prefix (`sidecar rewrite …`) and sets the run back to `completed`; a
search failure is never touched. The batch review counts a `failed` run as
not applied even though the apply bumped `items_applied` when it queued
the rewrite.

## Manual edits (roadmap WP-2.10)

Hand edits take the same path as provider applies. When a library has both
writeback flags on, `PATCH /series/{slug}/issues/{issue_slug}`, the bulk
metadata endpoint, `PATCH /series/{slug}` (identity fields — name, year,
volume, publisher, imprint, age rating, issue count, language — and the
legacy `comicvine_id` / `metron_id` fields) and the external-id endpoints
(`POST` / `DELETE …/external-ids[/{source}]` on an issue or a series —
the ids ride in ComicInfo's typed id elements and MetronInfo's `<ID>`;
audit 2026-11 N-M5) call
[`metadata::manual_writeback`](../../crates/server/src/metadata/manual_writeback.rs)
after their row + provenance transaction commits (the series PATCH writes
its row and its `user` pins in one transaction, like the issue PATCH — a
pin that cannot be written rolls the edit back; audit 2026-11 N-M6):

- the sidecars are composed from the **database alone** (empty provider
  payload — the composer's DB-wins path, shared with the drift flush);
- one `RewriteIssueSidecarsJob` is queued per issue with `post_apply =
  None` (the handler already wrote the user pins) and the editor as the
  audit actor; a series edit — identity field or series-level id — fans
  out one job per active issue with `skip_rescan = true` and then
  coalesces a single series-scoped rescan, exactly like
  `apply_series_via_sidecar`;
- `sidecar_refusal` (CBR without conversion, unsupported formats) skips the
  file; the database keeps the edit and drift surfacing covers the gap;
- the handler never fails on an enqueue error — the outcome lands on the
  audit row as `sidecar_rewrite` (`enqueued`, `not_writeback`, `refused: …`).

The scoped rescan re-ingests the XML; the user's `field_provenance` pins
(and the WP-2.5 tier gate) keep the values through it. Quick successive
edits queue one job each; the per-issue rewrite mutex serialises them and
the last one carries the final state.

## Library-wide re-emit (audit 2026-11 A7)

When the composer changes (MetronInfo v1.2, GTIN, arc numbers, junction
fixes), archives written earlier keep the older XML until something
touches them again. **Settings → Rewrite metadata files** on a library
(`POST /libraries/{slug}/sidecar-reemit`, admin, audited as
`admin.library.sidecars.reemit`) re-emits both sidecars for every active
issue from the database so the files catch up. It is refused with `409
writeback_disabled` unless both writeback flags are on, and with `409
reemit_in_progress` while the library already has a live run.

- **Same path as a manual edit.** Each issue is composed by
  `manual_writeback::enqueue_reemit_issue` (empty provider payload,
  `post_apply = None`, the starting admin as audit actor) and written by
  the ordinary `RewriteIssueSidecarsJob`, so `archive::rewrite_policy`,
  the `.bak` rotation and the rewrite mutex all apply. No new archive
  writer.
- **Throttle.** The run is a self-re-enqueueing tick on its own
  `sidecar_reemit` queue (concurrency 1). A tick queues at most 32
  rewrites and the run never has more than 32 queued or running
  (`TICK_BATCH`, `MAX_OUTSTANDING` in
  [`jobs/sidecar_reemit.rs`](../../crates/server/src/jobs/sidecar_reemit.rs)).
  Each rewrite job carries its XML inline, so a 10k-issue library queued at
  once would park tens of MB in Redis and put every interactive apply behind
  it; with the cap an edit waits behind at most 32 rewrites. The walk pages
  `(series_id, id)` with a projected `select_only` row; the composer loads
  one full issue at a time.
- **Rescans.** Rewrites run with `skip_rescan = true`. A per-series counter
  (one per queued rewrite, plus a guard while the walk is inside the
  series) queues **one** series-scoped rescan when it reaches zero, and
  only if an archive of that series was rewritten. Never a library scan.
- **Refusals.** `sidecar_refusal` archives (CBR / CB7 without conversion,
  unknown formats) are skipped and counted, never queued.
- **Progress.** Run state lives in Redis (`sidecar_reemit:run:<id>`, kept a
  week after the run ends): total, queued, rewritten, skipped (format),
  failed. It shows on `/admin/background-work` (“Metadata file rewrites”)
  and on the library's settings card. A restart resumes from the persisted
  cursor. Every report is matched to the queued issue, so an operator Retry
  of a dead-lettered rewrite moves its count instead of adding one. A run
  with no tick for 15 minutes reads as stalled; starting again supersedes
  it.
- **Stopping.** Turning either writeback flag off aborts the run at its next
  tick; the at most 32 rewrites already queued still finish (or refuse
  themselves when archive writeback is off). *Clear metadata queues* on
  `/admin/queue` also drops the run and its queued rewrites.

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
  `metadata_batch`; then apply in **replace-all** mode: the fetch
  dialog's **Replace all** (strong rows with their candidate, needs-review
  rows with the merge, both `replace_all`; `POST
  /api/metadata/batch/{id}/apply`, capped at 200 with a `remainder` to
  re-trigger), or the Review tab's needs-review **Replace all**. Not
  **Fill missing** and not **Accept all strong** (which sends the default
  `fill_missing`): the leaked description is a value the issue has, so
  fill-missing keeps it (audit 2026-11 N-M4; before that fix the writeback
  composer ignored the mode). In replace-all mode the composer writes each
  picked issue description and, with #970, the rescan ingests it. Quota per
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
   schemas share, in the v1.2 schema shape, and validates it against the
   vendored XSD with `xmllint`;
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
against the ComicInfo values and validates the document against the
MetronInfo v1.2 XSD (see "MetronInfo schema" above). The tools
themselves run in the interop check below.

## Interop check (audit 2026-11 WP A5)

The parity test is hermetic: it compares XML against a golden and an XSD,
but no other program ever opens the archive. The Slice A exit criterion of
the 2026-11 audit is that a Folio-written archive round-trips through
ComicTagger and Metron-Tagger with no diff on shared fields, **verified by
CI**. The `sidecar-interop` job in `ci.yml` does that in two steps:

1. **Producer** —
   [`crates/server/tests/sidecar_interop_emit.rs`](../../crates/server/tests/sidecar_interop_emit.rs)
   seeds one rich issue in a writeback library (credits incl. the
   comma-suffixed `Mike Deodato, Jr.`, three characters, teams, locations,
   two numbered arcs written to the junction only, genres, tags, a
   universe, a reprint, ComicVine /
   Metron / GCD issue ids, the series' Metron id, a UPC with a leading
   zero and an ISBN, cover + store date, `Mature 17+`, a primary URL and
   a ComicVine link), writes a three-page CBZ through the real
   `RewriteIssueSidecarsJob`, and — when `FOLIO_INTEROP_OUT` names a
   directory — copies `issue.cbz`, both XML files and `expected.json`
   (what Folio meant, in both sidecars' vocabularies) there. Without the
   variable it still runs as an ordinary test, so the producer can't rot.
2. **Readers** —
   [`scripts/ci/sidecar_interop.py`](../../scripts/ci/sidecar_interop.py)
   (stdlib + the two tools, `python3 -I`) opens the CBZ three ways —
   ComicTagger's `comicapi` on ComicInfo.xml, Metron-Tagger's `darkseid`
   on ComicInfo.xml and on MetronInfo.xml — and diffs each reading
   against `expected.json`: series (name, sort name, years, count, status,
   format, Metron id), number, title, cover / store date, publisher,
   imprint, language, summary, notes, page count, credits by role,
   characters, teams, locations, arcs (with numbers on the MetronInfo
   side), genres, tags, universes, reprints, ids + primary, GTIN, age
   rating, URLs; plus ComicInfo's `<GTIN>` and `<StoryArcNumber>` on the
   raw XML (below). darkseid also validates MetronInfo.xml against its
   bundled MetronInfo v1.1 and v1.2 schemas with an XSD 1.1 validator
   (the `<xs:assert>`s xmllint skips; v1.1 is tried first) and the script
   requires `Metron Info V1.2`. Any difference not covered by a *named*
   normalisation below fails the job with a readable diff; the archive,
   both sidecars, `expected.json` and the report are uploaded as the
   `sidecar-interop` artifact.

Tool versions are pinned in
[`scripts/ci/sidecar-interop-requirements.txt`](../../scripts/ci/sidecar-interop-requirements.txt)
(ComicTagger **1.6.0b9** — the audit's 1.6 target; 1.5.5 is the last
stable and its `comicapi` predates the `tags` API — Metron-Tagger
**5.0.0** and its reader darkseid **9.1.0**); the script refuses other
versions, so only a pin bump can move the comparison. The job runs on the
Rust path filter plus `fixtures/**` and the script / pins. It is **not a
required check yet**: promote it in branch protection once it has been
green on `main` for a week (the job's own comment says the same).

Locally, `just sidecar-interop` runs both steps against throwaway
Postgres + Redis containers (own names and ports, never the dev services)
and leaves the output directory for inspection.

**Tolerated normalisations** (the script's `TOLERATED` table; each is
printed when it fires):

| Name | What the tool does | How the comparison copes |
| --- | --- | --- |
| `cover_artist_role_is_cover` | comicapi and darkseid both report ComicInfo `<CoverArtist>` (and MetronInfo's `<Role>Cover</Role>`) as the role `Cover`. | Compared as Folio's `CoverArtist`. |
| `comicinfo_csv_comma_name` | A name with a comma makes Folio join the ComicInfo field with `; ` (`Fiona Staples; Mike Deodato, Jr.`). comicapi splits CSV fields on `,` only; darkseid splits credit columns on `;` and `,` and resource columns on `,`. The name arrives as fragments either way. | Expected = the tool's own split of Folio's join. **Known gap**, not fixable in the composer: ComicInfo has no lossless spelling for `Name, Jr.` (ComicTagger's own writer has the same problem). MetronInfo's `<Creator>` carries the name whole, and Folio's scanner re-joins the suffix on rescan (`split_csv`). Follow-up: consider writing the suffix without the comma in ComicInfo only. |
| `issue_number_leading_zeros` | darkseid passes `<Number>` through `IssueString`, which drops leading zeros (`003` → `3`). | Compared after the same normalisation (not exercised by the fixture's `1`). |
| `gtin_as_int` | darkseid parses `<GTIN><UPC>` / `<ISBN>` as integers, so a leading zero is lost (`07098530047000111` → `7098530047000111`). | Compared as integers. |
| `ids_numeric_as_int` | darkseid parses an all-digit `<ID>` and `<Series id>` as integers, so a leading zero is lost. | Both sides compared after the same integer round-trip. |

**Not compared** because the reader does not read it: from ComicInfo.xml
neither comicapi 1.6 nor darkseid reads `<Tags>`, `<GTIN>`,
`<Translator>`, `<StoryArcNumber>` or the `<ComicVineID>` / `<MetronID>`
extension elements, and ComicInfo has no universes or reprints; those
fields are checked on the MetronInfo reading. ComicInfo-only readers
(ComicRack, Kavita, Komga) do read `<GTIN>` and `<StoryArcNumber>`, so a
fourth, stdlib view (`read_comicinfo_xml`, `xml/ComicInfo` in the log)
opens the raw ComicInfo.xml and compares both as exact element text: the
fixture has `upc` and `isbn` ids and no `gtin` id, so `<GTIN>` must be the
UPC (`comicinfo_gtin` in `expected.json`), and its two arcs live in the
junction only, so `<StoryArc>Volume One, The Will</StoryArc>` must come
with `<StoryArcNumber>1, 4</StoryArcNumber>` (WP A8). Values
that legitimately differ *per sidecar* travel in `expected.json` under
their own keys rather than as exceptions: the age rating (`Mature 17+` in
ComicInfo, the schema's `Mature` in MetronInfo) and the series format /
status (`Single Issue` / `Completed`, MetronInfo only). darkseid's
ComicInfo schema check is printed but informational: it bundles the
ComicInfo v1 / v2 XSDs only, and Folio writes the v2.1 draft vocabulary
plus the extension elements. Implementation note: a darkseid `Comic`
caches one parsed metadata object for both formats, so the script opens a
fresh `Comic` per format.

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
| **Rescan latency**: UI shows stale data between apply and rescan | The MetadataMatchDialog subscribes to `/ws/scan-events` after apply and waits for *its* completion (post-audit T1.6): `metadata.applied` / `metadata.failed` carrying the apply's `run_id` (DB-direct path), or the scoped rescan's `scan.completed` whose `series_id` / `issue_id` match the 202 body's. A composite apply that already landed synchronously (`enqueued_rewrite: false`) resolves at once. After 30 s without an event the dialog says "Still applying" with a link to Background work and keeps listening; it never claims success on a timeout, and the worklist counts a closed-while-pending item as pending, not applied (D5a).|
| **Archive corruption from a botched rewrite** | Atomic temp → fsync → **validate-before-swap** (entries preserved + sidecars present + re-opens) → `.bak` rotation → rename → fsync-parent. Validation aborts the swap with the original intact, so `archive_backup_retain_count = 0` (no `.bak`, no size doubling) is safe; `1..=5` add rollback slots. |
| **Crash between the two renames loses the file** (WP-2.6 a, OP-8) | There are no longer two renames: the original is hard-linked (or copied) into `.bak` *while still at the target*, then the staging file is `rename(2)`d over it atomically. `startup_cleanup` can only ever reap a `.tmp` whose bytes are also at the target or superseded. Test: `archive_rewrite::crash_at_every_step_leaves_target_readable`. |
| **Foreign sidecars dropped on rewrite** (WP-2.6 b, DI-11) | One `archive::rewrite_policy` for the sidecar rewrite, page editor and CBR converter: only junk + the Folio pair are dropped; `CoMet.xml` / `.txt` / `.json` / anything else streams through byte-for-byte and the validator requires it to survive. Tests: `cbz_write::rebuild_preserves_foreign_sidecars_and_drops_junk`, `archive_edit::page_edit_preserves_foreign_sidecars_cbz_and_cbt`. |
| **Unmodelled XML elements deleted by the composer** (WP-2.6 c+d, DI-11) | `issues.metron_info_raw` persists the parsed MetronInfo so its `raw` map passes through (`<MangaVolume>`, vendor `<X-…>`); `MainCharacterOrTeam` / `AlternateNumber` / `AlternateCount` carry through from `comic_info_raw`. Tests: `sidecar_compose::compose_metroninfo_passes_unknown_elements_through`, `compose_carries_unmodelled_comicinfo_fields_through`, `processing::scan_persists_metron_info_raw_with_unmodelled_elements`. |
| **Lock-busy job silently dropped** (WP-2.6 e, DI-13; apply job: N-M7) | Busy → bounded requeue with `attempt` counter + 5 s pace (40 attempts, `jobs::busy_requeue`), then a loud `archive.errored` / `metadata.errored` event and the triggering run marked `failed`; Redis error → `Err` to apalis for retry / dead-letter. Same policy in the page editor and the metadata apply job. Tests: `metadata_apply_sidecar::sidecar_job_requeues_when_rewrite_lock_is_busy`, `writeback_job_retry::apply_job_requeues_while_the_mutex_is_held_and_completes_after_release`, `writeback_job_retry::apply_job_with_an_exhausted_busy_budget_marks_the_run_failed`. |
| **Failed rewrite never retried, run left "complete"** (N-M7) | Transient failures (I/O, archive write, conversion, DB) return `Err` so apalis retries under `JOB_MAX_ATTEMPTS`; the last attempt marks the run `failed` and writes an `error` event before the task is dead-lettered; refusals (`sidecar_refusal`, writeback off, issue gone) stay `Ok` and never retry. Tests: `writeback_job_retry::transient_rewrite_failure_is_retried_and_succeeds_once_the_fault_clears`, `rewrite_failure_on_the_last_attempt_marks_the_run_failed_and_aborts`, `sidecar_refusal_returns_ok_and_is_not_retried`. |
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
  The set is "what the composer writes from the provider and the scanner
  ingests back", including `Number`, `Notes` and `WebUrl` (audit 2026-11
  N-M8); the preview pane's `selected_fields` only gates the rows the
  pane shows (`masked_when_unselected`, the same predicate the payload
  mask uses) — a field the composer writes regardless gets its row
  regardless.
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
| [`server/tests/sidecar_interop_emit.rs`](../../crates/server/tests/sidecar_interop_emit.rs) | Interop producer: writes a rich Folio-authored CBZ + `expected.json` to `FOLIO_INTEROP_OUT` (CI job `sidecar-interop`, `just sidecar-interop`). |
| [`scripts/ci/sidecar_interop.py`](../../scripts/ci/sidecar_interop.py) | Interop readers: ComicTagger (`comicapi`) + Metron-Tagger (`darkseid`) read that CBZ; diff against `expected.json`; pins in `sidecar-interop-requirements.txt`. |
| [`server/api/health_issues.rs`](../../crates/server/src/api/health_issues.rs) — `flush_metadata_drift` | M6 flush endpoint: composer-only re-emit of DB state to XML.        |
| [`server/jobs/sidecar_reemit.rs`](../../crates/server/src/jobs/sidecar_reemit.rs) | Library-wide re-emit: throttled tick that queues one DB-composed rewrite per issue, one rescan per series. |
