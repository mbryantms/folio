# Similar series (WP-7.4)

Content-based, explainable "more like this" with no ML. Two series are
similar when they share metadata entities; every match carries the list
of shared entities that produced it. Code: scoring and cache in
[`crates/server/src/similarity.rs`](../../crates/server/src/similarity.rs),
endpoints in
[`crates/server/src/api/series_similar.rs`](../../crates/server/src/api/series_similar.rs),
tests in
[`crates/server/tests/series_similar.rs`](../../crates/server/tests/series_similar.rs).

## Endpoints

| Endpoint | What | Paging |
|---|---|---|
| `GET /api/series/{slug}/similar` | Neighbours of one series. Feeds the "Similar series" rail in the series page's Related tab. | Keyset cursor over `(score DESC, series_id ASC)`; `total` on the first page only; `limit` 1–50 (default 20). |
| `GET /api/me/similar-series` | Optional home rail: neighbours of the series the caller read most recently (`seed`), minus every series they've already started. | Same cursor; the cursor pins the seed so later pages don't switch seeds. |

The home rail is the system saved view `similar_series`
(`00000000-0000-0000-0000-000000000013`, migration
`m20270504_000001_similar_series_rail`). It is `auto_pin = false`: users
add it from the "Built-in" group of the pin picker. Its "View all" page
(`/views/similar-series`) walks every page.

Each item is `{ series: SeriesView, score, because: [SimilarReason] }`,
where a reason is `{ kind, role?, name, weight }`:

- `kind`: `creator`, `character`, `team`, `arc`, `genre`, `tag`,
  `publisher`, `imprint`, `relationship`.
- `role`: the credit role for creators (`writer`, `penciller`, …); the
  relationship kind, read from the neighbour's side, for relationships
  (`has_sequel` / `continued_by` when the neighbour comes before this
  series — WP-7.5 split `prequel_of` off as a narrative prequel); the
  `name` of a relationship reason is this series with its year.
- `label` (relationships only, WP-7.5): the kind's display label,
  lower-cased — e.g. "continued by Agents of Atlas (2020)" — so the web
  needn't map kinds itself. A tie-in role is folded in (WP-7.7): a
  `tie_in_to` edge with role `prelude` reads "prelude to Secret Wars
  (2015)", its other side "has prelude …", not "tie-in to" / "has tie-in".
  Arc-target edges never contribute (the signal joins on `to_series_id`).
- `weight`: that entity's contribution (before the per-kind cap).

The web renders up to three reasons as a caption (a relationship reason
uses `label`): "Because: writer Ed Brubaker, character Bucky Barnes, arc
Winter Soldier"
([`web/lib/similar.ts`](../../web/lib/similar.ts)).

## Signals and weights

Signals come from the series-level junctions the scanner rolls up
(`series_credits`, `series_characters`, `series_teams`, `series_genres`,
`series_tags`), story arcs (`series_arcs` plus `issue_arcs` of the
series' active issues, because the `series_arcs` rollup is not always
populated), the series' `publisher` / `imprint`, and accepted
relationships (`series_relationship`, WP-7.1).

A shared entity contributes `weight × idf`, with a normalised IDF:

```
idf = clamp(ln(N / df) / ln(N), 0, 1)
N   = live (non-removed) series
df  = live series carrying the entity
```

An entity on every series counts 0; an entity on two series counts
almost fully. Entities carried by more than half the library are skipped
outright (stop-entities: "Marvel" in a Marvel-heavy library, the line
editor on every book).

| Kind | Weight per entity | Cap per kind | Breadth-damped |
|---|---|---|---|
| creator: writer | 3.0 | 6.0 (all roles together) | yes |
| creator: penciller | 2.0 | | |
| creator: inker | 0.75 | | |
| creator: colorist / cover artist / translator | 0.5 | | |
| creator: letterer / editor | 0.25 | | |
| character | 1.5 | 6.0 | yes |
| team | 1.5 | 3.0 | yes |
| story arc | 3.0 | 6.0 | no |
| genre / tag | 0.75 | 1.5 each | no |
| publisher | 0.5 | 0.5 | no |
| imprint | 1.0 | 1.0 | no |
| relationship (any kind) | 6.0 flat, no IDF | 6.0 | no |
| arc via accepted tie-ins (WP-8.2) | 3.0 × IDF over tie-in membership | shares the story-arc 6.0 | no |

Rules on top of that:

- **One person counts once.** Someone credited as writer and penciller
  on both books contributes at their strongest role.
- **Per-kind cap.** Fifty shared cover artists cannot outweigh a shared
  writer plus a shared arc.
- **Breadth damping** for the long-list kinds (creators, characters,
  teams): the capped sum is multiplied by
  `min(1, sqrt(|target's entities of that kind| / |candidate's|))`. A
  700-issue flagship shares a few names with everything; without this it
  would top every list.
- **Score** = sum over kinds. Candidates under `MIN_SCORE = 1.0` are
  dropped (one shared rare writer clears it; a shared genre alone does
  not). The top `MAX_NEIGHBORS = 100` are kept, with their top
  `MAX_REASONS = 5` reasons.
- **Accepted arc tie-ins** (WP-8.2, `fetch_arc_tie_ins`): two series that
  both have an accepted `tie_in_to` edge to the same story arc share that
  arc. It contributes an **arc** reason (`label: "both tie in to"`, the web
  reads "both tie in to Secret Wars") worth `3.0 × idf`, where `df` is the
  number of live series tied in to that arc. **No double count with the
  `issue_arcs` / `series_arcs` signal**: both are arc reasons named after
  the same arc, so the per-(candidate, kind, entity) dedupe keeps only the
  larger one, and every arc reason shares the arc cap (6.0). The tie-in
  only adds when the issues aren't tagged with the arc (a manual edge) or
  when fewer series are accepted tie-ins than carry the tag (a higher IDF).
  Why max rather than a sum under a combined cap: both say "these two are
  in the same event"; adding them would count one fact twice. On the dev
  library, after accepting the Planet Hulk and Weirdworld tie-ins to
  `"Secret Wars" Battleworld`, Weirdworld's reason on Planet Hulk's rail
  became "both tie in to" at 2.735 (df 2) instead of the tagged arc at
  1.436 (df 60).
- **Relationships** count whatever their `source` (`manual` or
  `suggested`): suggestions live in the WP-7.2 table and only reach
  `series_relationship` once accepted. A related series is listed even
  when it shares no metadata.

The weights are constants in `similarity.rs` (`ReasonKind::weight` /
`cap`, `creator_role_weight`), not operator settings.

## Query shape

A cache miss costs four statements (WP-8.2 added the tie-in query):

1. **Overlap** (`OVERLAP_SQL`): the target's entities (`feats`), every
   series carrying one of them (`postings`, one index probe per kind:
   `series_credits(role, person)`, `btrim(lower(character|team))`,
   `genre`, `tag`, `arc_id`, `btrim(lower(publisher))`), `df` per entity,
   one row per (candidate, shared entity). The CTEs that feed several
   consumers are `MATERIALIZED`: inlined, Postgres re-evaluated the
   library count per row and estimated every CTE at one row, and an
   early single-statement version that also scored in SQL ran 2.2 s and
   then over 2 minutes on the dev library. Scoring moved to Rust.
2. **Relationships**: edges out of the target (inverse rows are stored,
   so that's the whole neighbourhood), joined to both series for names.
3. **Arc tie-ins** (WP-8.2): the target's accepted `tie_in_to` arcs, every
   series tied in to them (`series_relationship_to_arc` index), `df` per
   arc.
4. **Sizes**: distinct creators / characters / teams per candidate, for
   the damping.

On the dev library (2,573 series, 22k issues, 73k series credits) the
overlap query takes about 18 ms for a typical series and about 60 ms for
the largest (The Amazing Spider-Man 1989: 701 credits, ~20k overlap
rows). A cache hit fires no scoring queries; the request itself is slug
lookup + grants + hidden lookup + page hydrate. The perf guard in
`crates/server/tests/perf_regressions.rs` bounds both: cold ≤ 20
(observed 11 before WP-8.2 added the tie-in query), warm ≤ 15 (observed 8).

## Cache

In-process LRU (`SimilarityCache` on `AppState`, 512 series), one entry
per target series holding the **unfiltered** neighbour list (library id
and age rating carried per neighbour). Why in-process rather than a
table: Folio runs as a single instance (roadmap D2), the list is cheap to
rebuild, and a table would need its own invalidation writes in the same
places anyway.

Invalidation is global: one generation counter, bumped (and the LRU
cleared) by

- every scan that changed the catalogue
  (`library::scanner::finalize_run`, gated on `ScanStats::mutated()` or a
  failed scan; a no-op cron or watcher pass keeps the cache warm),
- every metadata apply (`metadata::apply::apply_series` /
  `apply_issue`, `metadata::composite::apply_composite`),
- manual metadata edits (issue PATCH and bulk metadata through
  `manual_rewrite_after_edit`; series PATCH),
- relationship create / delete (`api::series_relationships`),
- WP-8.2: an external link promoted to a series relationship — the
  metadata-apply job's post-apply external-id write, the external-ids admin
  endpoints, the provider-range reconcile, the scanner's folder-tag pass and
  the suggestion job, each only when the promotion created a pair (see
  `series-relationships.md` → "Promotion").

Per-series invalidation would be wrong: changing series B moves it into
or out of A's list and shifts every IDF. A compute that raced an
invalidation is not stored (the entry records the generation it was
computed under). A 15-minute TTL bounds staleness from any write path
that doesn't call `invalidate_all` (for example, a future WP-7.3 bulk
accept that writes relationships outside these handlers should call it).

Metric: `folio_similar_series_cache_total{result="hit"|"miss"}`.

## ACL and per-user filtering

Applied per request, never cached:

- **Library ACL + age-rating cap**: `VisibleLibraries::series_ok` on each
  neighbour (same rule as the series list). The target itself must be
  visible or the endpoint returns 404.
- **Hidden series**: a still-current `rail_dismissals` row of kind
  `series` (the per-user hide the On Deck rail uses, with its
  auto-restore rule: a newer progress write on the series un-hides it).
  The web's similar-series cards add "Hide from suggestions" to the
  cover menu, which writes that dismissal.
- **Removed series** never enter the cached list.
- **Home rail only**: excludes every series the caller has started
  (progress with `last_page > 0` or `finished`), and the seed. The seed
  is the most recently read visible, unhidden series; if its neighbours
  are all filtered out, the next two most recent are tried.

## Examples from the dev library

From the running branch against the dev library:

- **Agents of Atlas (2009)**, which has a manual `sequel_of` edge to
  the 2007 series: Agents of Atlas (2007) 9.33 (prequel of Agents of
  Atlas (2009), writer Jeff Parker, penciller Leonard Kirk), X-Men vs.
  Agents of Atlas (2010) 6.00 (writer Jeff Parker, penciller Gabriel
  Hardman), Avengers vs. Atlas (2010) 4.61. (Captured before WP-7.5; the
  reason label now reads "has sequel Agents of Atlas (2009)".)
- **Home rail** for the test account: "Because you read Monstress":
  X-23 (2011) (writer Marjorie Liu, penciller Sana Takeda), NYX: No Way
  Home (2008), Black Widow (2010), all through writer Marjorie Liu.

- **Daredevil (2013)** (Mark Waid / Chris Samnee): Daredevil (2014) 5.80
  (writer Mark Waid, penciller Chris Samnee, penciller Javier Rodriguez),
  Avenging Spider-Man (2013) 3.46 (writer Mark Waid, penciller Marco
  Checchetto), Superior Spider-Man (2014) 3.34 (penciller Marcos Martín,
  penciller Javier Rodriguez).
- **Hawkeye (2013)** (Matt Fraction / David Aja): The Immortal Iron Fist
  (2007) 3.66 (writer Matt Fraction, penciller David Aja, penciller
  Javier Pulido), Hawkeye Annual (2013) 3.44, Civil War: Choosing Sides
  (2006) 2.81.
- **Saga (2015)** (Image; sparse credits): Phantom Road (2023) 2.66
  (team Zombies, genre Fantasy, genre Horror), Logan (2008) 2.21 (writer
  Brian K. Vaughan), The Walking Dead (2015) 2.16 (team Zombies, genre
  Mature, genre Horror), X-Men/Runaways (2006) 2.04 (writer Brian K.
  Vaughan).

## Tuning

Change a weight or cap in `similarity.rs`, rerun
`cargo test -p server --test series_similar` (ranking, because-list,
paging) plus the unit tests in `similarity.rs`, and sanity-check a few
dev-library series. The integration fixtures are sized against
`MIN_SCORE`; a large weight cut can push their neighbours under it.

## Series page rail (WP-7.7)

The "Similar series" rail lives in the series page's **Related** tab
(`web/components/library/SeriesRelatedTab.tsx`), below the relationships
and the "Same universe" section. The tab panel is unmounted while
inactive, so `GET /series/{slug}/similar` fires only when the tab opens.

Its cards are sized by the page's card-size slider (Issues panel → View
options, key `folio.series.cardSize`) instead of a fixed 160 px:
`SimilarSeriesRail` takes `itemWidthPx`, which the tab sets to the issue
grid's effective column width (`useGridColumnWidth` over
`effectiveColumnWidth`, see `series-relationships.md` → "Related tab"), so
the covers match the grid exactly and follow the slider live. Without the
prop the rail falls back to 160 px.
