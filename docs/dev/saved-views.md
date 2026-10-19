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
| `read_progress`, `last_read`, `read_count`, `unread_issues`, `collection_completeness`, `metadata_completeness` | per-series rollups | — |

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
  `(sort value, series.id)`.
- `compile_issues(&IssueCompileInput)` — the `IssueSummaryView` card
  columns (never `comic_info_raw` / `pages`) plus `series_slug` /
  `series_name`, `FROM issues JOIN series`, active + not-removed issues
  only, library ACL on `issues.library_id` and the WP-2.7 age-rating cap
  on the issue's effective rating. Sorts: `name` (series name, then issue
  number, then id), `year`, `created_at`, `updated_at`; `last_read` /
  `read_progress` are series-only (`SortNotAvailable`, 422). Paginated
  with an opaque **keyset** cursor (`IssueCursor`: base64 JSON of the last
  returned row's sort-key values + issue id). The "after" predicate is
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

## Save-as-view from the library grid

`web/components/library/libraryGridStateToFilterState.ts` translates the
grid's facet state into a builder seed. Series mode seeds a
`filter_series` view, issues mode a `filter_issues` view; the rating and
read-status chips carry over as `rating between` / `read_status in`.
Facets with no equivalent on the target entity (metadata completeness in
issues mode) are reported as dropped and toasted.

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

## Not (yet) covered

- The issue-view detail page has no multi-select toolbar (the series
  view's bulk actions are series-shaped).
