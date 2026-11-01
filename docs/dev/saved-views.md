# Saved views — filter DSL, compiler, and view kinds

Saved views are rows in `saved_views`, discriminated by `kind`:

| kind            | What it lists                             | Results endpoint                                  |
| --------------- | ----------------------------------------- | ------------------------------------------------- |
| `filter_series` | series matching a filter DSL              | `GET /me/saved-views/{id}/results` (`SeriesListView`) |
| `filter_issues` | issues matching a filter DSL (WP-5.4)     | `GET /me/saved-views/{id}/issue-results` (`IssueListView`) |
| `cbl`           | an imported reading list                  | `GET /me/cbl-lists/{id}/entries`                  |
| `collection`    | a hand-curated mix of series + issues     | `GET /me/collections/{id}/entries`                |
| `system`        | built-in rails (Continue reading, …)      | per-rail endpoints (`/me/continue-reading`, …)    |

Each results endpoint refuses the other kinds with 422
`unsupported_view_kind` rather than returning an empty page. The two
filter kinds share one CHECK-constraint branch (match mode + conditions +
sort + limit; no CBL list, no system key). Any view kind can be pinned as a
home/page rail; the web rail (`SavedViewRail`) dispatches on `kind` and
renders series cards for `filter_series` and issue cards for
`filter_issues`.

Stateless previews: `POST /me/saved-views/preview` (series) and
`POST /me/saved-views/preview-issues` (issues) take the same `PreviewReq`.

## The DSL

`FilterDsl = { match_mode: all | any, conditions: [{ field, op, value }] }`
(`crates/server/src/views/dsl.rs`). `value`'s shape depends on the op:
a scalar for `equals` / `is` / `gt` …, `[lo, hi]` for `between`, an array
for `in` / `includes_any` …, a positive day count for `relative`, and
nothing for `is_empty` / `is_not_empty` / `is_true` / `is_false`.

## The registry

`crates/server/src/views/registry.rs` is the single source of truth per
field: kind, allowed ops, enum values, and **where the field lives in SQL
on each entity** — `source` for series views, `issue_source` for issue
views. `None` means the field isn't available on that entity and the
compiler returns `FieldNotAvailable` (422 `filter_invalid`).
`web/components/filters/field-registry.ts` mirrors it by hand
(`entities` / `issueLabel`); `web/tests/library/filter-registry.test.ts`
guards the mirror.

| Field                                   | Series views                         | Issue views                              |
| --------------------------------------- | ------------------------------------ | ---------------------------------------- |
| `library`, `year`, `volume`, `publisher`, `imprint`, `age_rating`, `language_code`, `created_at`, `updated_at` | `series.*` | `issues.*` (the issue's own column)       |
| `name`, `status`, `total_issues`        | `series.*`                           | the **parent series'** column (labelled "Series name" etc.) |
| genres, tags, credit roles, characters, teams, locations | `series_*` junction `EXISTS` | `issue_*` junction `EXISTS` (`issue_id`) |
| `read_status`                           | per-series rollup over `user_series_progress` | per-issue: `finished` → read, `last_page > 0` → in progress, else unread (same rule as `GET /issues?read_status=`) |
| `rating`                                | caller's own series rating (`user_ratings`, `target_type='series'`) | caller's own issue rating (`target_type='issue'`) |
| `special_type`, `format`, `story_arc`, `title` | —                             | `issues.*` (`title` = the issue's own title) |
| `has_notes`, `has_bookmarks`, `has_highlights` (WP-5.7) | the caller has a marker of that kind on any (non-removed) issue of the series | the caller has a marker of that kind on the issue |
| `has_favorites` (WP-8.4) | the caller starred something on any (non-removed) issue of the series: a `favorite` marker or any marker with `is_favorite` | same, on the issue |
| `read_progress`, `last_read`, `read_count` | per-series rollups over `user_series_progress` | the caller's own `progress_records` row: `percent`, `updated_at`, `run + finished` (completed reads); numbers COALESCE to 0, dates stay NULL (filter-parity-and-builtin-views-1.0 M1) |
| `finished_at` (M1)                      | `MAX(progress_records.finished_at)` over the series' active issues for the caller | the issue's own `finished_at` |
| `metadata_completeness`                 | 3-tier rollup (`complete` / `partial` / `needs_metadata`) | binary per issue via `completeness::issue_metadata_satisfied_sql` — `partial` never matches, and the builder hides it on issue views |
| `unread_issues`, `collection_completeness` | per-series rollups | — |
| `story_arcs`, `universes`, `concepts`, `objects` (M4) | `series_*` junction → lookup table `name` (`EXISTS … JOIN story_arc l ON l.id = j.arc_id`) | `issue_*` junction, same shape |
| `any_credit` (M4)                       | `series_credits` any role              | `issue_credits` any role                  |
| `provider_match` (M4)                   | `matched` / `unmatched` by an `external_ids` row (`entity_type='series'`, the dashboard's source list) | same, `entity_type='issue'` |
| `release_date` (M4)                     | `make_date(year, 1, 1)`                | `year`-`month`-`day` composed as a `date`; missing month / day fall back to 1, out-of-range days roll forward |
| `series_type`, `series_group`, `year_end` (M4) | `series.*`                      | the parent series' column                 |
| `summary` (M4)                          | `series.summary`                       | `issues.summary`                          |
| `page_count` (M4)                       | —                                      | `issues.page_count`                       |

### `is_empty` / `is_not_empty`

Offered on nullable fields only:

- text → `btrim(COALESCE(col, '')) = ''` (blank counts as empty; ComicInfo
  round-trips often leave empty elements);
- number / date / enum → `col IS NULL` (an unrated row has a NULL
  `rating`; an ordinary issue has a NULL `special_type`);
- multi (junction) → `NOT EXISTS` any junction row for the entity.

NOT NULL columns (`name`, `status`, `created_at`, `updated_at`, `library`)
and computed rollups (`read_status`, completeness, counts) don't offer
them — the result would be a constant.

`not_contains` / `not_equals` keep SQL three-valued semantics: rows whose
column is NULL are excluded (combine with `is_empty` under `any` to keep
them).

### `relative` / `not_relative`

Both take a positive day count. `relative` is `col >= NOW() - N days`
("in the last N days"); `not_relative` (filter-parity-and-builtin-views-1.0
M3) is the strict `col < NOW() - N days` ("older than N days"). A NULL
date never matches either, so "started but untouched for two weeks" is
`read_status is in_progress` + `last_read not_relative 14`, and
"never read or stale" adds `last_read is_empty` under `any`.

Date values are bound as text and cast to `timestamptz` in SQL (a bare
text parameter against a timestamp column raises `operator does not
exist` in Postgres).

### Annotation filters (WP-5.7)

`has_notes` / `has_bookmarks` / `has_highlights` / `has_favorites` are
boolean fields (`is_true` / `is_false`, no value) compiled to
`[NOT] EXISTS (SELECT 1 FROM markers m JOIN issues mi … WHERE m.user_id =
<viewer> AND m.kind = '<kind>' AND …)`. Markers are private, so the probe
is **always** scoped to the viewing user — another user's notes never
make a row match, and a shared system view evaluates per viewer. Markers
on removed issues don't count. `has_favorites` (WP-8.4) matches
`(m.kind = 'favorite' OR m.is_favorite)` — favorite is both a kind (the
page-level star) and a flag on any marker, the same union the
/bookmarks "Favorites" chip shows; it reuses `Source::MarkerExists`
with the `favorite` kind, special-cased in `marker_predicate`. The
series-level probe is served by `markers_user_series_kind_idx`
(`m20270307`); the favourite flag arm filters within the same
`(user_id, series_id)` prefix.

## The compiler

`crates/server/src/views/compile.rs` has two roots:

- `compile(&CompileInput)` — `SELECT series.* FROM series`, LEFT JOINing
  `user_series_progress` / the active-issue-count / metadata-completeness
  aggregates only when a condition or sort needs them. Keyset cursor on
  `(sort value, series.id)`; the sort key is also projected as text
  (`sort_value`) so the results handler encodes a real cursor for every
  sort, including the per-user ones (`last_read`, `read_progress`,
  `finished_at`, `rating`) that aren't `series` columns.
- `compile_issues(&IssueCompileInput)` — the `IssueSummaryView` card
  columns (never `comic_info_raw` / `pages`) plus `series_slug` /
  `series_name`, `FROM issues JOIN series`, active + not-removed issues
  only, library ACL on `issues.library_id` and the WP-2.7 age-rating cap
  on the issue's effective rating. Sorts: `name` (series name, then issue
  number, then id), `year`, `created_at`, `updated_at`; `last_read` /
  `read_progress` / `finished_at` / `rating` join the caller's own
  `progress_records` (`pr`) / `user_ratings` (`ur`) row (LEFT JOIN, only
  for the sort that needs it; projected as `sort_*` columns so the row
  shape is constant); `page_count` / `number` are issue-only
  (`SortNotAvailable` on series views, 422). Paginated with an opaque
  **keyset** cursor (`IssueCursor`: base64 JSON of the last returned
  row's sort-key values + issue id). The "after" predicate is
  lexicographic over `(keys…, issues.id)` in the view's direction with
  nullable keys (`sort_number`, `year`) ordered NULLS LAST, so issues
  added or removed between page fetches never cause skipped or repeated
  rows. `total` is counted on the first page only.

`compile::validate(dsl, entity, sort_field, sort_order)` is what the
create / update handlers call before persisting; an update re-validates
the stored filter against a new sort (and vice versa), so an issue view
can't be PATCHed onto a series-only sort.

User-supplied values are always bound parameters; table / column / role
identifiers in the hand-written SQL fragments come from the registry.

## Adding a field

1. Add the `Field` variant (`dsl.rs`) and a `FieldSpec` row with both
   `source` and `issue_source` (`registry.rs`); bump
   `KNOWN_FIELD_COUNT`.
2. If it needs a new SQL shape, add a `Source` variant and handle it in
   `compile_condition`.
3. Mirror it in `web/components/filters/field-registry.ts` (with
   `entities` if it's single-entity) and the registry test's field list.
4. `just openapi` (the `Field` enum is in the spec).

## Built-in views

Global rows (`user_id IS NULL`, `is_system = true`), seeded by migration
with fixed UUIDs. `auto_pin = true` rows land on every user's Home via the
lazy pin seed in `saved_views::list`; the rest show under "Built-in" in
the pin picker.

| id suffix | kind | name | auto_pin |
| --- | --- | --- | --- |
| …0001 / …0002 | filter_series | Recently added / updated series | ✓ |
| …0003 | filter_series | Recently finished series (was "Just Finished") | — |
| …0010 / …0011 / …0012 | system | Continue reading / On deck / New issues | ✓ |
| …0013 | system | Similar series | — |
| …0014 / …0015 / …0016 | system | Random picks (`/me/random-picks`) / On this day (`/me/on-this-day`) / Recently read (`/me/recent-history`) | — |
| …0020 | filter_issues | Recently finished issues (was "Recently finished" in v0.60.0; renamed by `m20271020_000002`) | ✓ |
| …0021 – …0027 | filter_issues | Finished this week, Rate what you read, Top rated issues, Annuals & specials, Short unread reads, Favorite issues, Annotated issues | — |
| …0028 – …0034 | filter_series | Jump back in, Almost done, Complete runs to start, Ended but unfinished, Top rated series, Incomplete collections, Favorite series | — |

`crates/server/tests/saved_views.rs::every_builtin_filter_template_resolves`
runs every filter template for a fresh user, so a DSL / sort that drifts
from the registry fails CI rather than the home page. Adding a template:
append to `TEMPLATES` in `m20271020_000001_builtin_views_wave2` (or a new
migration), bump that test's expected count.

## Save-as-view from the library grid

`web/components/library/libraryGridStateToFilterState.ts` translates the
grid's facet state into a builder seed. Series mode seeds a
`filter_series` view, issues mode a `filter_issues` view; the rating and
read-status chips carry over as `rating between` / `read_status in`.
Facets with no equivalent in the DSL are reported as dropped and
toasted rather than silently lost: the A–Z letter (the grid matches it
on `normalized_name`; `starts_with` is a case-sensitive probe on the raw
name), the per-provider `provider_source` narrowing, and the `partial`
metadata tier in issues mode (binary per issue). Provider match, any-role
credits and the two remaining metadata tiers carry over on both entities.

## Groups on the Views index

Settings → Views renders each tab (Filter views / Reading lists /
Collections) as a **grouped board**: the user drags cards into named
buckets that are renamed, reordered, collapsed, and deleted
independently of the views themselves. Groups are an organizational
overlay only — membership never affects what a view is, where it is
pinned, or whether it shows in the sidebar.

Storage (`m20270616`): `saved_view_groups` (per user, `scope` ∈
`filter` / `cbl` / `collection`, `position`, `collapsed`) and
`saved_view_group_items` keyed by `(user_id, view_id)` so a view sits
in at most one place per user, system views included. An item row with
a `group_id` is group membership; a row with a NULL `group_id` pins the
view's `position` inside the **ungrouped** remainder of its `scope`, so
hand-arranged order persists there too. Views with no row are ungrouped
with no pinned position. Items cascade with both the group (→ the view
is ungrouped) and the view. The client overlays all of this on the view
lists (`buildViewBoard` in `web/lib/saved-views/view-board.ts`): under
the default "Custom order" the pinned ungrouped ids lead and the rest
trail by name; the other sort keys re-sort the whole section, and a
drag while one is active pins the visible order and switches back to
Custom.

API (`crates/server/src/api/view_groups.rs`, 50 groups per scope):

| Method | Path                     | Notes                                                                       |
| ------ | ------------------------ | --------------------------------------------------------------------------- |
| GET    | `/me/view-groups?scope=` | groups + ordered `view_ids`, plus `ungrouped_ids`                           |
| POST   | `/me/view-groups`        | `{scope, name, view_ids?}`; appends; moves listed views                     |
| PATCH  | `/me/view-groups/{id}`   | `{name?, collapsed?}`                                                       |
| DELETE | `/me/view-groups/{id}`   | members become ungrouped                                                    |
| PUT    | `/me/view-groups/layout` | `{scope, groups: [{id, view_ids}], ungrouped?: [id]}` — whole-scope replace |

The layout `PUT` is what drag-and-drop commits to: one request carries
the group order, every group's members, and the ungrouped order, so a
cross-group move is atomic. It must list every group in the scope
exactly once and every view at most once across groups and `ungrouped`
(422 otherwise); unknown or other-user view ids are 422. `ungrouped`
may be omitted or partial — unlisted ungrouped views simply lose their
pinned position. The web mutation patches the query cache
optimistically and snaps back on error. Regression coverage:
`crates/server/tests/view_groups.rs` and
`web/tests/library/view-board.test.ts`.

The reading-lists tab also hosts the **updates feature**
(`CblRefreshDashboard.tsx`), kept out of the way until asked for: an
"Updates" button in the board toolbar (`CblUpdatesMenu`) runs "Check for
updates" (dry-run via `/me/cbl-lists/check-all`) or "Update all lists…",
and either one reveals the updates panel (`CblUpdatesPanel`) between the
toolbar and the board. The panel shows the review list with diff badges
and checkboxes, and refreshes selected lists one at a time through the
shared client-side runner (`useBatchRefresh`) with progress and per-list
outcomes; the group menu's "Update these lists" feeds the same runner and
opens the same panel. The button carries a count badge once a check has
found changes and spins while anything runs, so the panel can stay
hidden. State lives in `CblUpdatesProvider`, so hiding the panel keeps
the results.

## OPDS

Pinned or sidebar-visible filter views appear in the OPDS personal feeds
(`/opds/v1/views`, `/opds/v2/views`, and each page under `/opds/*/pages/
{slug}`), both kinds since WP-8.4. `/opds/*/views/{id}` dispatches on
kind: a `filter_series` view renders series subsection / navigation
entries; a `filter_issues` view renders issue acquisition entries (v1)
or publications (v2) through `opds::issue_view_issues`, which runs the
same `compile_issues` root as `…/issue-results` (ACL, age-rating cap,
the view's sort) capped at the view's `result_limit`, like the series
feed. Another user's view is a 404 on both protocols.

## Multi-select on the detail pages

Both filter-view detail pages carry the shared `useSelection` +
`SelectionToolbar` wiring (`web/components/saved-views/FilterViewDetail.tsx`,
`IssueFilterViewDetail.tsx`; the latter since the 2026-11 audit, N-O4).
Select sits before the card-size icon in the title row and in the phone
bar's View options sheet (phone-layout rule 11); the bar yields to the
sticky toolbar while selecting. The series view's actions are
series-shaped (every selected series expands to its issues server-side:
Mark read / Mark unread, Add to collection, Fetch metadata); the issue
view's are issue-shaped (`/me/progress/bulk` Mark read / Mark unread with
the shared backfill prompt at `BULK_BACKFILL_PROMPT_THRESHOLD`, Add to
collection). Only the loaded pages are selectable, as on every other
saved-view surface. `web/tests/dom/issue-filter-view-select.test.tsx`
covers the issue view.

## Hidden from rails

`POST /me/rail-dismissals` hides an issue, series or CBL list from the
home rails; `GET /me/rail-dismissals` lists the caller's own rows that
still hide something, newest dismissal first, cursor-paginated (`limit` default 50, max 200; `total`
and `unavailable` on the first page only). Each item carries the target
hydrated for display (name, subtitle, slug / series slug / saved-view id,
cover) and `hidden_from`, the rails that kind is filtered out of — the
row does not record which rail the click came from, so this is the fixed
set per kind (issue: Continue reading, Recently read and the OPDS
Continue reading feeds; series: On Deck, Random picks, Similar series;
cbl: On Deck). A row the rails already ignore — the caller has progress
newer than `dismissed_at` on the issue, or on any issue of the series —
is not listed or counted (`still_hiding_condition`); CBL rows stay,
since On Deck's restore rule for them reads the list's reading frontier.

Visibility: a row is listed only while its target passes the same
selects collections use (`access::visible_issues_select` with
`state = 'active'`, `access::visible_series_select`, and a CBL list the
caller owns or that is global). A dismissal whose target is soft-removed,
outside the caller's grant, over the age cap, or deleted is **omitted**
and counted in `unavailable`, so nothing about it reaches the client; it
reappears in the list if the target becomes visible again.
Creating a dismissal applies the same selects: a target the list would
omit (soft-removed, non-active issue, outside the grant, over the cap)
answers the same 404 as a missing one and stores nothing.

## Visibility on the other personal surfaces

- **Collections**: `POST /me/collections/{id}/entries` and
  `…/members/bulk-add` accept only a target the entries page would list
  (`collections::visible_{series,issues}_select`); an invisible target is
  a 404 (single add) or `not_found` (bulk add), exactly like a missing one.
- **CBL lists**: a match whose issue (or its series) was soft-removed reads
  as **missing**: `stats.matched` drops it and `stats.missing` gains it,
  `?status=matched` leaves it out and `?status=missing` (the Resolution
  tab) lists it, and the entry view says `match_status: "missing"` while
  keeping `matched_issue_id`, so the match returns with the file.
- **Markers**: list, count, search and the tag index use
  `access::visible_issues_select` / its SQL twin `access::visible_issues_sql`,
  so a removed parent series hides its issues' markers too.
- **Reading stats**: `/me/reading-stats` counts only sessions on issues the
  caller can see (`access::visible_issues_sql`), totals and streaks
  included, because the page links its rankings to series and issues and
  the totals must agree with them. The admin's per-user view applies the
  removed rule only. Session rows are kept, so a restore brings them back.

The web surface is the "Hidden from rails" section under Settings → Pages
(`web/components/settings/HiddenRailItems.tsx`, `useRailDismissals` +
`useRestoreRailItem`, which `DELETE`s the row). `invalidateRails` drops the
list, so a hide or restore anywhere refreshes it. Tests:
`crates/server/tests/rail_dismissals.rs`,
`web/tests/settings/hidden-rail-items.test.tsx`.
