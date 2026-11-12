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
They share one per-user `saved_view_preview` bucket (2/s, burst 20; see
`docs/architecture/rate-limits.md`).

## The DSL

`FilterDsl = { match_mode: all | any, conditions: [{ field, op, value }] }`
(`crates/server/src/views/dsl.rs`). `value`'s shape depends on the op:
a scalar for `equals` / `is` / `gt` …, `[lo, hi]` for `between`, an array
for `in` / `includes_any` …, a positive day count for `relative`, and
nothing for `is_empty` / `is_not_empty` / `is_true` / `is_false`.

Conditions are a flat list under one `match_mode`. `group_id` is on the
wire but always `0`: nested groups ("A and (B or C)") are not built yet,
and the id is reserved so they can land without a wire-format break.

### Operators

`Op` in `dsl.rs`; which ops a field takes is its registry `allowed_ops`
(mirrored by `allowedOps` in the web registry, labelled by `OP_LABELS`).
An unknown op fails when the request body is deserialized (422), before
the compiler runs; an op the field doesn't allow is 422 `filter_invalid`
from the compiler.

| Op | Builder label | Field kinds | SQL |
| --- | --- | --- | --- |
| `contains` / `not_contains` | contains / does not contain | text | `col [NOT] LIKE '%v%' ESCAPE '\'` |
| `starts_with` | starts with | text | `col LIKE 'v%' ESCAPE '\'` |
| `ends_with` (UX-8) | ends with | text | `col LIKE '%v' ESCAPE '\'` |
| `equals` / `not_equals` | equals / does not equal | text, number | `col = v` / `col <> v` |
| `is` / `is_not` | is / is not | enum | `col = v` / `col <> v` |
| `in` / `not_in` | is one of / is not one of | enum | `col [NOT] IN (…)` |
| `gt` / `lt` | > / < | number, date | comparison |
| `gte` / `lte` | ≥ / ≤ | number | comparison |
| `between` | between | number, date | `col >= lo AND col <= hi` |
| `before` / `after` | before / after | date | `col < v` / `col > v` |
| `relative` / `not_relative` | in last / not in last | date | see below |
| `includes_any` / `includes_all` / `excludes` | any of / all of / none of | multi | junction `EXISTS` / `NOT EXISTS` |
| `is_true` / `is_false` | is true / is false | bool | `col = true` / marker `EXISTS` |
| `is_empty` / `is_not_empty` | is empty / is not empty | nullable fields | see below |

The three text-pattern ops are offered on exactly the same fields (every
text field) and compile the same way: a plain, **case-sensitive** `LIKE`
on the raw column, the value bound as a parameter. The value matches
literally: `%`, `_` and `\` in it are escaped (`util::search::escape_like`)
under an explicit `ESCAPE '\'`, so `contains "X_Men"` never matches
"X-Men". (Before E9 of audit-2026-11 they were unescaped wildcards; a view
saved with `%` / `_` meant as wildcards now matches those characters
literally.) None of the columns these ops
read carries a trigram or `text_pattern_ops` index (the trigram indexes
sit on `series.normalized_name` and the people / entity lookup names), so
all three filter rows already narrowed by the library ACL, and `ends_with`
costs the same as `contains`.

### Size bounds

A filter that a request writes or previews is capped (`dsl.rs`, next to
`FilterDsl`):

| Constant | Cap | Counts |
| --- | --- | --- |
| `MAX_FILTER_CONDITIONS` | 50 | `conditions.len()`; the list is flat (no nested groups yet), and once groups land every leaf condition in every group counts against it |
| `MAX_FILTER_LIST_VALUES` | 200 | elements of one list-valued `value` (`in`, `includes_any`, …) |
| `MAX_FILTER_VALUE_LEN` | 200 bytes | one string value, or one string element of a list (the search endpoints' query cap) |

They are `garde` rules on `FilterDsl` / `Condition`, reached through
`Validated<T>` on `POST /me/saved-views`, `PATCH /me/saved-views/{id}`, the
two admin equivalents and both preview routes. A failure is a 422
`validation` whose `error.details` names the field (`filter.conditions`,
`filter.conditions[3].value`). On create the cross-field rules
(`validate_create`: name, kind, `filter` / `cbl_list_id` / `result_limit`)
run from the body's `garde::Validate` impl, so they and the bounds come back
in the same report. Malformed JSON stays 400. The web mirrors the caps in
`web/lib/saved-views/filter-bounds.ts`: the builder disables "Add condition"
at 50 and the multi-select stops at 200 values.

**Stored views are not re-checked.** A view written before the caps existed
still lists, opens and runs; reading one over them logs one WARN with the
view id and its counts (`FilterDsl::log_if_over_bounds`, called from
`saved_views::dsl_from_view` and `opds::dsl_from_view`, the two places a
stored filter is rebuilt). The statement timeout below is the backstop. To
enforce the caps on stored views later, reject in `log_if_over_bounds`.
A PATCH that only renames a view over the caps succeeds; one that sends a
new `filter` must fit them.

### The filter time box

Every query compiled from a filter runs through
`views::timebox::query_all`: a read-only transaction that issues
`SET LOCAL statement_timeout` (`FILTER_STATEMENT_TIMEOUT`, 5 s; `SET LOCAL`
so it ends with the transaction and never reaches the pooled connection),
runs the statements, and commits, or rolls back on any error. Postgres
cancels an overrun with SQLSTATE 57014, reported as
`FilterQueryError::TimedOut`. Callers and what they do on a timeout:

| Caller | On timeout |
| --- | --- |
| `GET …/results`, `GET …/issue-results` (rows and the first page's `total` in one transaction), both previews | 422 `filter_too_slow`, "This filter takes too long to run. Remove some conditions and try again." The web shows it inline: the builder's preview notice, the detail page's empty state, the home rail's empty state |
| Metadata bulk-fetch target expansion (`resolve_metadata_batch_targets`) | the same 422 |
| OPDS v1 / v2 `…/views/{id}` (series and issue views) | the feed is served empty with a WARN (a reader app has no error UI to show) |

Any other database error keeps its old answer (500, or the OPDS server
error). The timeout is `Config::filter_statement_timeout`, `#[serde(skip)]`
and not an operator knob; the test harness shortens it
(`TestApp::spawn_with_filter_statement_timeout`). Tests:
`crates/server/tests/saved_views_bounds.rs`. Home rails have no server-side
renderer: they call `…/results` / `…/issue-results` like the detail pages.

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

### `age_rating` compares ladder rungs

`is` / `is_not` / `in` / `not_in` on `age_rating` do not compare the text
as stored. A value that ranks on the age-rating ladder (a rung or one of
Metron's alias words, in any case, with surrounding ASCII whitespace)
stands for its whole rung: the compiler emits
`lower(btrim(col, <trim set>)) IN (<every spelling of the rungs named>)`
(`views::compile::rating_predicate` → `library::age_rating::filter_match`).
So a view saved on `Mature` matches `Mature 17+`, ` mature 17+ ` and
`Mature` alike. A value that does not rank (`Unknown`, `Rating Pending`,
off-ladder text) matches as written, exactly as before. The folding
happens at compile time; stored views are never rewritten. The negations
are `NOT (...)` of the same test, so an unrated (NULL) row stays excluded
from `is_not` / `not_in` as it was from `<>` / `NOT IN`. The list
endpoints' `?age_rating=` parameters (`GET /series`, `GET /issues`) and
the `/filter-options/age_ratings` lookup (one option per rung, labelled
with its canonical spelling; `?entity=issues` lists the issues' own
ratings) follow the same rule. The `entity=issues` lookup reads every
visible issue in scope each time the picker opens (no index covers it);
that cost is accepted for now.

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

### Collection completeness and specials (B9)

`collection_completeness` (`complete` / `incomplete` / `unknown`)
compares the owned count with `series.total_issues`. The owned count is
the active main run (`aic.main_count`, `special_type IS NULL` — point
issues included) plus, only when the **viewer** counts specials, the
series' tagged annuals / one-shots / specials / TPBs
(`aic.special_count`). Whether they count is
`COALESCE(cpo.count_specials, cpu.count_specials_in_completeness, FALSE)`:

- `cpu` — the caller's `users` row, LEFT JOINed once on its PK
  (`users.count_specials_in_completeness`, default false, the account
  setting from `PATCH /me/preferences`);
- `cpo` — the caller's `user_series_completion_prefs` row for the series,
  LEFT JOINed on the `(user_id, series_id)` PK (written by
  `PUT /me/series/{id}/completion-prefs`: `include` = true, `exclude` =
  false, `inherit` deletes the row).

Both joins are emitted only when a condition references
`collection_completeness` (`needs_completion_pref_join`), like the `aic`
aggregate — no per-row subquery. The same rule, resolved in Rust by
`api::series_completion_prefs::CompletionPrefs`, drives
`SeriesView.specials_count_toward_completion` (series cards and the
series page via `hydrate_series` / `get_one`; the web helper
`ownedCompletionCount` in `web/lib/series-status.ts` adds
`special_issue_count` when it is true) and the collection report's
`counted_owned` / `completeness_state` / `completeness_pct`
(`GET /series/{slug}/collection`; `total_owned` stays every owned issue).
A shared system view evaluates per viewer, so two users can get different
"Incomplete collections" results from the same view.

Surfaces deliberately left out: the reading-progress rollups
(`read_status`, `unread_issues`, "Caught up") count every active issue
and are about reading, not collecting; admin dashboards
(`/admin/stats`, the metadata coverage pages) have no per-user
completeness figure; OPDS shows no completeness of its own (its
saved-view feeds go through this compiler and so follow the setting).

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
| …0017 | system | New in series you're reading (`/me/new-in-series`, `m20271110_000001`) | — |
| …0020 | filter_issues | Recently finished issues (was "Recently finished" in v0.60.0; renamed by `m20271020_000002`) | ✓ |
| …0021 – …0027 | filter_issues | Finished this week, Rate what you read, Top rated issues, Annuals & specials, Short unread reads, Favorite issues, Annotated issues | — |
| …0028 – …0034 | filter_series | Jump back in, Almost done, Complete runs to start, Ended but unfinished, Top rated series, Incomplete collections, Favorite series | — |

`crates/server/tests/saved_views.rs::every_builtin_filter_template_resolves`
runs every filter template for a fresh user, so a DSL / sort that drifts
from the registry fails CI rather than the home page. Adding a template:
append to `TEMPLATES` in `m20271020_000001_builtin_views_wave2` (or a new
migration), bump that test's expected count.

### New in series you're reading

`GET /me/new-in-series` (`rails::new_in_series`): issues whose
`issues.created_at` (the scanner's ingest stamp, as for New issues) falls
in the last `NEW_IN_SERIES_WINDOW_DAYS` (30) days to a series the caller
was already reading at the time, that the caller has not finished. "Already
reading" means `created_at` is after the series' start, taken per series as
`MIN(COALESCE(p.finished_at, p.updated_at))` over the caller's reading rows
(finished, or read past page 0); no start time is stored. `finished_at` is
stamped once when a row flips to finished and kept while it stays finished,
so a whole series added at once and then started never counts as new.
Known limits: a reader whose only progress in the series is one unfinished
issue has a start that moves with every read (its `updated_at`), so an
issue added while they are still on it can be missed; and restarting a
finished issue clears its `finished_at`, so when that was their only
finished issue the start moves to the re-read. Newest addition first, tie-break on id; cursor-paginated on
`(created_at, id)` (`limit` default 24, max 100, `total` on the first
page). Removed issues and series, issues outside the caller's libraries
or over the age cap, and series the caller hid from the rails (On Deck's
auto-restore: newer progress in the series un-hides it) are left out.
One query: the started-series CTE over the caller's `progress_records`,
the issue side on `issues_active_created_idx` inside the window, the
finished check on the `progress_records` primary key. Opt-in like the
other recent rails; the home rail shows the first page, the "see all"
page walks the cursor.

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

### Fetching reading lists

A URL list is fetched by `cbl::fetch::fetch_cbl_url` at import and on a
forced refresh, and by `fetch_cbl_url_conditional` on refresh and "Check
for updates", which send the stored ETag and Last-Modified as
`If-None-Match` / `If-Modified-Since`. Both go through the same code
(`util::ssrf::fetch_public`). It accepts only `https://`, resolves the
host once and refuses the URL when any address is internal (the ranges
are listed on `util::ssrf::is_internal_ip`), connects to the address it
vetted, follows at most 2 redirects by hand with each target vetted
again, caps the body at 4 MiB while streaming, and gives the whole fetch
one deadline on top of the 30 s per-step timeout. A `304` is "not
modified" only when validators were sent. A refused URL gets a generic
message that names neither the address nor the range; the address is
logged at DEBUG.

**Proxies.** Reading lists fetched from a user-supplied URL always
connect directly and ignore `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`,
so the address check is the last word. A server that reaches the
internet only through a proxy therefore cannot import reading lists by
URL. Catalog lists and provider fetches (covers, metadata APIs) honour
the proxy environment; behind a proxy, the proxy performs the final name
resolution.

Catalog lists talk to fixed GitHub hosts (only the repository is
admin-configured), so names are not vetted: the client uses the default
resolver and the proxy environment, follows at most 2 redirects, each
required to stay on https and refused when its host is an internal
address literal, bounds each request at 30 s including the body, and
reads the tree listing (16 MiB) and blob (4 MiB) with a running cap that
also refuses an oversize `Content-Length` before reading.

**Time budgets** (`cbl::fetch`). Refresh, check, their `-all` variants
and URL or catalog import run under the 60 s JSON route timeout, so they
use `REQUEST_BUDGET`: 40 s per fetch and 40 s for one list's remote work,
returning their own timeout error. `refresh-all` / `check-all` give each
list at most 15 s (`CBL_REQUEST_BULK_LIST_BUDGET`), work through the
lists in random order so the same stalling hosts cannot hold the others
back on every run, stop starting lists after 45 s
(`CBL_REQUEST_BULK_BUDGET`, or when less than 5 s is left) and report the
rest as "not attempted"; results keep the lists' own order. Matching a
large list after a slow fetch can still take a request past the route
timeout: the write is then rolled back and the request answers 408,
while lists finished earlier in the same bulk request stay updated. The
hourly sweep
uses `SWEEP_BUDGET` (60 s per fetch, 120 s per list), visits due lists
in random order and starts no new list after 30 minutes
(`CBL_SWEEP_BUDGET`); lists it did not reach stay due for the next run.

**Atomic writes.** The fetch happens first. Replacing the entries,
updating the list row (and URL validators) and writing the refresh-log
row then commit in one transaction, as does creating a new list. An
error or a dropped request at any step leaves the stored list, its
entries and its log exactly as before.

A list may have at most 5000 entries (`cbl::MAX_BOOKS_PER_LIST`). The
cap applies at import and to new content on refresh and check: a source
whose content has changed and is over the cap is refused with the
import's `413 too_many_entries` ("exceeds 5000 entries") before anything
is written. Stored content that has not changed (an upload, a `304`, an
unchanged catalog blob or an identical body digest) is not re-checked,
so a list already over the cap keeps re-matching until its source
changes. The bulk check and refresh report a refusal as that list's
error. The scheduled sweep only logs failures (WARN); they are not shown
in the app.

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
Select sits before the cover-size icon in the title row and in the phone
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
Continue reading feeds; series: On Deck, Random picks, Similar series,
New in series; cbl: On Deck). A row the rails already ignore — the caller has progress
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
The rails themselves (Continue reading, Recently read, On this day,
Recently added, On Deck series) also skip issues of a removed series, so
nothing a rail shows is undismissable.

## Paging the progress rails

`/me/continue-reading`, `/me/recent-history` and `/me/on-deck` take
`?cursor=&limit=` (default 24, max 100) and return `next_cursor`; `total`
rides the first page of a request that names `limit`. A request with
neither parameter is the home-rail preview: the first 24 cards in one
query path and no `total` (On deck also returns no cursor there). The
"see all" pages walk the cursor with `useInfiniteQuery` +
`<InfiniteSentinel>`.

- **Continue reading / Recently read** keyset on
  `(progress.updated_at, issue_id)` descending inside the one SQL query
  (`progress_cards`); `total` is `COUNT(*) OVER ()` on that query. A card
  whose progress moves while someone pages jumps to the head: one already
  listed is not listed again, one not yet reached is skipped by that walk.
- **On deck** is merged and ranked in Rust (series walk + reading-list
  frontier), so a paged request recomputes the merge over a bounded set —
  the `ON_DECK_PAGED_CANDIDATES` (200) most recently active series and as
  many reading lists — and keysets over the ranked list (activity
  descending, reading-list cards before series cards, issue id). `total`
  counts that set; `bounded_to` is set on the first page when either
  source hit the bound, and the page says so. The preview, the OPDS On
  Deck feeds and the next-up fallback keep the rail's 40 candidates.

## Server prefetch + HydrationBoundary (the Home page)

The system Home is the app's one server-prefetched TanStack page (D18).
Without it the browser ran a request chain after hydration — pinned views,
then each rail's data — before the first rail could paint. Now
`app/[locale]/(library)/page.tsx` fetches, in the same render that already
read `/me/pages`, the page's pinned views (plus the library list for the
no-library card) and then the **first rail** only, and hands them to the
client through `<PrefetchBoundary>` (`components/PrefetchBoundary.tsx`, a
`<HydrationBoundary>`). The rail renders in the server HTML and its covers
start loading before any `/api` request.

How to prefetch for a page:

- **Same keys, same shape.** Key, path and response transform live once in
  `lib/api/query-options.ts` (`xOptions(fetch, …)`); the client hook spreads
  them with `jsonFetch`, the server calls them with `apiGet`. A prefetch
  that builds its own key or skips the hook's transform (e.g. `useMePages`
  unwraps `.items`) is just a wasted server request. Infinite hooks
  prefetch with `prefetchInfiniteQuery` and the same `initialPageParam`
  (they share the options object, so they cannot differ). Rail parameters
  (`RAIL_PREVIEW_LIMIT`, `CBL_RAIL_WINDOW`) live in
  `lib/saved-views/rail-queries.ts` for the same reason.
- **One `QueryClient` per request** (`createServerQueryClient()`), never at
  module level: the data is the signed-in user's. `apiGet` sends that
  request's cookies with `cache: "no-store"` (no cross-user caching; the
  app does not enable Cache Components, so nothing else caches it).
- **Best effort, bounded.** `prefetchPageRails` never throws; a failed,
  401 or unfinished query is simply not dehydrated (`dehydrate` keeps
  `success` only) and the client fetches it as before. At
  `PAGE_RAILS_PREFETCH_BUDGET_MS` (600 ms) the whole prefetch stops: the
  requests still in flight are aborted (`apiGet` takes an `AbortSignal`)
  and no further one starts, so a slow rail can neither slow the page nor
  keep calling the API after the response has gone. On loopback the chain
  (pinned views, then the first rail, the library list in parallel) costs
  about 20 ms.
- **The first rail that shows is the priority one.** A leading system
  rail that loads empty hides itself, so the prefetch walks on to the next
  rail and returns its id (`priorityViewId`); `PageRails` renders that
  rail eagerly with priority covers and mounts the empty ones before it
  (they render nothing). Without a prefetched rail the first rail is the
  priority one, as before. One degraded case is accepted: when the
  browser holds newer data than the snapshot (or refuses a replayed
  snapshot) and a rail the server judged empty now has cards, that rail
  shows above the priority one with lazy covers. The layout is right; only
  the eager covers sit one rail lower than they ideally would.
- **Why `PrefetchBoundary`, not the bare boundary.** The topbar search
  (`useSearchActions`, `lib/search/use-search-actions.ts`) calls
  `useContinueReading({ enabled: active && !!me.data })`, so that cache
  entry exists, empty, before the page renders. The stock boundary
  hydrates *existing* entries only in an effect, so the prefetched
  Continue reading rail rendered its skeleton on the server and refetched
  on the client. `PrefetchBoundary` hydrates an existing entry during
  render when it has no data and no request in flight. (The page list is
  different: the topbar's `useSearch` calls `useMePages()` enabled, so on
  every full page load the shell requests `/me/pages` itself before the
  page renders; the prefetched copy only matters on client navigations.)
- **Old snapshots are not hydrated.** Next replays a page's cached RSC
  payload on back/forward (the reader exits with `router.back()`). Each
  render carries a `snapshotId` (`lib/saved-views/snapshot-id.ts`: the
  server's render time plus a random part); the browser remembers when it
  first saw each id (its own clock, a bounded module-level map) and a
  snapshot first seen more than `SNAPSHOT_FRESH_MS` (30 s) ago hydrates
  nothing, so the hooks fetch as they did before the prefetch existed
  instead of painting the pre-reading Continue reading. An id the tab has
  forgotten is refused too when its render time is well behind the newest
  render the tab has seen (server time against server time), so an old
  page replayed from history can never pass for a new one.
- **One clock.** A fresh snapshot's queries are restamped to the moment
  this browser first saw it, so staleness and TanStack's newer-than-cache
  test never compare the server's clock with the browser's. An optimistic
  update in flight when Home opens can be replaced by the snapshot (it was
  fetched before the write landed); the mutation's `invalidateRails` on
  settle refetches it.
- **CSP.** The dehydrated state travels as a prop in the RSC payload; no
  inline script is added.

When not to: below-the-fold rails stay client-fetched on purpose
(`LazyRail`), and so does On this day (keyed by the browser's local date).
The card badges need no request of their own: every issue-card list
embeds the viewer's progress on each card (`IssueSummaryView.progress`),
and the progress rails carry their own, so no home rail requests the
unbounded `/progress` map (see
[reading-progress.md](reading-progress.md#issue-card-badges) for the
surface list). A prefetch only pays where the
first paint waits on a client request chain; data the shell already
fetches, or that is cheap and below the fold, is better left to the client.
A page that is reached mostly by client navigation with a warm cache pays
the server fetch for little gain.

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
  A manual match to such an issue (or one outside the grant) answers
  like an unknown id, and the create response reads its counts through
  the same `stats_for`.
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
