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
6. `field_provenance` — generalize `issue.user_edited` JSON into
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

`metadata_run_candidate.applied_at` flips when the user applies a
candidate; `dismissed_at` flips on dismiss. The admin Review Queue
reads candidates where both are NULL + bucket ∈ {medium, low}.

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
    is_active                   BOOLEAN NOT NULL,
    UNIQUE (issue_id, kind, ordinal, is_active) WHERE is_active = TRUE
);
```

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
  row before inserting the new one; the deactivated row stays for
  history (and is recoverable by an admin flip).

`series_cover` is the analogous table for series-level "banner"
images that providers return separately from per-issue covers.

## field_provenance

Generalizes the pre-M0 `issue.user_edited` JSON array (an array of
column names the user manually edited) into a typed table:

```sql
CREATE TABLE field_provenance (
    entity_type           TEXT NOT NULL,
    entity_id             TEXT NOT NULL,
    field                 TEXT NOT NULL,  -- MetadataField::key() — closed set
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
The pre-M0 read path (`issue.user_edited`) still works in parallel
during the transition window; `field_provenance` is the long-term
home (`issue.user_edited` is targeted for removal in M10).

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
