# Schema restructure (metadata-providers-1.0 M0)

The M0 migration ([`m20261228_000001_metadata_providers_schema`](../../crates/migration/src/m20261228_000001_metadata_providers_schema.rs))
landed the schema foundation for the metadata-providers subsystem.
This document explains what changed, why, and how the
denormalized read-cache pattern works.

Companion docs: [`metadata-providers.md`](metadata-providers.md)
(architecture) and [`metadata-operator-guide.md`](metadata-operator-guide.md)
(operations).

## Why a single migration

Eight changes that depend on each other land atomically:

1. New top-level entity tables (`character`, `team`, `story_arc`,
   `location`, `concept`, `object`, `publisher`, `imprint`, `universe`)
   — mirror the shape of [`person`](../../crates/entity/src/person.rs)
   added in `m20261223_000001_person`.
2. FK columns on existing string-keyed junctions
   (`issue_characters.character_id`, `series_teams.team_id`, …)
   alongside the existing `name` column.
3. The generic `external_ids` table — replaces the trio of fixed
   columns (`comicvine_id`, `metron_id`, `gtin`) on `series` +
   `issues` with a (entity_type, entity_id, source) composite key
   that supports unlimited sources.
4. `metadata_run` + `metadata_run_candidate` — per-search history
   the dialog reads from.
5. `issue_cover` + `series_cover` — replace the single
   `cover.webp`-per-issue model with primary/variant rows + per-row
   provenance + per-row phash.
6. `field_provenance` — generalize the `issue.user_edited` JSON list
   (dropped in WP-3.7 — see below) into
   a typed (entity, field, set_by, set_at) table covering scalar
   fields, junctions, and external IDs uniformly.
7. `issue_reprint` — the "this issue reprints …" relation.
8. `series.metadata_sync_paused` boolean — the per-series exclude
   from auto-refresh.

Backfill steps run *before* any drop so existing data is preserved.
Down-migration paths reverse every backfill (with `set_by='migration_v1'`
filter so reverse-mapping picks the right rows). The migration is
~1700 lines; it's a single file by necessity (each step depends on
the previous) but the section comments (`§1` through `§10`) make it
navigable.

## The denormalized read-cache pattern

The big architectural choice in M0: **CSV columns on `issues` stay
as a denormalized read-cache rebuilt from junction writes**. The
junction tables become the sole source of truth on writes from M4
onward, but list views + the OPDS feed + the search index all keep
reading the CSV columns the way they did pre-M0.

### Which columns are caches

On `issues`:

- `writer`, `penciller`, `inker`, `colorist`, `letterer`,
  `cover_artist`, `editor`, `translator` — comma-joined names per
  role, rebuilt from `issue_credits` joined to `person.name`
- `characters`, `teams`, `locations` — same shape, rebuilt from the
  per-entity junctions
- `story_arc`, `story_arc_number` — joined from `issue_arcs`
- `genre`, `tags` — joined from `issue_genres` + `issue_tags`

On `series`:

- `publisher`, `imprint` — the FK columns (`publisher_id`,
  `imprint_id`) are the truth; the string columns are the cache
- `characters`, `teams`, `locations` — same shape as the issue
  versions

### Why a cache at all

Two reasons:

1. **List-view query shape stays the same.** The OPDS feed
   generator, the library grid, the saved-views filter compiler,
   the search index — all of them read these CSV columns directly.
   Rewriting every consumer to join through junctions on every
   query would balloon the cost of common list queries (thousands
   of issues × tens of role types × N rows per junction).
2. **GENERATED ALWAYS columns can't reference other tables** in
   Postgres, so we can't push the cache into the schema itself.
   Application-side rebuild is the next-best thing.

### Write direction: junctions first, always

Provider applies and user edits write the junction tables and rebuild
the cache (below). Since 2026-10 the **scanner** follows the same
direction for file-tagged metadata: `process.rs` still copies the
ComicInfo strings into the columns and derives the junctions from them
(`metadata_rollup::replace_issue_metadata_*`), but the series rollup
(`rollup_series_metadata`) ends by rebuilding every active issue's
cache for the series from the junctions
(`writers::rebuild_series_issue_csv_cache`, one set-based statement).
So after a scan the columns hold the normalized names the junctions
hold — `"Mike Deodato Jr."`, not the file's `"Mike Deodato, Jr."` —
and every column reader (library grid, OPDS, search, saved-view
filters) agrees with the creator / character pages without parsing
rules of its own. The file's literal values remain in
`comic_info_raw`. The cache joiner uses `; ` when any name in the list
contains a comma, matching `split_csv` and the sidecar composer, and
orders every list by the junction's `ordinal` (the position in the
source list — ComicInfo CSV order for scanner writes, provider order
for applies; `issue_credits` had it already, `m20270607` added it to
characters / teams / locations / genres / tags) so a sidecar rewrite
reproduces the file's order rather than alphabetizing. It LEFT JOINs
the entity tables and falls back to the junction's own stored name, so
a file-tier row whose entity id the rollup hasn't linked yet is never
dropped from the column.

The issue detail endpoint reads the junctions directly:
`IssueDetailView.credits` (role, person, slug), `cast` (characters /
teams / locations / story arcs with slugs), `genres`, `tag_list`. The
web issue page renders those and never splits a column. The admin
Metadata dashboard's **Rebuild read-cache** backfill
(`BackfillKind::CsvCache`) rewrites the columns for the whole
catalogue — the catch-up for issues scanned before this rule.

### How the cache stays consistent

`writers::CsvRebuildBatch` queues `(issue_id)` keys touched during a
single transaction. Every `set_issue_*` call appends to the batch;
at transaction commit (or via `rebuild_issue_csv_cache` called
explicitly by the apply pipeline), the batch flushes one rebuild
per touched issue — not per junction-table write.

The rebuild SQL re-reads the junction rows + writes the comma-joined
strings back to the cache columns. Reads outside a transaction see a
consistent (junction + cache) state because the rebuild is in the
same transaction as the junction writes.

**Reviewer heuristic:** any new writer that touches an issue's
junction tables MUST `.queue(issue_id)` into the
`CsvRebuildBatch`. Forgetting means the OPDS feed serves stale
data until the next apply touches the same issue. The
`set_issue_*` helpers in `writers.rs` are the audited surface —
direct INSERT INTO `issue_credits` etc. from new code is wrong.

## external_ids — replacing the legacy trio

The pre-M0 shape:

```sql
ALTER TABLE issues
    ADD COLUMN comicvine_id BIGINT,
    ADD COLUMN metron_id    BIGINT,
    ADD COLUMN gtin         TEXT;
```

The M0 shape:

```sql
CREATE TABLE external_ids (
    entity_type      TEXT NOT NULL,  -- 'series'|'issue'|'person'|'character'|…
    entity_id        TEXT NOT NULL,  -- UUID-cast OR BLAKE3-hex for issues
    source           TEXT NOT NULL,  -- 'comicvine'|'metron'|'gcd'|'marvel'|…
    external_id      TEXT NOT NULL,
    external_url     TEXT,
    set_by           TEXT NOT NULL,  -- 'user'|'comicinfo'|'metroninfo'|'comicvine'|…
    first_set_at     TIMESTAMPTZ NOT NULL,
    last_synced_at   TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (entity_type, entity_id, source)
);
```

Wins:

- **Unlimited sources.** Adding GCD / Marvel / LoCG / MAL etc. no
  longer requires a migration per source — just a new variant on
  the [`Source`](../../crates/server/src/metadata/identifier.rs) enum.
- **Per-entity-type uniformly.** Persons, characters, teams,
  story_arcs etc. all carry their own provider IDs now, not just
  the headline series + issue rows. Cross-source matching ("the
  CV character page for Magneto IS the Metron character page")
  becomes a 1-line lookup.
- **Per-row provenance.** Every row records who set it
  (`set_by`) + when (`first_set_at`, `last_synced_at`). The
  user-precedence rule lives in
  [`writers::set_external_id`](../../crates/server/src/metadata/writers.rs):
  user-set rows are never silently overwritten by provider writes.

The `entity_id` column is `TEXT` because Folio's issue ids are
BLAKE3 hashes (string), not UUIDs. Casting UUIDs to text via
`::text` for series / person / etc. keeps a single primary-key
shape across entity types.

Legacy callers that still want the CV/Metron/GTIN trio (the
issue PATCH form, OPDS responses) read through
[`writers::fetch_legacy_id_trio`](../../crates/server/src/metadata/writers.rs)
which reverse-projects to the old shape.

## metadata_run + metadata_run_candidate

Every search creates one `metadata_run` row + N `metadata_run_candidate`
rows (one per ranked candidate). The dialog polls
`/series/{slug}/metadata/candidates?run_id=…` until the run's status
flips out of `queued`/`searching`.

Run lifecycle states:

- `queued` — row exists; worker hasn't picked it up
- `searching` — worker is fanning out to providers
- `completed` — at least one provider returned successfully
- `failed` — every provider returned a hard error, OR a non-quota error
- `awaiting_quota` — every enabled provider hit quota; `resume_after`
  set to the earliest refill time

The candidate rows survive the run, so re-rendering the dialog
after closing + reopening doesn't re-burn provider budget. Per-entity
Redis coalesce keys (`metadata:search:series:{id}`, `SET NX EX 60s`)
collapse rapid re-clicks into the same in-flight run.

`metadata_run_candidate.applied_at` flips when a candidate is
applied. (An earlier `dismissed_at` column backed the removed admin
review queue; it was dropped in `m20270110` once nothing wrote it.)

## issue_cover + series_cover

Replaces the pre-M0 model where every issue had exactly one
on-disk `cover.webp` discovered by file path. The new model:

```sql
CREATE TABLE issue_cover (
    id                          UUID PRIMARY KEY,
    issue_id                    TEXT NOT NULL,
    kind                        TEXT NOT NULL,  -- 'primary'|'variant'|'back'|'incentive'
    ordinal                     INTEGER NOT NULL,
    source_provider             TEXT,  -- 'comicvine'|'metron'|'archive_extracted'|'user_upload'
    source_external_id          TEXT,
    source_url                  TEXT,
    variant_label               TEXT,
    variant_artist_person_id    UUID REFERENCES person(id) ON DELETE SET NULL,
    local_path                  TEXT NOT NULL,
    width                       INTEGER,
    height                      INTEGER,
    phash                       BIGINT,  -- M9
    dhash                       BIGINT,  -- M9
    ahash                       BIGINT,  -- M9
    fetched_at                  TIMESTAMPTZ NOT NULL,
    is_active                   BOOLEAN NOT NULL
);
-- One ACTIVE row per slot; any number of inactive (history / hash) rows.
CREATE UNIQUE INDEX issue_cover_active_slot_uniq
    ON issue_cover (issue_id, kind, ordinal) WHERE is_active;
```

> **Slot uniqueness is partial (since `m20270218_000001_issue_cover_active_unique`).**
> The M0 migration shipped a *table-level* `UNIQUE (issue_id, kind,
> ordinal)` instead of the partial index this design called for, so an
> inactive row still occupied the slot. Two consequences until the fix:
> replacing an active primary (`apply_cover` deactivate + insert) always
> hit the constraint and left the issue with **no** active primary plus
> an orphaned file, and every issue the post-scan phash worker had
> touched (it keeps an inactive `archive_extracted` `primary/0` row as
> the matcher's side-channel) refused *any* provider primary cover
> (`cover_skipped_reason = "write_failed: …"`). The migration swaps the
> constraint for the partial index above (`series_cover` gets the same
> `series_cover_active_slot_uniq`). Its `down` is lossy — it collapses
> each slot to one row (active first, else newest) before restoring the
> table constraint. Runtime `ON CONFLICT` on this slot must name the
> index predicate: `ON CONFLICT (issue_id, kind, ordinal) WHERE is_active`.

Wins:

- **Multiple covers per issue** — variants, alternate-print, back
  covers, incentive editions. The dialog's `<CoverGallery>` renders
  them.
- **Per-cover provenance** — `source_provider='archive_extracted'`
  for covers ripped by the scanner; `'comicvine'`/`'metron'` for
  provider-applied; `'user_upload'` reserved for a future direct
  upload flow.
- **Per-cover phash** — M9 perceptual hashes (`phash`, `dhash`,
  `ahash` as 64-bit signed ints) so the matcher can use cover
  similarity as a confidence factor.
- **is_active flip** — `apply_cover` deactivates the prior active
  row and inserts the new one **in one transaction** (a failed insert
  rolls the deactivation back and removes the file it just wrote); the
  deactivated row stays for history (and is recoverable by an admin
  flip). `set_issue_variants` likewise swaps the whole variant set in
  one transaction and only deletes the previous set's files after the
  commit. Deactivated primary rows still own their files on disk — the
  WP-3.8 cleanup sweep for those files (deferred in PR #899 pending this
  fix) is now unblocked.

`series_cover` is the analogous table for series-level "banner"
images that providers return separately from per-issue covers.

## field_provenance

Generalizes the pre-M0 `issue.user_edited` JSON array (an array of
column names the user manually edited) into a typed table:

```sql
CREATE TABLE field_provenance (
    entity_type           TEXT NOT NULL,
    entity_id             TEXT NOT NULL,
    field                 TEXT NOT NULL,  -- MetadataField::key() or an issue column pin key — closed sets
    set_by                TEXT NOT NULL,  -- 'user'|'comicinfo'|'metroninfo'|'comicvine'|…
    set_at                TIMESTAMPTZ NOT NULL,
    source_external_id    TEXT,           -- the provider's id, when applicable
    PRIMARY KEY (entity_type, entity_id, field)
);
```

Wins:

- **Typed field keys.** [`MetadataField`](../../crates/server/src/metadata/field.rs)
  enum encodes every legal value; `key()` produces the stable string
  stored in the column; `from_str` round-trips. A unit test
  enumerates every variant to catch missing arms.
- **Junction-level provenance.** "characters[] was last set by
  Metron at 2026-04-15" is now expressible — the JSON shape only
  tracked scalar columns.
- **Cross-entity uniformity.** Persons / characters / teams /
  publishers all get provenance rows on the same shape, not just
  issues.

The user-precedence rule that the scanner uses (skip overwriting
fields the user touched) reads `field_provenance.set_by = 'user'`.

### Retirement of `issue.user_edited` (WP-3.7)

`field_provenance` is the **only** pin store. The `issues.user_edited`
column was dropped by
[`m20270216_000001_retire_user_edited`](../../crates/migration/src/m20270216_000001_retire_user_edited.rs)
(roadmap WP-3.7, audit AR-6 / DI-8):

- **Two key families for issue pins.** A user edit
  (`PATCH /series/{s}/issues/{i}` or the bulk-metadata PATCH) writes a
  `set_by='user'` row under the **column key** it touched *and* under
  the `MetadataField` key that column rolls up into — `writer` →
  `credits`, `genre` → `genres`, `number_raw` → `number`,
  `year`/`month`/`day` → `cover_date`, `story_arc` → `story_arcs`,
  `gtin` → `external_id.gtin`, … (keys equal for `title`, `tags`,
  `language_code`, …). Columns with no `MetadataField` slot
  (`sort_number`, `black_and_white`, `alternate_series`, `web_url`)
  get the column key only. Both families are closed sets:
  `MetadataField::key()` and
  [`writers::ISSUE_COLUMN_PIN_KEYS`](../../crates/server/src/metadata/writers.rs)
  (`issue_column_pin_field` is the rollup). The only write path is
  `writers::write_issue_user_pins`, run in the same transaction as the
  column update.
- **Who reads which key.** The composer / apply / drift paths read the
  `MetadataField` keys (plus `number_raw`). The scanner's rescan gates
  use `protected(MetadataField)` for slotted columns and the column key
  for `sort_number`, `black_and_white`, `alternate_series`, `web_url`
  and `number_raw` (which also honours a `number` user pin). The issue
  detail view exposes the column keys as `user_pinned_columns` (the
  edit form's per-field release icons); the metadata overview exposes
  every user-pinned key as `user_pinned_fields`.
- **Release keeps both families consistent.**
  `DELETE …/field-provenance/{field}` goes through
  `writers::clear_issue_user_pin`: releasing a `MetadataField` key
  drops every column pin rolling into it; releasing a column key drops
  the rolled-up pin once no sibling column is still pinned.
- **Migration.** `up` backfills every string entry of the old JSON
  list under both key families (`set_by='user'`, `set_at` = the
  issue's `updated_at`); a file-tier row at the same key is upgraded,
  a provider row is kept (an `override_user_edits` apply deliberately
  replaced the pin), then the column is dropped. `down` re-adds the
  column (`jsonb NOT NULL DEFAULT '[]'`) and repopulates it from the
  column-key user pins. Round-trip test:
  `crates/server/tests/migration_retire_user_edited.rs`.
- **API.** `IssueDetailView.user_edited` became `user_pinned_columns`
  and `MetadataOverviewView.user_edited` became `user_pinned_fields`
  (both `string[]`, sorted).

## Entity rows for scanner-minted names (WP-5.5)

The M0 backfill created `character` / `team` / `story_arc` /
`publisher` rows once. The scanner keeps writing name-only junction rows
(`character_id` / `team_id` NULL, arcs only in the `issues.story_arc`
CSV, `series.publisher_id` NULL), so names first seen after M0 had no
entity row — and therefore no slug for the `/characters/{slug}` etc.
landing pages.

- `m20270305_000001_entity_pages` re-runs the entity backfill for those
  names (base slug, or base + 8-hex `md5(normalized_name)` when the base
  is taken), then **links the rows by id**: NULL
  `issue_characters.character_id` / `series_characters.character_id` /
  `issue_teams.team_id` / `series_teams.team_id` / `series.publisher_id`
  are filled by normalized name, and `issue_arcs` is reconciled to the
  `story_arc` CSV for issues whose arcs are file-owned. It also adds
  `btrim(lower(name))` expression indexes for the remaining name fallback.
- The series rollup keeps both up to date on every scan:
  `writers::ensure_series_entity_rows` mints missing entity rows (next to
  `ensure_persons_for_series`), then
  `metadata_rollup::link_series_entity_ids` runs the same link statements
  scoped to the series (mirror of the `issue_credits.person_id` fill), and
  the `series_characters` / `series_teams` rebuild carries the FK along.
  - FKs are filled **only while NULL** — a provider apply links by
    external identifier, so its entity may carry a different normalized
    name than the junction text; a name match must never re-point it.
  - `issue_arcs` is reconciled (insert missing, drop arcs no longer named
    in the CSV; position from a numeric `story_arc_number` when the issue
    names a single arc) **only** for issues with no provider / user
    `field_provenance` on `story_arcs` — the same file-tier rule the
    scanner applies to every other junction (WP-2.5).
  - Entity inserts are `ON CONFLICT DO NOTHING`, so concurrent rollups
    racing on a shared name never error.
- Reads (`api::entity_pages`) join by id; a name fallback
  (`btrim(lower(name)) = normalized_name`) remains only where the id can
  still be NULL (a row scanned since the last rollup). Arc membership is
  `issue_arcs` alone — no per-row CSV split on the read side.

## ID column shapes

A subtle but important detail: `entity_id` on `external_ids` and
`field_provenance` is `TEXT`, not `UUID`. The reason: Folio's issue
ids are BLAKE3 content hashes (64-character hex strings), not
UUIDs. Persons / characters / series / etc. are UUIDs but get
cast to text for storage (`series_id::text`).

This means writers always stringify ids before binding:

```rust
writers::set_external_id(
    db,
    "series",
    &series_uuid.to_string(),  // <- cast required
    &identifier,
    SetBy::Provider(source),
).await?;
```

Conversely, readers parse back to UUID when the caller wants one:

```rust
let series_uuid = Uuid::parse_str(row.entity_id)
    .map_err(|e| ApplyError::InvalidScope(format!("...")));
```

This wart is documented in the migration comments + the entity
type docstrings. The alternative (`entity_id_uuid UUID NULL` +
`entity_id_text TEXT NULL` + a CHECK constraint enforcing exactly
one) would clutter every query; we chose the wart.

## Migration rollback

The `down()` path reverses every backfill in reverse order:

1. Re-add the legacy ID columns
2. Backfill them from `external_ids` rows where `set_by='migration_v1'`
3. Drop `external_ids`, `metadata_run`, `metadata_run_candidate`,
   `issue_cover`, `series_cover`, `field_provenance`, `issue_reprint`
4. Drop FK columns on existing junctions
5. Drop the new top-level entity tables

Tested via the migration harness's `up → down → up` sweep. User-set
external_id rows + provider-set rows added *after* the M0 migration
ran are silently dropped on rollback (we only re-mirror migration_v1
rows). Operators rolling back after using the new system should
treat that data as lost.

## Adding a new field that needs provenance

1. Add a `MetadataField::<Name>` variant + the matching
   `key()` arm + entry in `SCALAR_FIELDS`. The
   `key_round_trip_for_every_variant` unit test catches forgotten
   pieces.
2. Apply / diff code paths automatically pick up the new variant
   (they iterate `MetadataField::iter()`).
3. If the field has a dedicated junction table, add a writer in
   `writers.rs` that updates both the junction and the CSV cache.

## Adding a new entity type

Mirror `person`'s shape:

```sql
CREATE TABLE <name> (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    slug            TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    normalized_name TEXT NOT NULL UNIQUE,
    aliases         JSONB NOT NULL DEFAULT '[]'::jsonb,
    description     TEXT,
    image_url       TEXT,
    -- entity-specific cols here
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Plus:

- Junction table(s) connecting to issues / series with `(parent_id, <name>_id)` PK
- `Source` enum carries the entity through external_ids automatically
- `MetadataField` variant + `is_junction()` arm if it's a list field
- Writer helper `set_issue_<plural>` in `writers.rs` that also
  rebuilds the CSV cache when present

The pattern is intentionally repetitive across entity types — the
duplication is cheaper than the abstraction over it would be, and
it lets the developer eye-grep one file (`writers.rs`) for the
full surface.
