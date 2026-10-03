# Series relationships

WP-7.1 of the product roadmap (spec §5.2 / Phase 7), with the taxonomy,
arc targets and scoped links of WP-7.5 (roadmap M7b). Series carry typed,
directed edges to other series — or to a story arc: sequels, narrative
prequels, publication continuations, spin-offs, tie-ins, annuals,
collected editions, reprints, translations, adaptations and a catch-all
"see also". The suggestion engine (WP-7.2) and its review UI (WP-7.3) build
on this layer. WP-7.8 adds provider links (Metron `associated`, issue
reprints) and **external targets** — relationships to provider series that
aren't in the library (see "Provider links and external targets").

## Schema

`series_relationship` (migrations `m20270501_000001_series_relationship`
and `m20270505_000001_relationship_taxonomy`, entity
`crates/entity/src/series_relationship.rs`):

| column | type | notes |
|---|---|---|
| `id` | `uuid` PK | |
| `from_series_id` | `uuid` FK → `series` ON DELETE CASCADE | the subject: "*from* is a sequel of *to*" |
| `to_series_id` | `uuid` NULL FK → `series` ON DELETE CASCADE | indexed (`series_relationship_to_series`); NULL for an arc edge |
| `to_arc_id` | `uuid` NULL FK → `story_arc` ON DELETE CASCADE | WP-7.5 arc target; indexed (`series_relationship_to_arc`, partial) |
| `kind` | `text` | CHECK in the kind list below |
| `source` | `text` default `'manual'` | CHECK `manual` / `suggested` |
| `confidence` | `real` NULL | 0.0–1.0 (CHECK); set for accepted suggestions, NULL for manual edges |
| `created_by` | `uuid` NULL FK → `users` ON DELETE SET NULL | the admin who made the edge |
| `created_at` | `timestamptz` default `now()` | |
| `from_range` | `text` NULL | issue range on the *from* side (`1-6`, `1-6,Annual 1`); ≤ 100 chars (CHECK) |
| `to_range` | `text` NULL | issue range on the *to* side; ≤ 100 chars |
| `coverage` | `text` NULL | `full` / `partial` / `unknown`; only on `collects` / `collected_in` / `reprints` / `reprinted_in` (CHECK) |
| `qualifier` | `text` NULL | continuation qualifier (`relaunch` / `retitle` / `merge` / `split` / `numbering`) on `continues` / `continued_by`, or tie-in role (`main` / `tie_in` / `prelude` / `aftermath`) on `tie_in_to` / `has_tie_in` (CHECK binds each set to its kinds) |
| `note` | `text` NULL | free text, ≤ 500 chars |

Constraints:

- `series_relationship_target_chk`: `num_nonnulls(to_series_id, to_arc_id) = 1`.
- `series_relationship_arc_kind_chk`: an arc target is only `tie_in_to`.
- Partial unique indexes (they replaced `UNIQUE (from, to, kind)`):
  `series_relationship_series_uniq (from_series_id, to_series_id, kind) WHERE to_series_id IS NOT NULL`
  and `series_relationship_arc_uniq (from_series_id, to_arc_id, kind) WHERE to_arc_id IS NOT NULL`.
- `CHECK (from_series_id <> to_series_id)`.

Deleting a series removes both halves of every pair it took part in, and
deleting a story arc removes its tie-in rows (the FK cascades).

## Kinds and inverses (WP-7.5 taxonomy)

Every series → series edge is stored **together with its inverse**, so
"what is related to X" is a single `WHERE from_series_id = X` with no
`UNION`. A self-inverse kind stores the reverse row with the same kind
(`A see_also B` + `B see_also A`). Every kind belongs to one UI group; a
pair's two halves share it.

| group | kind | inverse | label (from the subject's side) |
|---|---|---|---|
| Story | `sequel_of` | `has_sequel` | Sequel to / Has sequel |
| Story | `prequel_of` | `has_prequel` | Prequel to / Has prequel — a *narrative* prequel: written later, set earlier |
| Story | `spin_off_of` | `has_spin_off` | Spin-off of / Has spin-off |
| Story | `side_story_of` | `has_side_story` | Side story of / Has side story |
| Story | `tie_in_to` | `has_tie_in` | Tie-in to / Has tie-in (role folded in: "Prelude to", "Aftermath of", "Main story of"; "Has prelude", …) |
| Story | `crossover_with` | itself | Crossover with |
| Story | `companion_to` | itself | Companion to |
| Story | `same_universe` | itself | Same universe as (manual only; "same universe" is derived from `universe` / `series_universe` membership and `SeriesGroup`, rendered by WP-7.7 — the engine no longer suggests it) |
| Story | `see_also` | itself | See also |
| Publication history | `continues` | `continued_by` | Continues / Continued by (qualifier: relaunch, retitle, merge, split, numbering) |
| Publication history | `annual_of` | `has_annual` | Annual of / Has annual |
| Publication history | `supplement_to` | `has_supplement` | Supplement to / Has supplement |
| Editions & contents | `collects` | `collected_in` | Collects / Collected in (coverage) |
| Editions & contents | `reprints` | `reprinted_in` | Reprints / Reprinted in (coverage) |
| Editions & contents | `alternate_edition_of` | itself | Alternate edition of |
| Editions & contents | `translation_of` | `has_translation` | Translation of / Has translation |
| Advanced | `adaptation_of` | `adapted_as` | Adaptation of / Adapted as (comic-to-comic) |
| Advanced | `reimagining_of` | `reimagined_as` | Reimagining of / Reimagined as |

`prequel_of` is **no longer** the inverse of `sequel_of` (it was in WP-7.1);
see "Migration" below for how existing rows moved.

**Contradictions.** A directional kind contradicts its own inverse on the
same ordered pair: asking for `A sequel_of B` while `A has_sequel B` exists
is refused (`PairError::Conflict`, HTTP 409). The two reading-order
families count as one (`RelationshipKind::contradictions`): `A continues B`
also contradicts `A has_sequel B`, and `A sequel_of B` contradicts
`A continued_by B`.

The typed enum is `server::relationships::RelationshipKind` (serde
snake_case + `ToSchema`, with `inverse()`, `label()`, `display_label(qualifier)`,
`group()`, `qualifiers()`, `allows_coverage()`, `allows_arc_target()`,
`is_canonical()`, `as_str()`), next to `RelationshipGroup`
(`story | publication | editions | advanced`), `RelationshipQualifier`,
`RelationshipCoverage` and `RelationshipSource` (`manual | suggested`). The
DB CHECK mirrors the enum: adding a kind needs both a migration and the enum
variant.

The **catalogue** is served at `GET /api/relationship-kinds`
(`RelationshipCatalogue { groups: [{group, label}], kinds: [{kind, inverse,
label, inverse_label, group, symmetric, qualifiers: [{value, label}],
allows_coverage, allows_arc_target}] }`), so the web never hard-codes kinds.

## Scope (WP-7.5)

Optional on any series edge, read from the *from* side:

- `from_range` / `to_range`: issue-number ranges (`1-6`, `1-6,Annual 1`).
  Validated lightly: ≤ 100 chars, no control characters; blank → NULL.
- `coverage`: `full | partial | unknown`, only for `collects` /
  `collected_in` / `reprints` / `reprinted_in`. Any other kind → 422.
- `qualifier`: `continues` / `continued_by` take a continuation qualifier;
  `tie_in_to` / `has_tie_in` take a role. Any other kind, or a qualifier
  from the wrong set → 422.
- `note`: ≤ 500 chars, any kind.

The inverse row **mirrors** the scope: ranges swapped, the same coverage,
qualifier and note (`Scope::mirrored`). `relationships::Scope::validate`
returns field-level issues (`[{field, message}]`), which the API returns as
the canonical 422 `error.details`.

## Arc targets (WP-7.5)

A relationship can target a story arc instead of a series: "this series is
a tie-in to *Secret Invasion*". Arc edges are **one-directional** rows (no
inverse row — an arc isn't a series), only `tie_in_to` (with an optional
role), and never part of the reading-order chain or the similar-series
signal (both join on `to_series_id`).

- `relationships::create_arc_edge(conn, from, arc_id, kind, source,
  confidence, created_by, &scope)` (idempotent like `create_pair`).
- `relationships::arc_edges(conn, series_id)`: a series' arc edges.
- `relationships::arc_tie_ins(conn, arc_id)`: every series tying in to an
  arc (unfiltered; the HTTP route applies the ACL).

**Visibility** follows the M5 arc entity pages: an arc is visible when the
caller can see at least one appearance of it — a visible issue in
`issue_arcs` or a visible series in `series_arcs` (library grant +
age-rating cap; `entity_pages::visible_arc_ids`). `GET
/api/series/{slug}/relationships` lists only visible arcs, and `GET
/api/arcs/{slug}/tie-ins` answers 404 for an arc the caller can't see
(`entity_pages::resolve_visible_for_user`, the same gate as
`/arcs/{slug}`).

## Write surface

`crates/server/src/relationships/mod.rs` is the **only** writer. Never
insert or delete `series_relationship` rows directly.

```rust
pub async fn create_pair<C>(conn, from, to, kind, source, confidence, created_by)
    -> Result<PairOutcome, PairError>;           // no scope
pub async fn create_pair_scoped<C>(conn, from, to, kind, source, confidence, created_by, &Scope)
    -> Result<PairOutcome, PairError>;           // PairOutcome { forward, inverse, created }
pub async fn create_arc_edge<C>(conn, from, arc_id, kind, source, confidence, created_by, &Scope)
    -> Result<ArcEdgeOutcome, PairError>;        // { row, created }
pub async fn update_edge<C>(conn, forward: Model, kind, Scope)
    -> Result<UpdateOutcome, PairError>;         // { before, forward, inverse, kind_changed }
pub async fn delete_pair<C>(conn, from, to, kind) -> Result<Option<Model>, DbErr>;
pub async fn delete_pair_by_id<C>(conn, id) -> Result<Option<Model>, DbErr>;  // either half, or an arc edge
pub async fn row_from_perspective<C>(conn, row, series_id) -> Result<Option<Model>, DbErr>;
// PairError { SelfEdge, Conflict { existing }, Duplicate { kind }, InvalidConfidence,
//             ArcKind { kind }, InvalidScope(Vec<ScopeIssue>), Db }
```

- Pass a `DatabaseTransaction` so both halves land, or vanish, together.
- `create_pair*` is **idempotent**: an existing pair comes back with
  `created = false` and its scope **unchanged** (edit scope with
  `update_edge`); a missing inverse half is re-created. Both inserts are
  `ON CONFLICT … WHERE to_series_id IS NOT NULL DO NOTHING`, so concurrent
  creates are safe. WP-7.2's suggestion-accept path calls `create_pair`
  with `RelationshipSource::Suggested` and the suggestion's confidence.
- `update_edge` keeps the halves in sync: a scope edit updates both rows in
  place (the inverse gets the mirrored scope); a **kind change is delete +
  create** of the pair (new ids; source, confidence and creator kept), so
  the contradiction rule applies to the new kind, and turning the edge into
  a pair that already exists is `PairError::Duplicate` (409). Arc edges
  stay `tie_in_to` and are updated in place. Run it in a transaction so a
  refused create rolls the delete back.
- Neither checks that the series exist (the FK does) or that the caller may
  see them (the HTTP layer does).

## Traversal

```rust
pub const MAX_TRAVERSAL_DEPTH: u32 = 6;
pub const CHAIN_BEFORE: [RelationshipKind; 2] = [SequelOf, Continues];
pub const CHAIN_AFTER:  [RelationshipKind; 2] = [HasSequel, ContinuedBy];
pub async fn traverse<C>(conn, start, kinds: &[RelationshipKind], max_depth: u32)
    -> Result<Vec<TraversalNode>, DbErr>;      // { series_id, depth, parent_id }
pub async fn chain<C>(conn, start) -> Result<Vec<ChainNode>, DbErr>;
                                               // { series_id, position, parent_id }
pub async fn direct<C>(conn, series_id) -> Result<Vec<series_relationship::Model>, DbErr>;
```

`traverse` is a raw-SQL recursive CTE (one of the spec's sanctioned raw-SQL
escape hatches). It follows series edges whose kind is in `kinds` (arc
edges are skipped), clamps depth to 6, and is **cycle-safe**: each
recursive row carries its visited path and never re-enters a node on it.
Each node is reported once, at its shortest depth, with the `parent_id` it
was reached through. The start series is never returned.

`chain` builds the reading order (WP-7.5): it walks narrative sequels and
publication continuity together — `sequel_of` / `continues` for what comes
before (negative positions) and `has_sequel` / `continued_by` for what
comes after (positive positions), mixed freely along a path, up to 6 hops
each way, with the start series at position 0. A narrative `prequel_of`
is **not** a chain edge (it is shown in its own group). Positions can
repeat when the chain branches (two sequels of the same book). A node that
a cycle puts on both sides is kept on the "before" side only.

Dense self-inverse graphs (`same_universe` across a whole publisher) make
path enumeration expensive at depth 6. The chain only follows
sequel/continuation edges, so this doesn't affect the series page. A
caller that traverses `same_universe` should pass a small `max_depth`.

## HTTP API (`api` group; `crates/server/src/api/series_relationships.rs`)

| method | path | who | result |
|---|---|---|---|
| `GET` | `/api/relationship-kinds` | any signed-in user | `RelationshipCatalogue` |
| `GET` | `/api/series/{slug}/relationships` | any user who can see the series | `SeriesRelationshipsResp` |
| `POST` | `/api/series/{slug}/relationships` | `RequireAdmin` | `201` new / `200` existing → `SeriesRelationshipView` (or `SeriesArcRelationshipView` for an arc target) |
| `PATCH` | `/api/series/{slug}/relationships/{id}` | `RequireAdmin` | `200` → the updated edge from `{slug}`'s side |
| `DELETE` | `/api/series/{slug}/relationships/{id}` | `RequireAdmin` | `204` |
| `GET` | `/api/arcs/{slug}/tie-ins` | any user who can see the arc | `CursorPage<ArcTieInView>` (`cursor`, `limit` 1–100, default 60; `total` on the first page), ordered by role — prelude, main story, tie-in (or unset), aftermath — then oldest first (WP-7.7) |
| `POST` | `/api/series/{slug}/external-relationships` | `RequireAdmin` | WP-7.8: `201` `CreateExternalRelationshipResp` (`external` set, or `relationship` when the provider series is already local) / `200` existing |
| `DELETE` | `/api/series/{slug}/external-relationships/{id}` | `RequireAdmin` | WP-7.8: `204` (a user link is deleted, a provider link dismissed) |
| `GET` | `/api/series/{slug}/same-universe` | any user who can see the series | `CursorPage<SameUniverseItem>` (`cursor`, `limit` 1–60, default 24; `total` on the first page) — derived, see "Same universe" below (WP-7.7; `crates/server/src/api/series_same_universe.rs`) |

`GET /api/series/{slug}` also carries `relationship_count` (WP-7.7): the
direct series edges plus arc edges the caller can see (the same filters as
the relationships `GET`) plus, since WP-7.8, the listed external links, so the series page's Related tab label shows a
count without loading the relationships. Detail only; list payloads omit
it.

```jsonc
// GET /api/series/{slug}/relationships
{
  "series_id": "…",
  "relationships": [{
    "id": "…",                 // row id; pass to PATCH / DELETE
    "kind": "continues",       // from this series' point of view
    "kind_label": "Continues", // tie-in roles folded in ("Prelude to")
    "group": "publication",
    "source": "manual",        // | "suggested"
    "confidence": null,        // 0–1 for suggested
    "created_at": "…",
    "from_range": null, "to_range": "1-6",
    "coverage": null,          // collects / reprints only
    "qualifier": "relaunch", "qualifier_label": "Relaunch",
    "note": null,
    "series": { /* SeriesView, hydrated like a grid card */ }
  }],
  "arcs": [{                   // series → arc edges (visible arcs only)
    "id": "…", "kind": "tie_in_to", "kind_label": "Prelude to", "group": "story",
    "qualifier": "prelude", "qualifier_label": "Prelude", "from_range": null, "to_range": null, "note": null,
    "source": "manual", "confidence": null, "created_at": "…",
    "arc": { "id": "…", "slug": "secret-invasion", "name": "Secret Invasion" }
  }],
  "chain": [                   // empty when there are no sequel/continuation edges
    { "position": -1, "series": { /* SeriesView */ } },
    { "position": 0,  "series": { /* this series */ } },
    { "position": 1,  "series": { /* SeriesView */ } }
  ],
  "external": [{               // WP-7.8: provider series not in the library
    "id": "…", "kind": "continued_by", "kind_label": "Continued by", "group": "publication",
    "qualifier": null, "qualifier_label": null,
    "source": "metron", "source_label": "Metron", "provider_series_id": "2311",
    "name": "Saga", "year": 2018, "url": "https://metron.cloud/series/2311/",
    "set_by": "provider",      // | "user"
    "confidence": 0.6, "created_at": "…",
    "local_series": null       // { id, slug, name, year } for a user link matched locally, not yet promoted
  }]
}

// POST body: "this series <kind> target". Exactly one of target / target_arc.
{ "target": "saga-2012" /* slug or UUID */, "kind": "collects",
  "from_range": "1", "to_range": "1-6", "coverage": "full", "note": "Vol. 1" }
{ "target_arc": "secret-invasion" /* slug or UUID */, "kind": "tie_in_to", "qualifier": "prelude" }

// PATCH body: every field optional; omit = keep, null = clear (scope fields).
{ "kind": "reprints", "note": null }
```

Errors: `422 validation` (with field `details`) for a self edge, a missing
or doubled target, scope that doesn't fit the kind (`coverage`,
`qualifier`), an over-long range / note, an arc target with a non-arc kind
(`kind`), or a malformed body; `404` when the series, target or arc is
missing; `409 conflict` for a contradicting directional kind, or a PATCH
kind change that would duplicate an existing pair; `403` for non-admin
writes; `400` for a malformed id. A duplicate POST is **not** an error: it
returns the existing forward row with `200`, unchanged, and writes no audit
row.

`PATCH` and `DELETE` take either half's id (it must touch `{slug}`); PATCH
reads the body from `{slug}`'s side (an inverse-half id is swapped for its
partner), DELETE removes both halves. A PATCH kind change answers with the
**new** row id.

**Permissions and ACL.** `GET` answers 404 when the caller can't see the
series. Related series are filtered to those the caller can see too:
library grant and age-rating cap (`VisibleLibraries::series_ok`), and
non-admins never see removed series. Arcs are filtered as described under
"Arc targets". The chain is pruned so that a hidden link also hides every
series reached through it. Nothing beyond a series the caller can't see
leaks. `GET /arcs/{slug}/tie-ins` filters the tying-in series in SQL with
the entity pages' series ACL (`entity_pages::series_visible_sql_for`), so
pages are never short; admins also see removed series there (WP-7.7,
consistent with `series::list`), non-admins don't.

**Audit.** `admin.series.relationship.create` (only when a pair or arc edge
was actually inserted), `admin.series.relationship.update` (payload: new
and previous row ids, `kind_before` / `kind`, `kind_changed`,
`scope_before` / `scope`) and `admin.series.relationship.delete`, all with
target `("series", <slug's series id>)`. Every write calls
`state.similarity.invalidate_all()`.

**Pagination.** The series list isn't cursor-paginated. Relationships are
curated (admin-made or accepted suggestions), so the set is bounded by the
domain, like `/me/sessions`. The arc tie-in list is cursor-paginated
because WP-7.6 will detect tie-ins from `issue_arcs` and big events can
have many.

## OPDS

Series feeds link related series the caller can see:

- OPDS 1.2 `/opds/v1/series/{id}`: a feed-level
  `<link rel="related" href="/opds/v1/series/{other}" type="…acquisition" title="Sequel to: Saga (2012)"/>`
  for each one.
- OPDS 2.0 `/opds/v2/series/{id}`: a `links[]` entry
  `{ "rel": "related", "href": "/opds/v2/series/{other}", "type": "application/opds+json", "title": …, "properties": { "folio:relationship": "sequel_of" } }`.

Both use `api::series_relationships::visible_related` (a `RelatedLink {
kind, label, series }` list, the label being `display_label` with any
tie-in role folded in), which applies the same ACL as the JSON `GET`.

Arc edges (WP-7.7) link to the arc's acquisition feed through
`visible_related_arcs` (arcs filtered by the arc visibility rule):

- OPDS 1.2: `<link rel="related" href="/opds/v1/arcs/{slug}" type="…acquisition" title="Prelude to: Secret Wars"/>`.
- OPDS 2.0: the same href with `"type": "application/atom+xml;profile=opds-catalog;kind=acquisition"`
  and `"properties": { "folio:relationship": "tie_in_to" }`. Story arcs have
  no OPDS 2.0 feed yet (M5 backlog), so the v2 link points at the 1.x
  feed and says so in `type`.

## Migration (`m20270505_000001_relationship_taxonomy`)

**Up**, in `series_relationship`:

1. Every `prequel_of` row becomes `has_sequel`: under the WP-7.1 model
   `prequel_of` was only ever the inverse half of `sequel_of`.
2. Every `source = 'suggested'` `sequel_of` / `has_sequel` pair whose
   suggestion (same `from` / `to`, `status = 'accepted'`, kind `sequel_of`)
   has a `name_continuation` or `provider_volume` entry in
   `evidence.sources` becomes `continues` / `continued_by` — those sources
   detect publication continuity, not narrative order.
3. Manual `sequel_of` stays (the admin chose it), and so does a suggested
   `sequel_of` that came from a **modified** accept (the admin picked
   `sequel_of` over the suggested kind).

In `series_relationship_suggestion`: `pending` / `stale` / `accepted`
`sequel_of` rows from those two sources become `continues` (accepted rows
too, so the "never re-suggest" rejection memory keeps matching the new
kind); `accepted_kind = 'prequel_of'` becomes `has_sequel` (same meaning);
rejected and modified rows keep their kind (the engine's dedupe treats
`sequel_of` ≡ `continues`, below). The kind / canonical / accepted-kind
CHECKs are replaced with the new lists.

**Down is lossy** but keeps working (CI's migration round-trip runs it):
arc-target rows are deleted and the scope columns dropped;
`has_sequel` / `continued_by` → `prequel_of`, `continues` / `has_prequel` →
`sequel_of`, every other new kind → `see_also`, keeping one row where two
now collide on the restored `UNIQUE (from, to, kind)` (a row already of the
old kind wins, then the oldest). Suggestions: `continues` → `sequel_of`
(dropped when a `sequel_of` row for the pair exists), rows of any other new
kind are deleted, `accepted_kind` is mapped like the edges.

Tested by `crates/server/tests/migration_relationship_taxonomy.rs` (down →
seed old-shape rows → up → assert → new-only rows → down → assert → up).

## Suggestion engine (WP-7.2)

An apalis job proposes relationships from evidence already in the DB. It
**never creates edges**: it writes candidate rows that an admin accepts
(which calls `create_pair_scoped`, or `create_arc_edge` for an arc target,
with `source = suggested` and the proposed scope) or rejects. WP-7.6 added
the detectors (annual, arc tie-in, reprint roll-up, alternate edition,
facsimile, supplement, translation, continuation qualifiers), scope and arc
targets on suggestions, and retired the pairwise `same_universe` and
arc-crossover sources.

Code: `crates/server/src/relationships/suggestions/` (`mod.rs` = merge,
upsert and review service; `sources.rs` = one set-based query per SQL
evidence source; `detectors.rs` = the name-based detectors over one shared
series catalogue; `citations.rs` = the "Collects X #1-6" parser), the job in
`crates/server/src/jobs/relationship_suggest.rs`, and the API in
`crates/server/src/api/relationship_suggestions.rs`.

### Schema

`series_relationship_suggestion` (migrations
`m20270502_000001_relationship_suggestion` and, for arc targets and scope,
`m20270506_000001_relationship_suggestion_scope` (WP-7.6); entity
`entity::series_relationship_suggestion`):

| column | type | notes |
|---|---|---|
| `id` | `uuid` PK | |
| `from_series_id` | `uuid` FK → `series` ON DELETE CASCADE | "*from* `kind` *to*" |
| `to_series_id` | `uuid` NULL FK → `series` ON DELETE CASCADE | NULL for an arc target |
| `to_arc_id` | `uuid` NULL FK → `story_arc` ON DELETE CASCADE | WP-7.6 arc target (`tie_in_to` only) |
| `from_range`, `to_range` | `text` NULL | proposed issue ranges (≤ 100 chars), read from *from*'s side |
| `coverage` | `text` NULL | `full` / `partial` / `unknown`; collects / reprints only |
| `qualifier` | `text` NULL | continuation qualifier (`continues`) or tie-in role (`tie_in_to`) |
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

Exactly one target (`series_relationship_suggestion_target_chk`), an arc
target only for `tie_in_to` (`…_arc_kind_chk`), and the same coverage /
qualifier / range CHECKs as `series_relationship`. Uniqueness is two
partial unique indexes (WP-7.6 replaced `UNIQUE (from, to, kind)`):
`(from_series_id, to_series_id, kind) WHERE to_series_id IS NOT NULL` and
`(from_series_id, to_arc_id, kind) WHERE to_arc_id IS NOT NULL`. The
migration's down is lossy (arc rows deleted, scope dropped). **Canonical form**, enforced
by CHECKs (rewritten by WP-7.5): self-inverse kinds (`crossover_with`,
`companion_to`, `same_universe`, `see_also`, `alternate_edition_of`) are
stored with `from < to`, so A→B and B→A are one row; directional kinds are
stored in one direction only — `sequel_of`, `prequel_of`, `spin_off_of`,
`side_story_of`, `tie_in_to`, `continues`, `annual_of`, `supplement_to`,
`collects`, `reprints`, `translation_of`, `adaptation_of`,
`reimagining_of` (`RelationshipKind::is_canonical`), never their inverses.
`canonicalize()` folds every candidate onto that form before the upsert.

Rows are **never deleted** (spec §5.7); only the series FK cascade removes
them. Only the status moves: a review (`accepted` / `rejected` /
`modified`) leaves `pending` once; the one way back is an admin
**reopening** a rejection (WP-7.3). `stale` is the engine's own state, not
a review (see "Review UI" below).

### Evidence sources and confidence

A run is per library and starts from that library's series. Story and
publication kinds (sequels, continuations, annuals, supplements, tie-ins,
crossovers, see-also) only link series **in the same library**: libraries
are usually split by publisher or format, and cross-library continuations
are rare. The **edition kinds** — `collects`, `reprints`,
`alternate_edition_of`, `translation_of` (`RelationshipKind::may_cross_libraries`)
— may pair two libraries (WP-8.2), because many users keep trades or
translations in a library of their own; see "Cross-library edition
suggestions" below. Removed series are ignored. Pair-producing SQL sources use a star, adjacency (`lag()` over an
ordered partition) or per-pair `GROUP BY` shape, never all-pairs, and each
query caps its output (5000 rows; 20000 for the arc rows). The name-based
detectors (`detectors.rs`) share **one** catalogue query (every live series
with its issue aggregates, read through per-series index lookups) and match
through hash indexes on the normalized base name (`match_key`: base name
minus a leading "the"), never pairwise.

| source (`evidence.sources[].source`) | kind → target | scope | confidence |
|---|---|---|---|
| **Name continuation** (`name_continuation`): series grouped by `(base name, publisher, language)`, where the base name is `normalized_name` minus a trailing `vol N` / `vN` / year (1930–2049) token, or the parent folder's name when the series folder is just `Vol N`; each series pairs with its predecessor by `(year, volume)` | `continues` (later → earlier) | `qualifier` (below) | consecutive volumes 0.9 · later volume with a gap 0.65 · later year, no volumes 0.7 · year and volume order disagree 0.45 · same name and same year → `see_also` 0.35 |
| **AlternateSeries** (`alternate_series`): ComicInfo `AlternateSeries` on issues of A, split on `,`/`;`, matched to a series by normalized name (closest year to the citing issues wins; A's own title is ignored) | `crossover_with` | — | plain value 0.75 · ComicVine reading-list style `"Avengers" Civil War` 0.5 · +0.05 for ≥ 3 issues · −0.05 when several series share the name · −0.15 when the year gap is > 3 |
| **Arc tie-in** (`arc_tie_in`, WP-7.6): each story arc in `issue_arcs` spanning ≥ 2 series; every participating series → the **arc** | `tie_in_to` → arc | `qualifier` = role, `from_range` = its issues in the arc | by how unambiguous the main series is: main by the WP-8.2 rule (below) — named like the arc (or its reading-list family, `"Secret Wars" Battleworld`) → main 0.9, tie-ins 0.8 · main holds ≥ 2× the runner-up's issues → 0.75 / 0.7 · two co-mains (runner-up ≥ 75% of the top, ≥ 2 issues, third ≤ half) → 0.6 / 0.55 · otherwise most issues → 0.55 / 0.55 · a role inferred from dates only −0.05 when main is clear |
| **Arc crossover** (`arc_crossover`, WP-7.6): the two co-main series of one arc (a genuine two-title crossover) | `crossover_with` | — | 0.65 |
| **Collected edition** (`collected_edition`): issues whose `Format` / `special_type` / series type / series name marks a TPB, HC, omnibus or graphic novel; their title, notes and a "Collects…"/"Reprints…" summary are parsed for `Name #lo-hi` (or `issues lo-hi`) citations; an unnamed citation means the edition's own title minus format words. WP-7.6 rolls citations up **per (edition, target) pair** | `collects` (edition → collected series) | `to_range` = the cited ranges merged ("1-6,9-10"), `from_range` = the citing issues, `coverage` = `full` when the merged ranges leave no gap, else `partial` | issue coverage in the library ≥ 80% 0.85 · ≥ 50% 0.7 · some 0.55 · none 0.4 · −0.1 for an unnamed citation · −0.1 when the name is ambiguous and nothing is covered (the pair takes its best citation) |
| **Reprint roll-up** (`reprint_rollup`, WP-7.6): `issue_reprints` rows (issue → reprinted issue, both in the library, different series) grouped per series pair. Label-only rows are skipped | `collects` when the reprinting series is a collected edition (format / `special_type` / series type / name marker), else `reprints` | `to_range` = reprinted numbers compacted ("1-6,9"), `from_range` = the reprinting issues, `coverage` = `full` when every target issue the library holds inside the reprinted span is linked, `partial` otherwise, `unknown` without numbers | `collects` 0.85 (0.9 with ≥ 3 issues) · `reprints` 0.65 (0.7 with ≥ 2) |
| **Shared provider volume** (`provider_volume`): local series of the same language whose first issue's ComicInfo `comicvine_series_id` / `metron_series_id`, or whose series-level `external_ids`, name the same provider series (2–12 claimants) | `continues` when issue ranges are disjoint and ordered, else `see_also` (overlapping numbers: probably duplicate copies) | `qualifier` (below) | 0.8 · `see_also` 0.6 (0.55 without issue numbers) |
| **Provider associated** (`provider_associated`, WP-7.8): a live provider row of `series_external_relationship` (Metron series `associated`) on A whose provider series resolves to local series B (`external_ids`, or the cached id bridge — see "Provider links and external targets") | the local series types / names refine Metron's untyped link (`relationships::external::provider_kind`): a collected edition (TPB / HC / GN / omnibus) vs a periodical (ongoing / limited / one-shot) → `collects` (edition → singles); an annual vs a periodical → `annual_of`; else `see_also`. When the local rows say nothing, the kind recorded at apply time from the provider's own types is used | — | `see_also` 0.6 · refined 0.72 (0.66 when the periodical side is only unknown, not explicitly a periodical). A `see_also` from this source is dropped when another source already gives the pair a specific kind; annual pairs drop it like the continuation sources |
| **Provider range** (`provider_range`): a `series_provider_range` row on A pointing at provider series P while another local series B is matched to P | `see_also` | — | 0.7. Not `continues`: the range sits inside A, and B usually duplicates those issues |
| **Annual** (`annual`, WP-7.6): a series named "X Annual" / "X Annuals" (before a year / volume), with Metron series type "Annual Series" (stored in `series.series_type`), or whose issues are ≥ 80% `Format` / `special_type` Annual → main series X by base name, same publisher (unknown allowed), overlapping or adjacent years; the volume whose years contain the annual's wins. WP-8.2 noise rule: the shared title must say more than the publisher's name | `annual_of` | — | name signal 0.9, type / format signal 0.8 · −0.15 when only adjacent / overlapping · −0.35 when years are unknown · −0.15 when several volumes fit equally · −0.1 when a publisher is unknown. The name-continuation / provider `continues` / `see_also` candidate on the same pair is dropped (an annual isn't the next volume) |
| **Alternate edition** (`alternate_edition`, WP-7.6): same base title once an edition marker is stripped — Deluxe (Edition), Director's Cut, Remastered, Colo(u)rized, Artist's / Gallery / Special / Treasury Edition, Absolute, Unlimited — and overlapping years or issue range inside the original's; the edition can't predate the original. WP-8.2: an "Unlimited" match needs **both** overlaps (it is usually an anthology with its own numbering), and the noise rule applies. May cross libraries | `alternate_edition_of` (self-inverse) | — | 0.6 · "Unlimited" 0.4 · +0.05 with both overlaps · −0.05 several candidates · −0.05 unknown publisher. "Absolute" counts only for a collected edition ("Absolute Carnage" is an event); a collected edition of a singles run is left to the collects sources |
| **Facsimile** (`facsimile`, WP-7.6): "X #N Facsimile Edition" (number from the name, else the facsimile's first issue) — not an alternate edition | `reprints` | `to_range` = N, `coverage` = `full` | 0.75 when the target's issue range holds N, else 0.6 · −0.1 several candidates |
| **Supplement** (`supplement`, WP-7.6): Handbook / Guidebook / Sourcebook anywhere in the name, or a name ending in Guide / Saga / Spotlight / Special (occasion words like Wedding / Holiday / Halloween dropped), whose remaining title names a series of the same publisher; the parent volume running at the time wins. WP-8.2 noise rule: "Marvel Holiday Special" → "Marvel" is dropped | `supplement_to` | — | handbook / guidebook / sourcebook 0.6 · guide / saga 0.55 · spotlight 0.5 · special 0.45 · −0.05 several candidates · −0.05 unknown publisher |
| **Translation** (`translation`, WP-7.6): the same work in another `series.language_code` (folded to ISO 639-1: `eng`/`EN` → `en`) | `translation_of` (later → original, the earlier series) | — | same provider series (issue ComicInfo ids / series `external_ids`) 0.7 · one lists the other's title in `aliases` / `alternate_names` 0.6 (the carrier is the translation unless the years say otherwise) · same base title + shared writers / pencillers 0.55 (0.6 with ≥ 2). Never high |

**Cross-library edition suggestions** (WP-8.2). The edition sources look
up targets across libraries:

- collected edition: this library's editions resolve their citations
  against every library (a target in the edition's own library wins a
  coverage tie);
- reprint roll-up: this library's reprinting issues, the reprinted issue in
  any library;
- provider associated: provider rows of every library are resolved, a pair
  is kept when either end is in this library (a cross-library pair of any
  non-edition kind is dropped);
- alternate edition, facsimile, translation: the detectors run over this
  library's catalogue **plus** the other libraries' series that could pair
  with one of them — a light name-only scan of the other libraries picks
  the series whose lookup keys (`match_key`, the title minus an edition
  marker, a facsimile's title with / without its number), normalized name
  or aliases meet this library's, and only those get the issue aggregates
  (`Catalogue::with_other_libraries`). Translation provider claims join
  other libraries' series through their series-level `external_ids`.
  Annuals and supplements stay within the library.

**Ownership.** A row belongs to the run of its canonical `from` series'
library — the same key rejection memory, the existing-edge dedupe and stale
marking already use. A run drops proposals whose `from` is in another
library (`RunReport.skipped_other_library`; that library's run writes
them) and counts the kept cross-library ones (`cross_library`). Because
the detectors see a pair the same way from either library, the owning run
always produces it, so two libraries' runs never flip a row between
pending and stale. The per-run cap, dedupe and rejection memory apply
unchanged.

**Noise rule** (WP-8.2, `detectors::specific_title`). An annual, alternate
edition or supplement match must share more than the publisher's own name:
the shared title's tokens minus *publisher-generic* tokens (both series'
publisher and imprint names, plus `the a an of and comics comic publishing
publications press studios entertainment books group inc none`) must not be
empty. On the dev library this dropped "Marvel Holiday Special → Marvel";
"X-Men Unlimited → X-Men (2004)" went with the stricter "Unlimited" rule
(the shared title "X-Men" is not publisher-generic).

**Arc main-series rule** (WP-8.2). (1) A series whose name matches the
arc's name exactly (`match_key`: base name, no leading "the") is main; failing
that, one named like the arc's reading-list family (the quoted part of
`"Secret Wars" Battleworld`) — the exact arc name wins over the family even
with fewer issues, and among same-named volumes the one with more issues in
the arc. The unquoted tail alone ("Battleworld") no longer names a main.
(2) Otherwise the series with the most issues in the arc is main (with the
existing margin / co-main / weak confidence split). On the dev library the
`"Secret Wars" Battleworld` arc keeps "Secret Wars: Battleworld (2015)" as
main by rule (1): Secret Wars (2015) has no issues tagged with that arc.

**Continuation qualifier** (WP-7.6) on `continues` from name continuation
and provider volumes, in this order; null when the evidence doesn't say:

- `retitle`: provider continuity under different base names;
- `split`: either series has a `series_provider_range` row (a provider
  splits that run across provider series);
- `numbering`: the later series' first number is > 1 and above the earlier
  one's last (legacy numbering continues);
- `relaunch`: the later series starts at #1 (or #0) and the earlier one's
  last issue year is not after the new start.

`merge` is never inferred.

**Retired in WP-7.6** (their pending rows go `stale` on the next run through
the normal stale mechanism; accepted edges and reviewed rows stay):

- `series_group` — the SeriesGroup star → `same_universe`;
- `character_density` — publisher + uncommon character/team overlap →
  `same_universe`;
- `story_arc` — the pairwise shared-arc → `crossover_with` star (replaced by
  the arc tie-in detector; `crossover_with` now comes only from
  AlternateSeries and from two co-main series of one arc).

"Same universe" is now derived from `universe` / `series_universe`
membership and `SeriesGroup`, not suggested pairwise (WP-7.7 renders it).

**Merging.** Candidates landing on the same canonical row merge: confidence
is the strongest source's plus 0.05 for each additional distinct source
(max 0.99), reasons are joined strongest first, and `evidence.sources`
keeps one entry per source. **Scope** merges field by field from the
strongest source that set it; a candidate folded onto the canonical
direction (e.g. `collected_in` → `collects`) has its ranges mirrored, and
fields the kind doesn't take (or over-long ranges) are dropped.

### Dedupe, rejection memory, cap

Per run, after merging:

0. (WP-8.2) Drop a proposal whose canonical `from` is in another library,
   or a non-edition proposal whose two ends are in different libraries.
1. Drop a proposal whose `(from, to)` (series or arc target) already has an edge with the same kind
   **or its inverse** (the inverse would 409 on accept). Because edges are
   stored as pairs, one lookup covers both orientations. WP-7.5:
   `sequel_of` and `continues` are **equivalent** here
   (`suggestions::equivalent_kinds`) — an existing `sequel_of` (or
   `has_sequel`) edge satisfies a `continues` proposal for the same ordered
   pair and vice versa, so a pair linked by hand as a narrative sequel is
   never re-proposed as a continuation.
2. Drop a proposal whose row (or an equivalent-kind row: a pre-WP-7.5
   rejected `sequel_of` covers `continues`) is already `accepted` /
   `rejected` / `modified`. A rejected suggestion never reappears; re-suggesting needs
   the rejection cleared by hand (spec §5.7) — the WP-7.3 **reopen**
   action. `stale` rows are not rejection memory.
3. Keep the top **1000** by confidence (`MAX_SUGGESTIONS_PER_RUN`; ties by
   ids, so it's deterministic). The rest count as `capped` in the report
   and come back on a later run once reviews free up room.
4. Upsert (one statement per target type, each on its partial unique
   index): new rows insert as `pending`; a still-pending row gets its
   confidence, bucket, reason, evidence and scope refreshed when they changed, and
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
- **Runtime** (WP-7.6, debug build): the stress test (6,000 series:
  1,500 two-volume titles plus 1,500 annuals, all in one 4,500-series arc;
  7,500 proposals, 1,000 written) runs in about 0.3–0.4 s, cold. The real
  dev library (2,573 series) takes about 1 s. Per-series aggregates use
  `CROSS JOIN LATERAL` index lookups (`issues_series_sortnum_idx`) rather
  than a join on a grouped subquery, which planned badly on freshly
  inserted, un-analyzed data (12 s).
  At 50,000 issues / 2,500 series (WP-8.3, `docs/dev/load-testing.md`
  "M7 surfaces"), a full run with 1,690 proposals takes about 0.5–0.6 s
  after the collected-edition source stopped evaluating its series regex
  once per issue (it was about 0.9 s).

### Service API

```rust
// crate::relationships::suggestions
pub async fn generate_for_library<C: ConnectionTrait>(conn: &C, library_id: Uuid)
    -> Result<RunReport, DbErr>;
pub async fn accept<C: ConnectionTrait + TransactionTrait>(
    conn: &C, id: Uuid, actor: Uuid, kind_override: Option<RelationshipKind>,
) -> Result<AcceptOutcome, ReviewError>;
// AcceptOutcome { suggestion, kind, forward, inverse: Option<Model> /* None for an arc edge */, created }
pub fn scope_of(row: &Model) -> Scope;     // the proposed scope (WP-7.6)
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
pub async fn bulk_accept<C: ConnectionTrait + TransactionTrait>(
    conn: &C, ids: &[Uuid], actor: Uuid, kind: Option<RelationshipKind> /* WP-8.2 */)
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
  `from → to` direction. WP-7.6: the proposed scope goes to the edge
  (`create_pair_scoped`, the inverse half mirrored; or `create_arc_edge`
  for an arc target). Scope fields the overriding kind doesn't take are
  **dropped** (`Scope::fitted`), not an error; an arc suggestion accepted
  as a kind that can't target an arc is `PairError::ArcKind` (422). Bulk
  accept goes through the same `accept`.
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
| `POST` | `/api/admin/relationship-suggestions/{id}/reject` | none — a pending **or stale** row (WP-8.2) | `RelationshipSuggestionView` |
| `POST` | `/api/admin/relationship-suggestions/{id}/reopen` | none | `ReopenRelationshipSuggestionResp` `{ suggestion }` (WP-7.3) |
| `POST` | `/api/admin/relationship-suggestions/bulk-accept` | `{ "ids": [...], "kind"? }` (1–500; WP-8.2 `kind` accepts the whole selection as that kind) **or** `{ "bucket": "high", "library_id"? }` | `BulkReviewRelationshipSuggestionsResp` (WP-7.3) |
| `POST` | `/api/admin/relationship-suggestions/bulk-reject` | `{ "ids": [...] }` (1–500) | `BulkReviewRelationshipSuggestionsResp` (WP-7.3) |
| `POST` | `/api/admin/relationship-suggestions/run` | `?library_id=` | `202` `{ "enqueued": [...], "already_queued": [...] }` |

```jsonc
// RelationshipSuggestionListView
{
  "items": [{
    "id": "…",
    "from_series": { /* SeriesView (cover_url, slug, …) */ },
    "to_series":   { /* SeriesView */ },       // null for an arc target (WP-7.6)
    "to_arc": null,                            // { id, slug, name } for an arc tie-in
    "kind": "continues", "kind_label": "Continues",   // tie-in role folded in ("Prelude to")
    "from_range": null, "to_range": null,      // WP-7.6 proposed scope
    "coverage": null, "qualifier": "relaunch", "qualifier_label": "Relaunch",
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
  "inverse_id": "…" /* null for an arc edge */, "kind": "sequel_of", "created": true }
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
  Library (select; WP-8.2: matches a suggestion when **either** end is in
  the library, since edition suggestions may pair two libraries), status
  (Pending / Accepted / Modified / Rejected / Stale / All) and confidence
  bucket (High / Medium / Low, counts from the
  first page's `bucket_counts`) are **server params** — nothing is filtered
  client-side.
- Each row: both covers and names (linked), the kind label, confidence %,
  bucket, status, the reason, and a collapsible **Evidence** list (one entry
  per source with its fields).
- Pending rows: **Accept**, **Edit kind** (popover with the grouped
  catalogue picker — Story / Publication history / Editions & contents /
  Advanced — read "*from* is … *to*"; a different kind accepts as
  `modified`) and **Reject**. Stale rows: **Reject** (WP-8.2). Rejected
  rows: **Reopen**. Reviewed rows show the review date.
- **Run now** queues the engine for the selected library (or every library).
- **Accept all high-confidence (N)** sits behind an `AlertDialog` and sends
  bucket mode for the current library filter.
- **Select…** enters multi-select (`useSelection` + `SelectionToolbar`) on
  pending rows: bulk **Accept**, **Accept as…** (WP-8.2: a dialog around
  the shared `RelationshipKindSelect`, opening on the first selected row's
  kind; it warns that story-arc rows only take `tie_in_to` and will be
  skipped otherwise; focus returns to the toolbar button on close), and
  bulk **Reject** behind an `AlertDialog`. In the Stale view, selection
  offers bulk **Reject** only.

**Series page chips** (`SeriesSuggestedRelationships`, admins only, inside
the Related tab): pending suggestions touching the series from
`GET /api/series/{slug}/relationship-suggestions` (cursor-paginated, "Show
more"), read from this series' side (a row stored as "*other* `continues`
*this*" shows "Continued by *other*", from the view's `inverse_kind` /
`inverse_kind_label`, WP-7.5), the reason and confidence in a
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
default list and from `status=all`; `status=stale` shows them. Accepting a
stale row is `409` ("stale"). WP-8.2: **rejecting** a stale row is allowed
(single and bulk) — the way to dismiss a suggestion for good: it becomes
`rejected`, which is rejection memory, so when its evidence returns the
run skips it (`skipped_reviewed`) instead of reviving it. Stale itself is not rejection memory and
never touches reviewed rows. On the dev library the first run with stale
marking retired 31 low `same_universe` rows left over from earlier
heuristic versions; a second run marked none.

### Gaps

- **No spin-off, side-story or companion detectors.** Nothing in the DB
  separates a spin-off from a crossover, a same-universe title or a new
  volume: ComicInfo has no field for it, Metron `associated` series are
  untyped (WP-7.8), shared characters say "same universe" at best, and a
  name prefix ("Venom: Lethal Protector") is as often a mini-series of the
  same line. `side_story_of` and `companion_to` have even less signal.
  These stay manual.
- `issue_reprints` is filled from Metron issue `reprints` since WP-7.8
  (only for issues matched and applied from Metron); label-only reprint
  rows (`reprinted_issue_id` NULL) are skipped by the roll-up until the
  reprinted issue is scanned in and resolved.
- Translation evidence needs `series.language_code` to be right; a library
  tagged entirely in one language (the dev library: `en` / `eng` / `EN`)
  yields none. Without years, only alias evidence gives a direction.
- The continuation qualifier `merge` is never inferred, and `split` comes
  only from `series_provider_range` (the classic "one title split into
  two" has no evidence source).
- The derived "same universe" query and its section are WP-7.7.
- Cross-library suggestions are limited to the edition kinds (WP-8.2);
  story and publication kinds stay within one library by design.
- Cross-library freshness: a row is written only by its owner library's
  run. A trade library's `collects` row for a single newly scanned into
  another library appears on the trade library's next run (its own scan,
  or **Run now** for every library), not on the singles library's run.
- Collected-edition citations are parsed only for the run's own library
  (editions elsewhere are found by their own library's run); translation
  provider claims of other libraries count only series-level
  `external_ids`, not their issues' ComicInfo ids.
- The dev library has no cross-library edition pairs (its libraries are
  split by publisher); the behaviour was checked against a simulated trades
  library on a clone (see the WP-8.2 PR).

## Provider links and external targets (WP-7.8)

Only Metron exposes structured links: series `associated` (untyped,
symmetric) and issue `reprints`. GCD's REST API serves neither bonds nor
reprints (deferred until it does), ComicVine has no volume-to-volume links.
The provider-side fix (Metron `associated` was always parsed empty; aliases
now come from `alt_names`; reprints are now persisted) is in
`docs/dev/metadata-providers.md` ("Metron links"). Reprints feed the WP-7.6
reprint roll-up unchanged.

### Schema

`series_external_relationship` (migration
`m20270508_000001_series_external_relationship`, entity
`entity::series_external_relationship`) — "this series `kind` a provider
series", one-directional (no inverse half: there's no local series to hold
it):

| column | type | notes |
|---|---|---|
| `id` | `uuid` PK | |
| `from_series_id` | `uuid` FK → `series` ON DELETE CASCADE | |
| `kind`, `qualifier` | `text` | the 31 kinds; qualifier CHECK as `series_relationship` |
| `source` | `text` | `metron` / `comicvine` / `gcd` |
| `provider_series_id` | `text` | 1–64 chars |
| `provider_series_name`, `provider_series_url`, `provider_year` | | display + attribution link |
| `set_by` | `text` | `user` / `provider` |
| `confidence` | `real` NULL | provider rows |
| `evidence` | `jsonb` | `{ "source": "metron", "field": "associated", "ids": [own id, linked id], "label", "series_type" }` |
| `created_by` | `uuid` NULL FK → `users` | user rows |
| `promoted_series_id` | `uuid` NULL FK → `series` ON DELETE SET NULL | provider row matched locally |
| `dismissed_at`, `dismissed_by` | NULL | provider row an admin removed (rejection memory) |
| `first_set_at`, `last_synced_at` | `timestamptz` | |

Unique `(from_series_id, kind, source, provider_series_id)`; index on
`(source, provider_series_id)` for promotion. The same migration adds
`issue_reprints.reprinted_source` / `reprinted_external_id` (provider id of
a reprinted issue that isn't local yet). Down drops both (round-trip test
`crates/server/tests/migration_series_external_relationship.rs`).

### Writes (`crates/server/src/relationships/external.rs`)

- `record_provider_links(conn, series, source, own_id, own_type, links)` —
  the series apply (both the DB-direct and the sidecar-writeback path, via
  `write_series_scalar_fields`; links aren't a ComicInfo / MetronInfo
  field) upserts one `provider` row per linked series, kind from
  `provider_kind` (the linked series' type comes from its cached Metron
  detail, else from a local series matched to it, else from its name). A
  **user** row or a **dismissed** row with the same key is left alone;
  provider rows of that source the provider stopped listing are deleted.
  Not gated by the preview pane's `selected_fields`.
- `create_user_link` / `remove_link` — the admin API. Removing a user row
  deletes it; removing a provider row **dismisses** it (hidden, never
  re-created by a later apply, ignored by the engine). An admin adding a
  link with the same key as a provider row claims it (`set_by → user`,
  un-dismissed).

### Promotion

A row's provider series **resolves** to a local series through
`external_ids` (same source + id) or the **id bridge**: the provider's
cached detail of that series (`metadata_cache`) lists other providers' ids
(Metron's `cv_id` / `gcd_id`), and a local series is matched under one of
them (`resolve_where`; removed series and the row's own series don't
count; a direct match wins). On resolution:

- a **user** row becomes a manual pair (`create_pair_scoped`, same kind and
  qualifier, `created_by` kept) and is deleted; a contradicting kind
  (`PairError::Conflict`) leaves the row and logs a warning;
- a **provider** row is marked `promoted_series_id`; the suggestion
  engine's `provider_associated` source proposes the pair (suggestions
  only — the same dedupe, rejection memory, cap and stale rules as every
  source). The mark clears when the match goes away.

Promotion runs from `writers::set_external_id` the moment a series gains a
provider id (`promote_for_provider_id`, also through the bridge; WP-8.2:
`writers::set_external_id_promoting` also returns how many pairs it
created), after
`record_provider_links`, and per library before every suggestion run
(`promote_library`, which also resolves label-only reprints). Reads
resolve lazily too, so a missed hook degrades gracefully: the `GET` hides a
resolved provider row and lists a resolved user row with `local_series`
until the next run promotes it.

**Similar-series cache** (WP-8.2). A promoted user row is a new edge, a
similar-series signal, but the promotion code only holds a connection. The
callers that hold an `AppState` drop the cache when pairs were created
(`Promoted.pairs_created`): the metadata-apply job's post-apply external-id
write (`persist_applied_series_external_ids`, which runs after the apply's
own invalidation), the external-ids admin endpoints, the provider-range
reconcile, the scanner's folder-tag pass (runs even on non-mutating scans),
and the suggestion job (`relationship_suggest::run_with_state`, from
`RunReport.promoted_pairs`). Scanner file ingests are covered by the scan's
own invalidation (`ScanStats::mutated`), the series PATCH invalidates
unconditionally.

### Read side and admin API

`GET /api/series/{slug}/relationships` → `external`
(`api::series_external_relationships::external_views`): the series' live
rows, oldest first, minus dismissed rows and provider rows whose target
resolves locally. ACL: the series' own 404 gate; `local_series` only when
the caller can see that series (otherwise the row reads as external — it's
provider data, nothing leaks).

`POST /api/series/{slug}/external-relationships` (`RequireAdmin`):
`{ kind, qualifier?, source, provider_series_id (digits, ≤ 12), name
(1–300), year? (1800–2200) }`; 422 with field `details`; idempotent (`200`
for an existing user link, no audit row); when the provider series is
already local the pair is created right away (`201` with `relationship`,
`409` on a contradiction). `DELETE …/{id}` (`RequireAdmin`): `204`; `404`
for another series' row or an already-dismissed one. Audit:
`admin.series.external_relationship.create` (payload: kind, qualifier,
source, provider id, name, year, `promoted_relationship_id`) and
`…delete` (payload incl. `set_by` and `dismissed`).

### Related tab

External links render in their kind's group and heading
(`groupRelationships` puts them in the slot's `external` list), after the
local cards, as compact muted text rows — "Saga (2018) — not in your
library" plus the provider pill link (the `ProviderBadges` style, opens
the provider page). No cover: cover-size-aware cards are for local series
only. A resolved user link reads "in your library" and links to the local
series. Admins get a remove button behind an `AlertDialog` (which says a
provider link won't come back). The add dialog's target toggle has a third
mode, **Not in library** (provider select + numeric id + name + optional
year; only the qualifier of the scope applies), posting to
`/external-relationships` (`useCreateExternalRelationship`,
`useDeleteExternalRelationship`).

### Gaps (WP-7.8)

- **GCD bonds** are deferred: the GCD REST API exposes no series bonds or
  reprint links. ComicVine has no volume links.
- External links are read from Metron series details only; a series
  matched to ComicVine / GCD alone gets none.
- The reprint hook resolves pending rows by a **direct** provider-id match
  only; the cv / gcd bridge for reprints runs at apply time.
- External links aren't exported to OPDS feeds.

## Web

### Related tab (WP-7.7)

Relationships live in a **Related** tab on the series page, after
Collection in the strip (`web/app/[locale]/(library)/series/[slug]/SeriesTabs.tsx`
+ `web/components/library/SeriesRelatedTab.tsx`). The issue list follows the
tab strip directly; nothing relationship-related renders in the page body.

- **Lazy.** The panel is a `StackedTabsPanel` (no `forceMount`), so it is
  unmounted while inactive and none of the relationships / same-universe /
  similar queries fire until the tab opens
  (`web/tests/dom/series-related-tab.test.tsx` asserts no fetch before
  activation). `StableTabsPanelStack` pins its single column to
  `minmax(0, 1fr)` so a horizontal rail inside a panel can't widen the page.
- **Deep link.** `?tab=related` (any tab name) opens that tab; switching
  tabs `replaceState`s `?tab=` (dropped for the default Credits tab, other
  params such as the Issues panel's `?q=` kept). Unknown or unavailable tab
  names fall back to Credits.
- **Count.** The trigger reads "Related N": the server's
  `relationship_count` until the tab has loaded, then the live count from
  the query cache (`useSeriesRelationships(slug, { enabled: false })`, which
  never fetches on its own), so add / remove updates it.
- **Contents**, top to bottom: `SeriesRelatedSection` (header with the admin
  **Add relationship** button, the **Reading order** strip, the admin
  **Suggested** chips, the grouped relationships, **Part of event**), then
  **Same universe** (`SameUniverseSection`), then **Similar series**
  (`SimilarSeriesRail`, see `similar-series.md`).
- **Cover size.** Every cover in the tab — reading-order strip, relationship
  cards, same-universe and similar rails — renders at the issue grid's
  effective column width for the page's card-size slider. The slider stays
  in the Issues panel's "View options"; both read `folio.series.cardSize`
  (`SERIES_CARD_SIZE` in `web/lib/library/series-card-size.ts`).
  `useCardSize` syncs every instance sharing a key: the setter broadcasts a
  `folio:card-size` window event (same page) and other browser tabs follow
  through the native `storage` event. `useGridColumnWidth`
  (`web/lib/library/use-grid-column-width.ts`) measures the tab panel (as
  wide as the grid) with a `ResizeObserver` and applies
  `effectiveColumnWidth` (`web/lib/library/grid-window.ts`):
  `cols = max(1, floor((W + gap) / (size + gap)))`,
  `width = (W − gap·(cols − 1)) / cols` with the grid's 16 px gap. Browser
  check on the dev library: 180.28 px covers in both the grid and the tab
  at slider 160, 327.5 px at 280, 121.39 px at 120.
- **Empty / loading.** A cover-sized skeleton while loading; readers see
  "No related series linked yet." when there are none, admins a prompt.
  Group headings render a skeleton until the kind catalogue loads (never
  the raw group key).

### Relationship cards and editing

Direct relationships are grouped by UI group (Story / Publication history /
Editions & contents / Advanced) and display label, with a "Suggested" badge
on accepted suggestions and the scope (qualifier, ranges, coverage, note) as
secondary text (`scopeCaption`). **Part of event** lists the series' arc
edges with their role ("Prelude to Secret Wars", "Tie-in to …"), linking to
`/arcs/{slug}`.

For admins each card (and each event row) has **Edit** and **Remove**.
Remove sits behind an `AlertDialog`. Add and Edit open
`RelationshipFormDialog` (`web/components/library/RelationshipFormDialog.tsx`,
react-hook-form):

- **Relationship**: `RelationshipKindSelect`, a searchable command palette
  (shadcn `Command` in a popover) with a heading per group, tall enough to
  show every group, filtering on label and group name. A skeleton until the
  catalogue loads. The admin review page's "Edit kind" uses the same picker.
  One scroller only: the popover doesn't scroll; the list sits in the themed
  `ScrollArea`, sized to Radix's available height (capped at 26rem), with
  sticky group headings, and opens on the current kind. The dialog never
  scrolls itself (`overflow-visible`; pickers portal into it so the modal
  focus trap keeps their search live) — only its form body does, and only
  on viewports shorter than the form.
- **Target** (add only): a "Series | Story arc" toggle. Story arc is enabled
  only for arc-capable kinds (`allows_arc_target`, i.e. `tie_in_to`); each
  side has a typeahead (`/series?q=` and `/arcs?q=`, cursor-paginated with
  "More results").
- **Scope**: qualifier / role (the kind's allowed set from the catalogue),
  coverage (only when `allows_coverage`), this series' and the other side's
  ranges, note. Changing the kind drops scope the new kind doesn't take.
  Editing an arc edge keeps the kind list to arc-capable kinds.
- Edit sends `PATCH` with every scope field (`null` clears); Save is
  disabled until something changes. Server 422 `details` bind to the inputs
  via `applyServerErrors` (field names match the request body); the
  mutation hook still toasts.

Hooks: `useSeriesRelationships` (`queryKeys.seriesRelationships`, optional
`enabled`), `useCreateSeriesRelationship`, `useUpdateSeriesRelationship`,
`useDeleteSeriesRelationship`. All writes invalidate both series'
relationship lists, both similar rails (the server drops its similarity
cache on every write) and, for an arc edge, that arc's tie-in list.

The kind list comes from `GET /api/relationship-kinds`
(`useRelationshipKinds`, `queryKeys.relationshipKinds`, never refetched);
`web/lib/relationships.ts` only slices that catalogue (`groupedKinds`,
`kindInfo`, `kindLabel`, `kindOrder`, `scopeCaption`).

For admins the tab also shows the WP-7.3 **Suggested** chips
(`web/components/library/SeriesSuggestedRelationships.tsx`). The review
hooks live in `web/lib/api/mutations/relationship-suggestions.ts`
(`useAcceptRelationshipSuggestion`, `useRejectRelationshipSuggestion`,
`useReopenRelationshipSuggestion`, `useBulkAccept…`, `useBulkReject…`,
`useRunRelationshipSuggestions`); chip labels come from the suggestion
view's `kind_label` / `inverse_kind_label`.

### Same universe (derived)

"Same universe" is not an edge. `GET /api/series/{slug}/same-universe`
derives it on read: series sharing a `universe` with this one
(`series_universes`), or a ComicInfo `SeriesGroup` value (`series.series_group`,
split on `,` / `;`, compared trimmed and case-insensitively). Each item is
`{ series, shared: [{ via: "universe" | "series_group", name }] }`
(universes first). ACL as the relationships `GET`: 404 for a series the
caller can't see; listed series pass the library grant + age-rating cap;
removed series are listed for admins only. Keyset over `(name, id)`.

The section is a cover-size-aware horizontal rail with an end sentinel
that walks pages, each card captioned "Universe: Earth-616 · Group:
Spider-Man", hidden when nothing is shared. It replaces the pairwise
`same_universe` suggestions (WP-7.6 retires that source); manual
`same_universe` edges are ordinary relationships and still show in the
grouped list.

### Arc page tie-ins

`/arcs/{slug}` gains a **Tie-ins N** tab (only when the arc has tie-ins;
the first page is fetched with the page for the count) over
`GET /api/arcs/{slug}/tie-ins` (`useArcTieInsInfinite`,
`queryKeys.arcTieIns`): infinite scroll, grouped into Preludes / Main story
/ Tie-ins / Aftermath. The server orders by role, so `groupTieIns` only
groups contiguous runs and a later page never reopens an earlier group.

## Tests

- `crates/server/tests/series_relationships.rs`: inverse pair on create and
  delete, self-inverse kinds, idempotent create (200, one audit row),
  conflict 409, self-edge 422 (and the DB CHECK), non-admin 403, ACL
  filtering and chain pruning, cycle safety, the depth-6 cap, FK cascade,
  and OPDS v1/v2 related links. WP-7.5: the kind catalogue endpoint; scope
  validation (422 + field `details` for coverage / qualifier on the wrong
  kind, over-long range / note, arc target with a non-arc kind, missing or
  doubled target) and the mirrored inverse scope, plus the DB CHECKs; PATCH
  (scope edit on both halves, edit through the inverse half, kind change as
  delete + create, 422 / 409 roll back, audit rows, similarity
  invalidation, 403 / 404 / 400); arc targets (single row, idempotent, the
  target / kind / uniqueness constraints, the arc ACL on the series GET and
  on `/arcs/{slug}/tie-ins` incl. pagination and 404, PATCH / DELETE, FK
  cascade); the mixed `continues` + `sequel_of` chain with a `prequel_of`
  kept out, and the cross-family contradiction.
- `crates/server/tests/migration_relationship_taxonomy.rs`: the WP-7.5 data
  migration (down → seed old-shape rows → up → assert; then the lossy down
  and up again).
- `crates/server/src/relationships/mod.rs` unit tests: inverse involution
  and the full inverse table, groups and labels (catalogue order is
  grouped), scope rules and mirroring, contradictions, arc-capable kinds,
  and str/serde round-trip.
- `web/tests/library/series-related-section.test.tsx`: chain order and
  highlight, grouping by UI group and kind, scope captions, "Part of
  event", covers sized to the grid width, admin edit / remove gating, empty
  state, and the group-heading skeleton before the catalogue loads.
- WP-7.6, same integration file: one fixture-driven test per detector —
  annual (name / series type / format signals, volume containment,
  publisher mismatch, two equally fitting volumes), arc tie-in (main by
  name, tie-in, prelude by series name and by date, aftermath by issue
  title and by date, co-main crossover, single-series arc ignored),
  reprint roll-up ranges and coverage (`full` / `partial`, label-only rows
  ignored) plus citation roll-up, alternate edition (Director's Cut, Deluxe,
  "Absolute" singles ignored), facsimile → `reprints`, supplements,
  translation (shared creators, alias, provider series; language splits the
  continuation partitions), each continuation qualifier; the retired
  pairwise `same_universe` / arc-crossover rows going stale; accepting an
  arc / scoped suggestion (arc edge with role and range, mirrored inverse
  scope, a dropped qualifier on edit-kind, 422 for a non-arc kind, bulk
  accept); and the stress cap with the new sources.
  `crates/server/tests/migration_relationship_suggestion_scope.rs`: the
  WP-7.6 migration's down → up → CHECKs → lossy down → up round trip.
- WP-7.7: `crates/server/tests/relationship_ui.rs` (same-universe
  derivation, ACL, age cap, removed-for-admins and paging;
  `relationship_count`; OPDS v1/v2 arc links; arc tie-ins role order across
  pages and admin visibility of removed series; the tie-in role in
  similar-series reasons). Web: `web/tests/dom/series-related-tab.test.tsx`
  (no fetch before the tab opens, `?tab=` deep link and URL sync, live
  count), `web/tests/dom/card-size-sync.test.tsx` (column-width math,
  same-page and cross-tab sync), `web/tests/dom/relationship-form.test.tsx`
  (picker groups / search / skeleton / filter, edit PATCH body, scope per
  kind, 422 binding, arc target toggle) and
  `web/tests/library/arc-tie-ins.test.ts` (role grouping, cursor).
- `crates/server/tests/relationship_suggestions.rs` (WP-7.2): a fixture
  library with one cluster per evidence source (expected kind, bucket and
  reason), canonical dedupe of reversed self-inverse candidates, existing-
  edge dedupe, rejection memory and pending refresh on rerun, accept via
  `create_pair` (plus a kind override → `modified`, a contradicting edge →
  409, and similarity-cache invalidation), list pagination, filters and
  counts, non-admin 403 on every endpoint, `run` enqueue dedupe, audit rows,
  and the 1000 cap on a 3,000-series stress library.
- `relationships::suggestions` unit tests: canonical forms (every kind
  folds onto a canonical one), the `sequel_of` ≡ `continues` dedupe
  equivalence, merge and corroboration, buckets, the citation parser and
  the base-name helpers. Integration (`sequel_of_and_continues_dedupe_each_other`):
  a legacy rejected `sequel_of` row and existing `sequel_of` /
  `continued_by` edges suppress `continues` proposals; a `has_sequel` edge
  makes accepting a `continues` suggestion a 409.
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
- WP-7.8: `crates/server/tests/provider_links.rs` — reprints persisted on
  a DB-direct apply (Metron-id resolution, label + pending provider id,
  provenance), resolved by the `set_external_id` hook and by the
  suggestion run when the hook was missed, rolled up to a `collects`
  suggestion; user pin / override / fill vs replace; the writeback path
  (written only after the rewrite job, pin respected); `associated` →
  external rows with kinds refined by series type, evidence, no aliases;
  local pair → `provider_associated` suggestion (`collects`, `annual_of`);
  writeback series apply records links; provider-row promotion → suggestion
  (and un-marking), through the cached id bridge; user-row promotion → a
  manual pair (hook and suggestion run); the `GET` `external` list with
  ACL and `relationship_count`; admin create (validation, idempotency,
  promote-on-create) / delete (dismissal survives a re-apply), audit and
  403; the cache schema-version miss. Unit tests: `metron.rs`
  (`associated` labels, `alt_names` → aliases, reprint parsing),
  `relationships::external` (`provider_kind`, sources).
  `crates/server/tests/migration_series_external_relationship.rs`: down →
  up → CHECKs / unique / cascade → down → up. Web:
  `web/tests/library/series-related-section.test.tsx` (external rows in
  their group, provider link, local link, admin remove, empty state) and
  `web/tests/dom/relationship-form.test.tsx` ("Not in library" target,
  client checks, POST body, 422 binding).
- E2E: `web/tests/e2e/relationship-review.spec.ts` runs in the
  docker-smoke job (WP-8.5). It starts from the shared admin session that
  the Playwright `setup` project (`web/tests/e2e/admin.setup.ts`) creates —
  first-user registration, library over the generated fixture, scan — so it
  never registers and can't race `reader-flow.spec.ts` for the admin role.
  The fixture (`web/tests/e2e/fixtures/make-library.mjs`) writes
  "Relay (2011)" and "Relay (2016)" (ComicInfo Series `Relay`, Volume 1 /
  2, Year 2011 / 2016), so the post-scan run proposes
  "Relay (2016) continues Relay (2011)" (name continuation, 0.9). The spec
  polls for that pending row (bounded, no sleeps), accepts it from
  `/admin/relationships`, checks the pair and the reading-order chain over
  the API, and runs axe (WCAG 2.2 AA, `support/axe.ts`) on: the Related tab
  with the pending chip; the pending list; the "Edit kind" popover with the
  kind picker open; the Related tab with the link, reading-order strip and
  Similar rail; and the "Add relationship" dialog with the kind picker
  expanded.
- WP-8.2 (relationship tuning): `crates/server/tests/relationship_suggestions.rs`
  — `edition_kinds_pair_series_across_libraries` (a singles and a trades
  library: one cross-library pair per edition source, no story /
  publication pair across, ownership by the `from` library, idempotent
  reruns with no stale flip-flop, the library filter matching either end,
  rejection memory), `cross_library_edges_show_only_to_users_who_see_both_series`
  (admin-only suggestions; the accepted edge is listed only for a user with
  both libraries), `stale_rows_can_be_rejected_for_good` (single + bulk,
  not revived when the evidence returns), `bulk_accept_by_ids_takes_an_optional_kind`
  (modified + `accepted_kind`, scope fitted, arc row fails as `invalid`, one
  audit row carrying `kind`, `kind` without `ids` → 422) and
  `noise_rules_drop_publisher_only_and_unlimited_matches`. Unit tests: the
  noise rule, the "Unlimited" rule and `index_keys` (`detectors.rs`), both
  branches of the arc main-series rule (`sources.rs`), `may_cross_libraries`
  (`relationships/mod.rs`), and the tie-in / `issue_arcs` max in
  `similarity.rs`. `crates/server/tests/series_similar.rs`:
  `accepted_arc_tie_ins_are_a_signal_without_double_counting` and
  `promoting_an_external_link_invalidates_the_cache` (external-ids endpoint
  and the suggestion job). Web: `web/tests/admin/relationship-suggestions-panel.test.tsx`
  (bulk "Accept as…" with focus return, stale reject single + bulk) and
  `web/tests/library/similar-series.test.tsx` ("both tie in to").
