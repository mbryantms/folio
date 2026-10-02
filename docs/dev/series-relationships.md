# Series relationships

WP-7.1 of the product roadmap (spec §5.2 / Phase 7). Series carry typed,
directed edges to other series: sequels, prequels, spin-offs, crossovers,
collected editions, shared universes and a catch-all "see also". The
suggestion engine (WP-7.2) and its review UI (WP-7.3) build on this layer.

## Schema

`series_relationship` (migration `m20270501_000001_series_relationship`,
entity `crates/entity/src/series_relationship.rs`):

| column | type | notes |
|---|---|---|
| `id` | `uuid` PK | |
| `from_series_id` | `uuid` FK → `series` ON DELETE CASCADE | the subject: "*from* is a sequel of *to*" |
| `to_series_id` | `uuid` FK → `series` ON DELETE CASCADE | indexed (`series_relationship_to_series`) |
| `kind` | `text` | CHECK in the kind list below |
| `source` | `text` default `'manual'` | CHECK `manual` / `suggested` |
| `confidence` | `real` NULL | 0.0–1.0 (CHECK); set for accepted suggestions, NULL for manual edges |
| `created_by` | `uuid` NULL FK → `users` ON DELETE SET NULL | the admin who made the edge |
| `created_at` | `timestamptz` default `now()` | |

Constraints: `UNIQUE (from_series_id, to_series_id, kind)` and
`CHECK (from_series_id <> to_series_id)`. Deleting a series removes both
halves of every pair it took part in (the FK cascade).

## Kinds and inverses

Every edge is stored **together with its inverse**, so "what is related to
X" is a single `WHERE from_series_id = X` with no `UNION`.

| kind | inverse | reads as |
|---|---|---|
| `sequel_of` | `prequel_of` | *from* continues *to*; *to* is read first |
| `prequel_of` | `sequel_of` | *from* is read before *to* |
| `spin_off_of` | `has_spin_off` | *from* was spun off from *to* |
| `has_spin_off` | `spin_off_of` | *from* spawned *to* |
| `collects` | `collected_in` | *from* (a TPB / omnibus series) collects *to* |
| `collected_in` | `collects` | *from* is collected in *to* |
| `crossover_with` | itself | |
| `same_universe` | itself | |
| `see_also` | itself | |

`has_spin_off` isn't in the roadmap's original kind list; it was added so
`spin_off_of` has a distinct inverse. A self-inverse kind stores the reverse
row with the same kind (`A see_also B` + `B see_also A`).

A directional kind contradicts its own inverse on the same ordered pair:
asking for `A sequel_of B` while `A prequel_of B` exists is refused
(`PairError::Conflict`, HTTP 409).

The typed enum is `server::relationships::RelationshipKind` (serde
snake_case + `ToSchema`, with `inverse()`, `label()`, `as_str()`), next to
`RelationshipSource` (`manual | suggested`). The DB CHECK mirrors the enum:
adding a kind needs both a migration and the enum variant.

## Write surface

`crates/server/src/relationships/mod.rs` is the **only** writer. Never
insert or delete `series_relationship` rows directly.

```rust
pub async fn create_pair<C: ConnectionTrait>(
    conn: &C, from: Uuid, to: Uuid, kind: RelationshipKind,
    source: RelationshipSource, confidence: Option<f32>, created_by: Option<Uuid>,
) -> Result<PairOutcome, PairError>;           // PairOutcome { forward, inverse, created }

pub async fn delete_pair<C: ConnectionTrait>(
    conn: &C, from: Uuid, to: Uuid, kind: RelationshipKind,
) -> Result<Option<series_relationship::Model>, DbErr>;  // deleted forward row

pub async fn delete_pair_by_id<C: ConnectionTrait>(conn: &C, id: Uuid)
    -> Result<Option<series_relationship::Model>, DbErr>;  // either half's id
```

- Pass a `DatabaseTransaction` so both halves land, or vanish, together.
- `create_pair` is **idempotent**: an existing pair comes back with
  `created = false` (a missing inverse half is re-created). Both inserts are
  `ON CONFLICT DO NOTHING`, so concurrent creates are safe. WP-7.2's
  suggestion-accept path calls it with `RelationshipSource::Suggested` and
  the suggestion's confidence.
- `create_pair` does not check that the series exist (the FK does) or that
  the caller may see them (the HTTP layer does).

## Traversal

```rust
pub const MAX_TRAVERSAL_DEPTH: u32 = 6;
pub async fn traverse<C>(conn, start, kinds: &[RelationshipKind], max_depth: u32)
    -> Result<Vec<TraversalNode>, DbErr>;      // { series_id, depth, parent_id }
pub async fn chain<C>(conn, start) -> Result<Vec<ChainNode>, DbErr>;
                                               // { series_id, position, parent_id }
pub async fn direct<C>(conn, series_id) -> Result<Vec<series_relationship::Model>, DbErr>;
```

`traverse` is a raw-SQL recursive CTE (one of the spec's sanctioned raw-SQL
escape hatches). It follows edges whose kind is in `kinds`, clamps depth to
6, and is **cycle-safe**: each recursive row carries its visited path and
never re-enters a node on it. Each node is reported once, at its shortest
depth, with the `parent_id` it was reached through. The start series is
never returned.

`chain` builds the reading order: it walks `sequel_of` edges for what comes
before (negative positions) and `prequel_of` edges for what comes after
(positive positions), up to 6 hops each way, with the start series at
position 0. Positions can repeat when the chain branches (two sequels of
the same book). A node that a cycle puts on both sides is kept on the
"before" side only.

Dense self-inverse graphs (`same_universe` across a whole publisher) make
path enumeration expensive at depth 6. The chain only follows
sequel/prequel edges, so this doesn't affect the series page. A caller that
traverses `same_universe` should pass a small `max_depth`.

## HTTP API (`api` group; `crates/server/src/api/series_relationships.rs`)

| method | path | who | result |
|---|---|---|---|
| `GET` | `/api/series/{slug}/relationships` | any user who can see the series | `SeriesRelationshipsResp` |
| `POST` | `/api/series/{slug}/relationships` | `RequireAdmin` | `201` new / `200` existing → `SeriesRelationshipView` |
| `DELETE` | `/api/series/{slug}/relationships/{id}` | `RequireAdmin` | `204` |

```jsonc
// GET
{
  "series_id": "…",
  "relationships": [{
    "id": "…",                 // row id; pass to DELETE
    "kind": "sequel_of",       // from this series' point of view
    "kind_label": "Sequel of",
    "source": "manual",        // | "suggested"
    "confidence": null,        // 0–1 for suggested
    "created_at": "…",
    "series": { /* SeriesView, hydrated like a grid card: slug, name, year, cover_url, issue_count, … */ }
  }],
  "chain": [                   // empty when there are no sequel/prequel edges
    { "position": -1, "series": { /* SeriesView */ } },
    { "position": 0,  "series": { /* this series */ } },
    { "position": 1,  "series": { /* SeriesView */ } }
  ]
}

// POST body: "this series <kind> target"
{ "target": "saga-2012" /* slug or UUID */, "kind": "sequel_of" }
```

Errors: `422 validation` for a self edge, a blank target or a malformed
body; `404` when the series or target is missing; `409 conflict` for a
contradicting directional kind; `403` for non-admin writes. A duplicate
POST is **not** an error: it returns the existing forward row with `200`
and writes no audit row.

`DELETE` takes either half's id (it must touch `{slug}`) and removes both
halves.

**Permissions and ACL.** `GET` answers 404 when the caller can't see the
series. Related series are filtered to those the caller can see too:
library grant and age-rating cap (`VisibleLibraries::series_ok`), and
non-admins never see removed series. The chain is pruned so that a hidden
link also hides every series reached through it. Nothing beyond a series
the caller can't see leaks.

**Audit.** `admin.series.relationship.create` (only when a pair was actually
inserted) and `admin.series.relationship.delete`, both with target
`("series", <slug's series id>)` and the edge ids/kinds in the payload.

**Pagination.** The list isn't cursor-paginated. Relationships are curated
(admin-made or accepted suggestions), so the set is bounded by the domain,
like `/me/sessions`. If WP-7.2 starts accepting `same_universe` suggestions
in bulk, revisit this.

## OPDS

Series feeds link related series the caller can see:

- OPDS 1.2 `/opds/v1/series/{id}`: a feed-level
  `<link rel="related" href="/opds/v1/series/{other}" type="…acquisition" title="Sequel of: Saga (2012)"/>`
  for each one.
- OPDS 2.0 `/opds/v2/series/{id}`: a `links[]` entry
  `{ "rel": "related", "href": "/opds/v2/series/{other}", "type": "application/opds+json", "title": …, "properties": { "folio:relationship": "sequel_of" } }`.

Both use `api::series_relationships::visible_related`, which applies the
same ACL as the JSON `GET`.

## Suggestion engine (WP-7.2)

An apalis job proposes relationships from evidence already in the DB. It
**never creates edges**: it writes candidate rows that an admin accepts
(which calls `create_pair` with `source = suggested`) or rejects.

Code: `crates/server/src/relationships/suggestions/` (`mod.rs` = merge,
upsert and review service; `sources.rs` = one set-based query per evidence
source; `citations.rs` = the "Collects X #1-6" parser), the job in
`crates/server/src/jobs/relationship_suggest.rs`, and the API in
`crates/server/src/api/relationship_suggestions.rs`.

### Schema

`series_relationship_suggestion` (migration
`m20270502_000001_relationship_suggestion`, entity
`entity::series_relationship_suggestion`):

| column | type | notes |
|---|---|---|
| `id` | `uuid` PK | |
| `from_series_id`, `to_series_id` | `uuid` FK → `series` ON DELETE CASCADE | "*from* `kind` *to*" |
| `kind` | `text` | canonical kinds only (below) |
| `confidence` | `real` | 0–1 |
| `bucket` | `text` | `high` (≥ 0.8) / `medium` (≥ 0.55) / `low` |
| `reason` | `text` | human-readable, one clause per source |
| `evidence` | `jsonb` | `{ "sources": [ { "source": "story_arc", "confidence": 0.65, "reason": "…", …source fields } ] }` |
| `status` | `text` | `pending` / `accepted` / `rejected` / `modified` / `stale` (`stale` added by `m20270503_000001_relationship_suggestion_stale`, WP-7.3) |
| `accepted_kind` | `text` NULL | the kind actually created when accepted with an override (`modified`) |
| `created_at`, `updated_at` | `timestamptz` | |
| `reviewed_at` | `timestamptz` NULL | |
| `reviewed_by` | `uuid` NULL FK → `users` ON DELETE SET NULL | |

`UNIQUE (from_series_id, to_series_id, kind)`. **Canonical form**, enforced
by CHECKs: self-inverse kinds (`crossover_with`, `same_universe`,
`see_also`) are stored with `from < to`, so A→B and B→A are one row;
directional kinds are stored in one direction only (`sequel_of`,
`spin_off_of`, `collects`, never their inverses). `canonicalize()` folds
every candidate onto that form before the upsert.

Rows are **never deleted** (spec §5.7); only the series FK cascade removes
them. Only the status moves: a review (`accepted` / `rejected` /
`modified`) leaves `pending` once; the one way back is an admin
**reopening** a rejection (WP-7.3). `stale` is the engine's own state, not
a review (see "Review UI" below).

### Evidence sources and confidence

Every query is scoped to **one library**: suggestions only link series in
the same library. Libraries are usually split by publisher or format, cross-
library continuations are rare, and per-library scoping keeps a run
proportional to the library that was just scanned. Removed series are
ignored. Pair-producing sources use a star (members → hub) or adjacency
(`lag()` over an ordered partition) shape, never all-pairs, and each query
caps its output at 5000 rows.

| source | kind | confidence |
|---|---|---|
| **Name continuation**: series grouped by `(base name, publisher)`, where the base name is `normalized_name` minus a trailing `vol N` / `vN` / year (1930–2049) token, or the parent folder's name when the series folder is just `Vol N`; each series pairs with its predecessor by `(year, volume)` | `sequel_of` (later → earlier) | consecutive volumes 0.9 · later volume with a gap 0.65 · later year, no volumes 0.7 · year and volume order disagree 0.45 · same name and same year → `see_also` 0.35 |
| **AlternateSeries**: ComicInfo `AlternateSeries` on issues of A, split on `,`/`;`, matched to a series by normalized name (closest year to the citing issues wins; A's own title is ignored) | `crossover_with` | plain value 0.75 · ComicVine reading-list style `"Avengers" Civil War` 0.5 · +0.05 for ≥ 3 issues · −0.05 when several series share the name · −0.15 when the year gap is > 3 |
| **SeriesGroup**: series sharing a normalized `series_group`; star onto the member whose name equals the group (else the oldest) | `same_universe` | group ≤ 12 series 0.85 · ≤ 40 0.7 · larger 0.5 |
| **Story arc**: arcs in `issue_arcs` spanning ≥ 2 series; star onto the series with the most issues in the arc, aggregated across arcs; same-name pairs skipped | `crossover_with` | 0.5 (one issue on the far side) or 0.65 (≥ 2), +0.1 per extra shared arc (max 0.9) · capped at 0.45 when the smallest shared arc spans > 10 series (event tie-ins) |
| **Collected edition**: issues whose `Format` / `special_type` / series type / series name marks a TPB, HC, omnibus or graphic novel; their title, notes and a "Collects…"/"Reprints…" summary are parsed for `Name #lo-hi` (or `issues lo-hi`) citations; an unnamed citation means the edition's own title minus format words | `collects` (edition → collected series) | issue coverage in the library ≥ 80% 0.85 · ≥ 50% 0.7 · some 0.55 · none 0.4 · −0.1 for an unnamed citation · −0.1 when the name is ambiguous and nothing is covered |
| **Shared provider volume**: local series whose first issue's ComicInfo `comicvine_series_id` / `metron_series_id`, or whose series-level `external_ids`, name the same provider series (2–12 claimants). The series-level `external_ids` row is unique per provider id, so a second claimant only shows up through its issues | `sequel_of` when issue ranges are disjoint and ordered (the provider sees one continuous run split into several local series), else `see_also` (overlapping numbers: probably duplicate copies) | 0.8 · `see_also` 0.6 (0.55 without issue numbers) |
| **Provider range**: a `series_provider_range` row on A pointing at provider series P while another local series B is matched to P | `see_also` | 0.7. Not `sequel_of`: the range sits inside A (A is not read entirely before or after B), and B usually duplicates those issues rather than continuing them |
| **Character/team density**: same publisher, sharing ≥ 5 *uncommon* characters/teams (`series_characters` / `series_teams`) with overlap ≥ 0.5 of the smaller set. "Uncommon" means present in at most 2% of the library's series that have character data (clamped to 3–25), which also bounds the self-join. Each series keeps at most 3 partners, counted across both ends | `same_universe` | 0.25 + 0.25 × overlap, so **always low** (≤ 0.5) |

`same_universe` is deliberately conservative (WP-7.1 flagged that bulk-
accepting it bloats the series page and traversal). Only an explicit
`SeriesGroup` can reach high, and big groups decay to medium/low.

**Merging.** Candidates landing on the same canonical row merge: confidence
is the strongest source's plus 0.05 for each additional distinct source
(max 0.99), reasons are joined strongest first, and `evidence.sources`
keeps one entry per source.

### Dedupe, rejection memory, cap

Per run, after merging:

1. Drop a proposal whose `(from, to)` already has an edge with the same kind
   **or its inverse** (the inverse would 409 on accept). Because edges are
   stored as pairs, one lookup covers both orientations.
2. Drop a proposal whose row is already `accepted` / `rejected` /
   `modified`. A rejected suggestion never reappears; re-suggesting needs
   the rejection cleared by hand (spec §5.7) — the WP-7.3 **reopen**
   action. `stale` rows are not rejection memory.
3. Keep the top **1000** by confidence (`MAX_SUGGESTIONS_PER_RUN`; ties by
   ids, so it's deterministic). The rest count as `capped` in the report
   and come back on a later run once reviews free up room.
4. Upsert: new rows insert as `pending`; a still-pending row gets its
   confidence, bucket, reason and evidence refreshed when they changed, and
   a `stale` row is revived to `pending`. The
   `ON CONFLICT … WHERE status IN ('pending', 'stale')` guard means a review
   racing the run is never overwritten.
5. Stale marking (WP-7.3): every `pending` row of the library that this run
   did not produce (kept **or** capped) becomes `stale`. Skipped entirely
   when any evidence source errored (`RunReport.failed_sources`), so a
   transient SQL error can't empty the review queue. The report counts
   `revived` and `marked_stale`.

### Hooking and observability

- **After scans**: the scanner's finalize step calls
  `jobs::relationship_suggest::enqueue` when the scan changed anything
  (files added/updated, series created/removed, issues removed/restored),
  for full, watcher-scoped and series-scoped scans alike.
- **On demand**: `POST /api/admin/relationship-suggestions/run`
  (`?library_id=` or every library).
- **Dedupe**: `SET NX EX` on `relsuggest:queued:<library_id>` (30-minute
  TTL). The handler deletes the key when it starts, so a scan finishing
  mid-run queues exactly one follow-up. Queue `relationship_suggest`,
  concurrency 1, listed on the admin Queue page and in dead-letter counts.
- **Observability**: a `relationship_suggest` tracing span with
  `library_id`, a `debug` line per source (candidates, elapsed), and an
  `info` "run complete" line with the full report. Runs that inserted or
  changed rows write a `library_events` row (category `series`, action
  `generated`, `detail.kind = "relationship_suggestions"`, `detail.report`).
- **Runtime**: the stress test (3,000 series, 7,496 proposals) runs in
  about 0.25 s. The real dev library (2,573 series) takes about 0.45 s.

### Service API

```rust
// crate::relationships::suggestions
pub async fn generate_for_library<C: ConnectionTrait>(conn: &C, library_id: Uuid)
    -> Result<RunReport, DbErr>;
pub async fn accept<C: ConnectionTrait + TransactionTrait>(
    conn: &C, id: Uuid, actor: Uuid, kind_override: Option<RelationshipKind>,
) -> Result<AcceptOutcome, ReviewError>;   // AcceptOutcome { suggestion, kind, pair: PairOutcome }
pub async fn reject<C: ConnectionTrait + TransactionTrait>(conn: &C, id: Uuid, actor: Uuid)
    -> Result<series_relationship_suggestion::Model, ReviewError>;
pub async fn list<C: ConnectionTrait>(
    conn: &C, filter: &SuggestionFilter, cursor: Option<SuggestionCursor>, limit: u64,
) -> Result<SuggestionPage, DbErr>;
// ReviewError { NotFound, AlreadyReviewed { status }, Pair(PairError), Db(DbErr) }
// SuggestionFilter { status, bucket, library_id, series_id } (all Option)
// SuggestionPage { items, next_cursor, total /* first page */, bucket_counts /* first page */ }
// jobs::relationship_suggest::{enqueue(&AppState, library_id) -> bool, run(&DatabaseConnection, library_id)}

// WP-7.3
pub async fn reopen<C: ConnectionTrait + TransactionTrait>(conn: &C, id: Uuid)
    -> Result<(Model /* before */, Model /* after */), ReviewError>;   // rejected → pending
pub async fn bulk_accept<C: ConnectionTrait + TransactionTrait>(conn: &C, ids: &[Uuid], actor: Uuid)
    -> Result<BulkOutcome, DbErr>;   // BulkOutcome { succeeded, created, failed: Vec<BulkFailure { id, error }> }
pub async fn bulk_reject<C: ConnectionTrait + TransactionTrait>(conn: &C, ids: &[Uuid], actor: Uuid)
    -> Result<BulkOutcome, DbErr>;
pub async fn pending_ids_in_bucket<C: ConnectionTrait>(
    conn: &C, bucket: SuggestionBucket, library_id: Option<Uuid>, limit: usize,
) -> Result<(Vec<Uuid>, u64 /* total matching */), DbErr>;
pub const MAX_BULK: usize = 500;
```

- `accept` runs in one transaction (a savepoint when `conn` is already a
  transaction) and locks the row `FOR UPDATE`, so a double accept can't
  create twice. A `kind_override` different from the suggested kind
  records `modified` + `accepted_kind`. The override reads in the row's
  `from → to` direction.
- **Seam for bulk accept**: loop `accept` inside one outer transaction (each
  call becomes a savepoint, so one conflict doesn't sink the batch), write
  **one** audit row for the batch, and call
  `state.similarity.invalidate_all()` once after commit if any
  `pair.created`. The service functions only take a connection, so **every
  caller must invalidate the WP-7.4 similarity cache itself**; the single-
  accept handler does.

### HTTP API (admin only, `api` group)

| method | path | body / query | result |
|---|---|---|---|
| `GET` | `/api/admin/relationship-suggestions` | `status` (`pending` default / `accepted` / `rejected` / `modified` / `stale` / `all` = every status except `stale`), `bucket`, `library_id`, `cursor`, `limit` (1–200, default 50) | `RelationshipSuggestionListView` |
| `GET` | `/api/series/{slug}/relationship-suggestions` | `cursor`, `limit` | pending suggestions with the series on either end |
| `POST` | `/api/admin/relationship-suggestions/{id}/accept` | `{ "kind"?: RelationshipKind }` (send `{}` to accept as suggested) | `AcceptRelationshipSuggestionResp` |
| `POST` | `/api/admin/relationship-suggestions/{id}/reject` | none | `RelationshipSuggestionView` |
| `POST` | `/api/admin/relationship-suggestions/{id}/reopen` | none | `ReopenRelationshipSuggestionResp` `{ suggestion }` (WP-7.3) |
| `POST` | `/api/admin/relationship-suggestions/bulk-accept` | `{ "ids": [...] }` (1–500) **or** `{ "bucket": "high", "library_id"? }` | `BulkReviewRelationshipSuggestionsResp` (WP-7.3) |
| `POST` | `/api/admin/relationship-suggestions/bulk-reject` | `{ "ids": [...] }` (1–500) | `BulkReviewRelationshipSuggestionsResp` (WP-7.3) |
| `POST` | `/api/admin/relationship-suggestions/run` | `?library_id=` | `202` `{ "enqueued": [...], "already_queued": [...] }` |

```jsonc
// RelationshipSuggestionListView
{
  "items": [{
    "id": "…",
    "from_series": { /* SeriesView (cover_url, slug, …) */ },
    "to_series":   { /* SeriesView */ },
    "kind": "sequel_of", "kind_label": "Sequel of",
    "confidence": 0.9, "bucket": "high",
    "reason": "Daredevil vol. 4 (2014) follows Daredevil vol. 3 (2011) — same title, next volume",
    "evidence": { "sources": [ { "source": "name_continuation", "confidence": 0.9, … } ] },
    "status": "pending", "accepted_kind": null,
    "created_at": "…", "updated_at": "…", "reviewed_at": null, "reviewed_by": null
  }],
  "next_cursor": "…",                                   // opaque keyset (confidence desc, id)
  "total": 603,                                         // first page only
  "bucket_counts": { "high": 171, "medium": 275, "low": 157 }  // first page only; ignores `bucket`
}
// AcceptRelationshipSuggestionResp
{ "suggestion": { /* view, status accepted|modified */ }, "relationship_id": "…",
  "inverse_id": "…", "kind": "sequel_of", "created": true }
```

Errors: `400` malformed id / cursor; `403` non-admin; `404` unknown
suggestion / series / library; `409` already reviewed, or the accept
contradicts an existing directional edge (the row stays pending); `422`
malformed body.

**Audit**: `admin.relationship_suggestion.accept` (target
`relationship_suggestion`, payload carries the edge ids, kinds and
`created`), `admin.relationship_suggestion.reject`,
`admin.relationship_suggestion.reopen` (payload keeps the cleared
`rejected_at` / `rejected_by`), `admin.relationship_suggestion.bulk_accept`
and `…bulk_reject` (**one row per batch**, see below), and
`admin.relationship_suggestion.run`. Accepted edges show up in the series
page's "Related" block with the "Suggested" badge (`source = suggested`,
`confidence` set).

### Review UI and bulk accept (WP-7.3)

**Admin page** `/admin/relationships` (nav: Content → Relationships;
`web/components/admin/relationships/RelationshipSuggestionsPanel.tsx`):

- The queue is `GET /api/admin/relationship-suggestions` through
  `useRelationshipSuggestionsInfinite` (IntersectionObserver sentinel).
  Library (select), status (Pending / Accepted / Modified / Rejected /
  Stale / All) and confidence bucket (High / Medium / Low, counts from the
  first page's `bucket_counts`) are **server params** — nothing is filtered
  client-side.
- Each row: both covers and names (linked), the kind label, confidence %,
  bucket, status, the reason, and a collapsible **Evidence** list (one entry
  per source with its fields).
- Pending rows: **Accept**, **Edit kind** (popover with all nine kinds,
  read "*from* is … *to*"; a different kind accepts as `modified`) and
  **Reject**. Rejected rows: **Reopen**. Reviewed rows show the review date.
- **Run now** queues the engine for the selected library (or every library).
- **Accept all high-confidence (N)** sits behind an `AlertDialog` and sends
  bucket mode for the current library filter.
- **Select…** enters multi-select (`useSelection` + `SelectionToolbar`) on
  pending rows: bulk **Accept**, and bulk **Reject** behind an
  `AlertDialog`.

**Series page chips** (`SeriesSuggestedRelationships`, admins only, inside
the Related block): pending suggestions touching the series from
`GET /api/series/{slug}/relationship-suggestions` (cursor-paginated, "Show
more"), read from this series' side (a row stored as "*other* `sequel_of`
*this*" shows "Prequel of *other*"), the reason and confidence in a
tooltip, one-click accept / reject.

Every accept path (single, chip, bulk) invalidates both series'
`seriesRelationships`, the `["similar"]` rails and every suggestion list;
bulk accept invalidates all series' relationship blocks (it doesn't know
which series changed).

**Bulk semantics** (`POST …/bulk-accept`, `…/bulk-reject`):

- **Bounded.** Exactly one of `ids` (1–500, duplicates processed once) or
  `{ "bucket": "high", "library_id"? }`. Bucket mode takes the top 500
  pending high rows by confidence and returns `remaining` (pending high rows
  left after the batch); send it again for the next batch — each request is
  its own batch with its own audit row. Medium / low rows can only be
  bulk-accepted by explicit ids, after someone looked at them. Bulk reject
  takes ids only. Anything else is `422` with field `details`; an unknown
  `library_id` is `404`.
- **One transaction, a savepoint per item.** Each item runs the normal
  `accept` / `reject` (which opens a savepoint inside the outer
  transaction). A per-item refusal — `not_found`, `already_reviewed`,
  `stale`, `conflict` (contradicts an existing directional edge, including
  one an earlier item of the same batch created), `invalid` — rolls back
  only that item and is reported in `failed[]`; the rest commits. A
  database error aborts the whole batch (`500`, nothing commits).
- **One audit row per batch**: `admin.relationship_suggestion.bulk_accept`
  (payload `mode`, `bucket`, `library_id`, `requested`, `accepted`,
  `created`, `failed`, `accepted_ids`, `failures[{id, code}]`,
  `remaining`) or `…bulk_reject` (`requested`, `rejected`, `failed`,
  `rejected_ids`, `failures`). No per-item `accept` / `reject` rows.
- **One similarity invalidation** (`state.similarity.invalidate_all()`)
  per batch, only when `created > 0`.

```jsonc
// BulkReviewRelationshipSuggestionsResp
{ "requested": 5, "succeeded": ["…", "…"], "created": 2,
  "failed": [{ "id": "…", "code": "conflict", "message": "conflicts with existing `prequel_of` relationship" }],
  "remaining": 40 }   // bucket mode only
```

**Reopen** (`POST …/{id}/reopen`): `rejected` → `pending` only (anything
else is `409`). The row is kept; its `reviewed_at` / `reviewed_by` /
`accepted_kind` are cleared so it reads as pending again, and the audit row
keeps who rejected it and when. The next run refreshes it like any pending
row — or marks it `stale` if the engine no longer proposes it.

**Stale** (`m20270503_000001_relationship_suggestion_stale` extends the
status CHECK): set by the engine on pending rows a run no longer produces
(series renamed, character data cleaned up, heuristic tightened, or an
admin linked the pair by hand), and cleared back to `pending` when a later
run produces the row again (same row id). Stale rows are hidden from the
default list and from `status=all`; `status=stale` shows them. Accepting or
rejecting a stale row is `409` ("stale"). Stale is not rejection memory and
never touches reviewed rows. On the dev library the first run with stale
marking retired 31 low `same_universe` rows left over from earlier
heuristic versions; a second run marked none.

### Gaps

- No `spin_off_of` source: nothing in the DB distinguishes a spin-off from
  a crossover or same-universe title.
- Cross-library suggestions are out of scope by design (see above).
- Bulk accept has no kind override (accept-as-modified is per row).

## Web

`web/components/library/SeriesRelatedSection.tsx` renders the "Related"
block on the series page, between the metadata tabs and the issue list:

- a **Reading order** strip (the chain, with this series highlighted);
- direct relationships grouped by kind, with a "Suggested" badge on
  accepted suggestions;
- for admins, an **Add related series** form (kind select plus a
  cursor-paginated series typeahead) and remove buttons behind an
  `AlertDialog` confirm.

Hooks: `useSeriesRelationships` (`queryKeys.seriesRelationships`),
`useCreateSeriesRelationship` and `useDeleteSeriesRelationship`. Both
mutations invalidate the relationship lists of the current series and the
other series.

For admins the block also shows the WP-7.3 **Suggested** chips
(`web/components/library/SeriesSuggestedRelationships.tsx`). The review
hooks live in `web/lib/api/mutations/relationship-suggestions.ts`
(`useAcceptRelationshipSuggestion`, `useRejectRelationshipSuggestion`,
`useReopenRelationshipSuggestion`, `useBulkAccept…`, `useBulkReject…`,
`useRunRelationshipSuggestions`); kind labels and inverses are in
`web/lib/relationships.ts`.

## Tests

- `crates/server/tests/series_relationships.rs`: inverse pair on create and
  delete, self-inverse kinds, idempotent create (200, one audit row),
  conflict 409, self-edge 422 (and the DB CHECK), non-admin 403, ACL
  filtering and chain pruning, cycle safety, the depth-6 cap, FK cascade,
  and OPDS v1/v2 related links.
- `crates/server/src/relationships/mod.rs` unit tests: inverse involution,
  the self-inverse set, and str/serde round-trip.
- `web/tests/library/series-related-section.test.tsx`: chain order and
  highlight, grouping, admin gating, and empty state.
- `crates/server/tests/relationship_suggestions.rs` (WP-7.2): a fixture
  library with one cluster per evidence source (expected kind, bucket and
  reason), canonical dedupe of reversed self-inverse candidates, existing-
  edge dedupe, rejection memory and pending refresh on rerun, accept via
  `create_pair` (plus a kind override → `modified`, a contradicting edge →
  409, and similarity-cache invalidation), list pagination, filters and
  counts, non-admin 403 on every endpoint, `run` enqueue dedupe, audit rows,
  and the 1000 cap on a 3,000-series stress library.
- `relationships::suggestions` unit tests: canonical forms, merge and
  corroboration, buckets, the citation parser and the base-name helpers.
- WP-7.3, same integration file: bulk accept by ids (one audit row, no
  per-item rows, partial failures `conflict` / `already_reviewed` /
  `not_found` reported while the good item commits, one similarity
  invalidation, none when nothing was created), bucket mode (`remaining`,
  medium / low untouched) and its 422 / 404 validation, bulk reject (one
  audit row), reopen (409 unless rejected, review stamp cleared, audit
  payload, engine treats it as pending), stale marking / hiding / revival
  (and reviewed rows never go stale), and 403 on every new endpoint.
- Web: `web/tests/admin/relationship-suggestions-panel.test.tsx` (filters
  as server params, row actions, reopen, evidence, accept-all-high confirm,
  multi-select bulk accept, run now),
  `web/tests/library/series-suggested-relationships.test.tsx` (perspective /
  inverse labels, chip accept / reject, admin gating) and
  `web/tests/api/relationship-suggestions.test.ts` (next-page + query-string
  helpers, accept / bulk invalidation, bulk toast summary).
- E2E: `web/tests/e2e/relationship-review.spec.ts` — opt-in (needs
  `E2E_ADMIN_EMAIL` / `E2E_ADMIN_PASSWORD` for an existing admin and at
  least one suggestion; it queues a run and skips if none appear). It never
  registers, so it can't race `reader-flow.spec.ts` for the first-user admin
  role; in docker-smoke (no creds, one-series fixture) it skips.
