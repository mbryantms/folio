# Search

How Folio's search surfaces are wired across the modal, the inline
topbar dropdown, the dedicated `/search` page, and the per-page
narrow-scope inputs. Plan archive:
`~/.claude/plans/noble-nibbling-kahan.md`.

## What ships today

| Surface                       | Categories                        | Snippet | Sort + facets | Commands |
|-------------------------------|-----------------------------------|---------|---------------|----------|
| Topbar inline (`sm+` desktop) | series · issues · markers · people | ✓       | —             | `>` prefix |
| Topbar modal (`<sm` mobile)   | series · issues · markers · people | ✓       | —             | `>` prefix |
| `/search?q=…` rails           | all 4, hides empty                | ✓       | —             | —        |
| `/search?category=series`     | series                            | ✓       | Sort + filter sheet | —        |
| `/search?category=…` (others) | one category                      | ✓       | —             | —        |
| Series detail (`?q=`)         | issues, series-scoped             | ✓       | sort + order  | —        |
| CBL detail                    | entries, list-scoped (client filter + auto-walk) | — | hide-missing  | — |
| `/bookmarks`                  | markers, kind/tag chips + page-local q | —  | kind/tag      | —        |
| `/creators/<slug>`            | series rails per role             | —       | —             | —        |

## Backend endpoints

| Endpoint                              | Hit type             | Notes                                       |
|---------------------------------------|----------------------|---------------------------------------------|
| `GET /series?q=…`                     | `SeriesView`         | `ts_rank_cd` over `search_doc`; `?sort=` overrides relevance. `?credits=<name>` any-role filter (M1 polish for the multi-role click-through). |
| `GET /issues/search?q=…`              | `IssueSearchHit`     | `?series_id=` narrows to one series.        |
| `GET /me/markers/search?q=…`          | `MarkerSearchHit`    | Per-caller via `/me/…` route. ILIKE on `body` + `selection->>'text'`. |
| `GET /people?q=…`                     | `PersonHit`          | Trigram similarity + ILIKE; LEFT JOIN `person` for slug. |
| `GET /creators/{slug}`                | `CreatorDetailView`  | Per-role series rails. ACL filters credits by visible library in both UNION arms. |
| `GET /creators/resolve?name=…`        | `{ slug }` or 404    | Name → slug lookup. Used by the `/creators/by-name/<name>` redirect from credit pills. |

All snippet-producing endpoints emit `<mark>…</mark>`-wrapped excerpts
via `ts_headline('simple', …, 'MaxFragments=1, MaxWords=18, …')`. The
client sanitises via `web/lib/search/render-snippet.ts` — tokenising
regex strips everything but the literal `<mark>` allowlist, escapes
the rest.

## Index map

| Table         | Search column      | Index                                  |
|---------------|--------------------|----------------------------------------|
| `series`      | `search_doc` (tsvector, generated) | GIN. Weights: name=A, publisher+year=B, summary=D. |
|               | `normalized_name`  | `gin_trgm_ops` (fuzzy fallback)        |
| `issues`      | `search_doc` (tsvector, generated) | GIN. Weights: title=A, number+characters+teams+locations=B, tags+story_arc+writer+penciller=C, summary=D. |
| `markers`     | `body` + `selection->>'text'`      | No FTS index today — ILIKE under per-user filter is fast enough at typical scale. |
| `series_credits.person` / `issue_credits.person` | — | `gin_trgm_ops` for people search. |
| `person`      | `normalized_name`  | UNIQUE + B-tree.                       |
|               | `name`             | `gin_trgm_ops` (creator-page fuzzy lookup). |

## Frontend primitives

- `web/components/SearchModal.tsx` — mobile (`<sm`) modal. Same body
  shape as the inline dropdown.
- `web/components/TopbarSearchInline.tsx` — desktop (`sm+`) real
  `<input>` + popover panel.
- `web/lib/search/use-search.ts` — `useGlobalSearch` fans out to
  series / issues / markers / people in parallel; merges into
  `SearchGroups`. Optional `seriesFilters` forwards `/search` page
  sort + facet state into the series fetch only.
- `web/lib/search/render-snippet.ts` — sanitises backend snippets to
  the `<mark>` allowlist.
- `web/lib/search/use-recent-searches.ts` — `localStorage` ring
  buffer (`folio.search.recents.v1`), cap 8, MRU ordering, exposed
  helpers `appendRecent` / `removeRecent` for unit tests.
- `web/lib/search/actions-registry.ts` — static command-palette
  registry. `parseCommandPrefix` flips the modal/inline to
  command-only when input starts with `>`. Role-gated entries
  filtered out for non-admin users.
- `web/lib/search/series-search-filters.ts` — URL ↔ filter state for
  the `/search?category=series` facet sheet.

## URL surface map

- `/search` — empty-state rails of every category.
- `/search?q=…` — rails for non-empty categories only, each with a
  header "View all →" link to the category grid.
- `/search?q=…&category=series` — full series grid + sort dropdown
  + filter sheet. All filter state is URL-driven and deep-linkable.
- `/search?q=…&category=issues|markers|people` — category-specific
  grid. No facets today (series-only for now).
- `/creators/<slug>` — creator overview with per-role rails (capped
  at 12 cards each + "View all <N>" overflow).
- `/creators/<slug>?role=<role>` — single-role drill-in, full grid.
- `/creators/by-name/<encoded-name>` — name → slug redirect. Used by
  credit chips on series + issue detail pages. Falls back to
  `?library=all&credits=<name>` (legacy any-role library grid) when
  no `person` row exists yet for that name. Names with dots
  (`Brian K. Vaughan`) must encode the dot as `%2E` — Next.js
  routes treat a trailing `.<chunk>` as a file extension.
- `/series/<slug>?q=…` — issues panel filtered by `q`; debounced
  `history.replaceState` keeps the URL in sync without RSC re-render.

## `>` command prefix

Typing `>` as the first character of the modal or inline input
hides every content category and shows only the action registry.
27 entries today across Settings, Library, and Admin groups; admin
entries are filtered out for non-admin users.

Implementation: `parseCommandPrefix(raw)` returns
`{ needle, commandMode }`; the modal + inline both branch on
`commandMode` to drop content fetches and render an "Actions"
section instead of "Jump to…" + categories.

Gotcha: substring matching is literal. Plural-only labels miss
singular queries (`>library` doesn't substring-match "Manage
libraries"), so add the singular form as an explicit keyword on
new plural-labelled entries. See `admin-libraries` in the registry
for the established pattern.

## Conventions to preserve

- **Snippets are sanitised on the client.** Never render
  `dangerouslySetInnerHTML` on a search snippet without piping
  through `renderSearchSnippet`. The `<mark>` allowlist is the only
  HTML the panel accepts.
- **Library ACL is enforced server-side** in every endpoint —
  including the people + creator endpoints, where filtering happens
  in both UNION arms of the credits query.
- **Slug stability for creators**: the `person` table holds the
  canonical slug. Don't compute slugs client-side — use the
  `/creators/by-name/<name>` redirect so collision-suffixed slugs
  (`-2`, `-3`) work transparently.
- **Auto-walk-while-searching** is the right shape on bounded list
  surfaces (CBL detail). Use the `entriesQuery.fetchNextPage` loop
  in an effect keyed on the debounced query; without it the search
  silently lies about what matched once the user has scrolled past
  the first page.
- **Empty rails hide** on `/search`. New categories should follow
  the `enabled && count === 0 → null` pattern in
  `CategoryRail` so an unmatched category doesn't render a dead
  band.

## Out of scope (deferred follow-ups)

- **OCR global text search** (M7 of the plan, deliberately
  skipped). Would require a new `issue_ocr_page` table + per-page
  OCR worker + per-library opt-in admin toggle.
- **Sort + facets on issues / markers / people categories.** M4
  shipped series-only. Adding other categories needs backend
  `?sort=` handling per endpoint + the frontend filter-state
  module extended.
- **Person table re-sync after scan.** The backfill is a one-shot
  migration. New credits inserted by post-migration scans don't
  yet write back to `person`. The `/creators/by-name/<name>`
  redirect's `/?library=all&credits=…` fallback covers the
  user-facing gap.
- **Playwright E2E.** The Plan calls for an end-to-end suite
  covering modal open → type → enter, recents persistence, facet
  deep-link hydration, series-detail `?q=` round-trip, CBL
  auto-walk, command prefix, creator landing. Not yet wired into
  the harness — `tests/e2e/` is owned by Playwright per
  `vitest.config.ts` but the search-specific specs are TODO.
