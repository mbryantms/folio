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

`has_notes` / `has_bookmarks` / `has_highlights` are boolean fields
(`is_true` / `is_false`, no value) compiled to
`[NOT] EXISTS (SELECT 1 FROM markers m JOIN issues mi … WHERE m.user_id =
<viewer> AND m.kind = '<kind>' AND …)`. Markers are private, so the probe
is **always** scoped to the viewing user — another user's notes never
make a row match, and a shared system view evaluates per viewer. Markers
on removed issues don't count. `favorite` markers have no filter. The
series-level probe is served by `markers_user_series_kind_idx`
(`m20270307`).

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

## Not (yet) covered

- OPDS feeds only expose `filter_series` views; issue views don't appear
  in `/opds/*/views`.
- The issue-view detail page has no multi-select toolbar (the series
  view's bulk actions are series-shaped).
