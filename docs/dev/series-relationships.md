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
