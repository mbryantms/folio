# UX and Architecture Improvement Plan

**Date:** 2026-06-02
**Status:** Draft for review, updated with product decisions
**Scope:** Follow-up plan for the UX, feature quality, frontend architecture,
backend API, performance, reliability, and polish audit.

This plan is intentionally incremental. It prioritizes fixes that improve
trust, remove dead-end flows, clarify user intent, and create reusable
frontend/backend contracts for future UX work. It does not propose a rewrite.

## Planning principles

- Fix objective defects before subjective product enhancements.
- Prefer accurate copy over unimplemented promises.
- Keep UI behavior and API contracts aligned, especially for search,
  selection, progress, saved views, and admin status surfaces.
- Add backend metadata where it lets the frontend become simpler, faster, or
  more reliable.
- Treat large-list behavior as a product contract, not just a rendering detail.

## Product decisions

The following decisions are incorporated into this plan. No remaining product
clarifications are required before starting Phase 1.

1. **Offline changes:** Remove queuing language and keep explicit retry actions.
2. **Theme "System" option:** Implement system theming with SSR-safe hydration
   behavior.
3. **Bulk selection:** Add backend bulk operations for "all matching" and keep
   an explicit "Select loaded" action.
4. **Collection bulk mark behavior:** Label issue-only actions clearly.
5. **Search result scope:** Add category totals and cursor pagination.
6. **Bookmark/marker result cards:** Include page-region thumbnails.

## Phase 1: low-risk polish and consistency fixes

Goal: remove misleading states, broken actions, silent no-ops, and inconsistent
confirmation patterns without changing major contracts.

### 1. Correct offline status messaging

- **Audit item:** Offline changes are advertised as queued, but no queue exists.
- **Locations:** [QueryProvider.tsx](../../web/components/QueryProvider.tsx),
  [mutations/_core.ts](../../web/lib/api/mutations/_core.ts)
- **Plan:**
  - Remove queueing language from offline toast copy.
  - Ensure failed transient mutations continue to expose the existing retry
    action.
  - Add a regression test or component test for the offline toast copy if the
    existing test setup supports browser events.
- **Complexity:** Low
- **User impact:** High
- **Priority:** P1
- **Acceptance criteria:**
  - Going offline never claims work will be queued.
  - A failed mutation gives a clear retry path.
  - No product copy promises offline persistence.

### 2. Fix series "Read from beginning" routing

- **Audit item:** Series menu routes to `/read/{issueId}` while the reader route
  is slug-based.
- **Locations:** [SeriesSettingsMenu.tsx](../../web/app/[locale]/(library)/series/[slug]/SeriesSettingsMenu.tsx),
  [reader route](../../web/app/[locale]/read/[seriesSlug]/[issueSlug]/page.tsx),
  [urls.ts](../../web/lib/urls.ts)
- **Plan:**
  - Pass enough first-issue metadata into `SeriesSettingsMenu` to call the
    shared reader URL helper.
  - Add a focused URL/unit test covering the menu action.
- **Complexity:** Low
- **User impact:** High
- **Priority:** P1
- **Acceptance criteria:**
  - The action opens the first issue reader for a real series.
  - The generated URL matches the app's slug-based reader route.

### 3. Replace native confirmation for health validation

- **Audit item:** Admin health validation uses `window.confirm`.
- **Location:** [HealthIssuesTable.tsx](../../web/components/admin/library/HealthIssuesTable.tsx)
- **Plan:**
  - Replace browser confirm with the app's `AlertDialog`.
  - Include action scope, expected cost, and clear cancel/confirm buttons.
  - Keep existing mutation behavior and toasts.
- **Complexity:** Low
- **User impact:** Medium
- **Priority:** P2
- **Acceptance criteria:**
  - No native browser confirm appears for validation.
  - Dialog is keyboard accessible and styled consistently.
  - Existing validation mutation still fires only after confirmation.

### 4. Make CBL unmatched-entry bulk behavior explicit

- **Audit item:** Missing CBL entries can be selected and bulk mark can silently
  no-op.
- **Location:** [CblViewDetail.tsx](../../web/components/saved-views/CblViewDetail.tsx)
- **Plan:**
  - Disable mark read/unread when selected rows contain no matched issue IDs,
    or show an explanatory toast.
  - For mixed selections, report matched count and skipped missing count.
- **Complexity:** Low
- **User impact:** Medium
- **Priority:** P2
- **Acceptance criteria:**
  - Every enabled bulk action produces a visible result.
  - Missing-only selections do not silently return.
  - Mixed selections communicate skipped missing rows.

### 5. Tighten filter and settings validation feedback

- **Audit items:** Year filters parse invalid input permissively; settings and
  async filter lookups can fail silently.
- **Locations:** [use-grid-filters.ts](../../web/lib/library/use-grid-filters.ts),
  [MultiSelectEditor.tsx](../../web/components/filters/value-editors/MultiSelectEditor.tsx),
  [ReadingPrefs.tsx](../../web/components/settings/ReadingPrefs.tsx)
- **Plan:**
  - Require full numeric matches for year filters instead of `parseInt`
    partial matches.
  - Add explicit loading, empty, and error states to async multi-select
    editors.
  - Show inline validation or save feedback for activity threshold settings.
- **Complexity:** Low/Medium
- **User impact:** Medium
- **Priority:** P2
- **Acceptance criteria:**
  - Invalid year input is not silently normalized.
  - Remote option failures are distinguishable from "No results".
  - Settings inputs show invalid, saving, saved, or failed states.

### 6. Clarify capped search and marker copy

- **Audit items:** Search "View all" is capped; marker/bookmark category copy is
  inconsistent and marker cards are unfinished.
- **Locations:** [SearchView.tsx](../../web/app/[locale]/(library)/search/SearchView.tsx),
  [use-search.ts](../../web/lib/search/use-search.ts),
  [types.ts](../../web/lib/search/types.ts)
- **Plan:**
  - Rename capped CTAs and section labels to "Top results" until pagination
    ships.
  - Align copy so bookmarks/markers are either consistently included or
    intentionally hidden.
  - Add a marker card that shows issue, page, marker label/type, and a
    page-region thumbnail when the backend can provide enough crop metadata.
- **Complexity:** Medium
- **User impact:** Medium
- **Priority:** P2
- **Acceptance criteria:**
  - UI no longer implies exhaustive search unless it is exhaustive.
  - Marker/bookmark terminology is consistent across modal and page search.
  - Marker results no longer reuse the people grid.
  - Marker results show page-region thumbnails with a graceful fallback when a
    crop is unavailable.

### 7. Add load-more behavior to admin findings rails

- **Audit item:** Findings pages expose `next_cursor` but provide no way to
  continue.
- **Location:** [FindingsView.tsx](../../web/components/admin/findings/FindingsView.tsx)
- **Plan:**
  - Add cursor-driven "Load more" actions to health findings and scan runs.
  - Preserve filter state while loading more.
  - Keep "refine filters" guidance as secondary copy.
- **Complexity:** Low/Medium
- **User impact:** Medium
- **Priority:** P2
- **Acceptance criteria:**
  - Users can page through all available rows.
  - Loading, empty, and error states remain clear.
  - Existing filters continue to constrain pagination.

## Phase 2: feature-completion and flow improvements

Goal: finish flows that are technically present but incomplete, and add API
metadata that improves user confidence.

### 8. Implement SSR-safe system theming

- **Audit item:** The "System" theme currently resolves to dark instead of
  following OS preference.
- **Locations:** [theme.ts](../../web/lib/theme.ts),
  [ThemePicker.tsx](../../web/components/settings/ThemePicker.tsx),
  [ThemeProvider.tsx](../../web/components/ThemeProvider.tsx),
  [layout.tsx](../../web/app/layout.tsx),
  [globals.css](../../web/styles/globals.css)
- **Plan:**
  - Make the `system` preference resolve from `prefers-color-scheme` while
    preserving the user's selected theme cookie.
  - Add an SSR-safe initial theme strategy so the first paint and hydrated
    client state agree.
  - Update theme picker copy to describe the real behavior.
  - Add tests for explicit themes, system light, system dark, and cookie
    persistence.
- **Complexity:** Medium
- **User impact:** Medium
- **Priority:** P2
- **Acceptance criteria:**
  - Choosing "System" follows OS light/dark preference.
  - First paint does not flash through the wrong theme.
  - Hydration does not produce theme mismatch warnings.
  - Explicit light, dark, and amber selections still override OS preference.

### 9. Add server-computed series resume target

- **Audit item:** Series resume derives from the first 100 issues plus broad
  progress fetching.
- **Locations:** [series page](../../web/app/[locale]/(library)/series/[slug]/page.tsx),
  [series.rs](../../crates/server/src/api/series.rs),
  [progress.rs](../../crates/server/src/api/progress.rs)
- **Plan:**
  - Add a server-computed resume/next-readable field to series detail, or a
    dedicated `/series/{slug}/resume` endpoint.
  - Include reason metadata such as `next_unread`, `continue_started`,
    `all_read`, or `no_readable_issues`.
  - Update the series page to render the CTA from this server result.
  - Add backend tests for series with more than 100 issues.
- **Complexity:** Medium
- **User impact:** High
- **Priority:** P1
- **Acceptance criteria:**
  - Long series resume correctly beyond the first 100 issues.
  - The frontend no longer fetches all progress just to derive one CTA.
  - Empty/all-read/no-readable cases render intentional next actions.

### 10. Define bulk selection semantics across paginated surfaces

- **Audit item:** `selectAll()` selects only loaded items, but the UI can imply
  all matching items.
- **Locations:** [use-selection.ts](../../web/lib/selection/use-selection.ts),
  [multi-select.md](multi-select.md),
  [IssuesPanel.tsx](../../web/app/[locale]/(library)/series/[slug]/IssuesPanel.tsx),
  [CollectionViewDetail.tsx](../../web/components/saved-views/CollectionViewDetail.tsx),
  [FilterViewDetail.tsx](../../web/components/saved-views/FilterViewDetail.tsx)
- **Plan:**
  - Keep an explicit "Select loaded" action for currently materialized items.
  - Add "Select all matching" for full filtered result sets.
  - Add selection metadata to the toolbar: loaded count, total count when
    known, and selected count.
  - Add backend bulk-by-filter operations rather than client-walking every page.
  - Return structured result summaries so the UI can report updated, skipped,
    forbidden, and not-found counts.
- **Complexity:** Medium
- **User impact:** High
- **Priority:** P1
- **Acceptance criteria:**
  - Users can tell exactly what will be affected before running a bulk action.
  - "Select loaded" and "Select all matching" are distinct actions.
  - Infinite-list selection behavior is consistent across list surfaces.
  - Bulk action tests cover loaded-only and all-matching cases.

### 11. Complete search pagination and totals

- **Audit item:** Search is capped and single-page across categories.
- **Locations:** [use-search.ts](../../web/lib/search/use-search.ts),
  [series.rs](../../crates/server/src/api/series.rs),
  [issues.rs](../../crates/server/src/api/issues.rs),
  [markers.rs](../../crates/server/src/api/markers.rs),
  [people.rs](../../crates/server/src/api/people.rs)
- **Plan:**
  - Add category-level totals and cursors to search responses.
  - Preserve ranking stability across pages.
  - Update search page to support loading more per category.
  - Keep search modal lightweight by showing top results plus a route to the
    full page.
- **Complexity:** Medium/High
- **User impact:** High
- **Priority:** P1
- **Acceptance criteria:**
  - Search page can reach all matches in each category.
  - Counts distinguish total matches from currently loaded results.
  - Modal and full-page search use consistent result contracts.

### 12. Improve account/settings capability metadata

- **Audit item:** Some settings infer mutability from failed requests rather
  than explicit capability metadata.
- **Location:** [AccountForm.tsx](../../web/components/settings/AccountForm.tsx)
- **Plan:**
  - Extend the account/me response with capability flags such as
    `email_editable`.
  - Render disabled controls and explanatory copy before the user attempts an
    impossible edit.
- **Complexity:** Medium
- **User impact:** Medium
- **Priority:** P2
- **Acceptance criteria:**
  - OIDC/local-account differences are visible before submission.
  - Server still enforces permissions and returns structured errors.

### 13. Align collection bulk progress behavior

- **Audit item:** Collection bulk mark handles issue IDs while mixed collection
  entries can include series.
- **Location:** [CollectionViewDetail.tsx](../../web/components/saved-views/CollectionViewDetail.tsx)
- **Plan:**
  - Rename actions so they clearly communicate issue-only behavior.
  - Show skipped series count for mixed selections.
  - Disable issue-only progress actions when no selected collection entries are
    issues.
- **Complexity:** Low
- **User impact:** Medium
- **Priority:** P2
- **Acceptance criteria:**
  - The action label matches actual affected entities.
  - Series entries are clearly skipped rather than silently ignored.

## Phase 3: architectural modernization

Goal: reduce future UX complexity by strengthening shared contracts and
components.

### 14. Unify saved-view result contracts

- **Audit item:** Generic saved-view results can return empty stubs for valid
  CBL and collection view kinds.
- **Locations:** [saved_views.rs](../../crates/server/src/api/saved_views.rs),
  [collections.rs](../../crates/server/src/api/collections.rs),
  [queries.ts](../../web/lib/api/queries.ts),
  [SavedViewRail.tsx](../../web/components/saved-views/SavedViewRail.tsx)
- **Plan:**
  - Define a polymorphic saved-view results envelope that distinguishes filter,
    collection, and CBL results.
  - Alternatively, remove or hard-error unsupported generic result paths.
  - Update docs and generated OpenAPI types.
  - Migrate frontend callers to the explicit contract.
- **Complexity:** Medium
- **User impact:** Medium
- **Priority:** P2
- **Acceptance criteria:**
  - No valid saved-view kind returns an accidental empty stub.
  - Frontend dispatch does not depend on hidden endpoint exceptions.
  - Contract tests cover all saved-view kinds.

### 15. Strengthen the shared DataTable primitive

- **Audit item:** DataTable wires sorting state without exposing operable sort
  controls.
- **Location:** [data-table.tsx](../../web/components/ui/data-table.tsx)
- **Plan:**
  - Add sortable header affordances, `aria-sort`, keyboard support, and visual
    indicators.
  - Add optional pagination/loading/empty/error slots.
  - Document when callers should use DataTable versus custom dense lists.
- **Complexity:** Medium
- **User impact:** Medium
- **Priority:** P2
- **Acceptance criteria:**
  - Sortable columns are discoverable and accessible.
  - Non-sortable tables do not expose misleading state.
  - Existing admin tables render consistently after migration.

### 16. Standardize form validation and save feedback

- **Audit item:** Validation and save feedback vary across filters, settings,
  and edit forms.
- **Locations:** [ReadingPrefs.tsx](../../web/components/settings/ReadingPrefs.tsx),
  [IssueActions.tsx](../../web/app/[locale]/(library)/series/[slug]/issues/[issueSlug]/IssueActions.tsx),
  [SeriesEditDrawer.tsx](../../web/app/[locale]/(library)/series/[slug]/SeriesEditDrawer.tsx),
  [filter-builder.tsx](../../web/components/filters/filter-builder.tsx)
- **Plan:**
  - Establish a common pattern for invalid, dirty, saving, saved, and failed
    states.
  - Prefer shared schema validation where forms already use structured data.
  - Avoid silent `onBlur` commits unless the field shows save state.
- **Complexity:** Medium
- **User impact:** Medium
- **Priority:** P2
- **Acceptance criteria:**
  - Similar forms communicate state in similar ways.
  - Invalid input is visible and recoverable.
  - Failed saves do not look successful.

## Phase 4: performance and scalability improvements

Goal: keep large libraries responsive without relying on eager client-side page
walking.

### 17. Move CBL filtering/search server-side

- **Audit item:** CBL detail can eagerly walk all pages for local search.
- **Location:** [CblViewDetail.tsx](../../web/components/saved-views/CblViewDetail.tsx)
- **Plan:**
  - Add server-side `q`, status, and matched/missing filters to the CBL window
    endpoint.
  - Preserve cursor pagination under active search.
  - Remove eager page walking from the frontend.
- **Complexity:** Medium
- **User impact:** Medium
- **Priority:** P3
- **Acceptance criteria:**
  - Searching a large CBL does not fetch every page.
  - Matched/missing filters are reflected in the URL or stable view state.

### 18. Replace full collection reorder arrays with move operations

- **Audit item:** Collection detail may load all pages to avoid wiping unloaded
  tail entries during reorder.
- **Location:** [CollectionViewDetail.tsx](../../web/components/saved-views/CollectionViewDetail.tsx)
- **Plan:**
  - Add backend move-before/move-after or rank-based reorder endpoints.
  - Update drag/drop UI to send local move operations.
  - Keep full reorder only as an admin/debug fallback if needed.
- **Complexity:** Medium/High
- **User impact:** Medium
- **Priority:** P3
- **Acceptance criteria:**
  - Reordering one item does not require all collection entries to be loaded.
  - Unloaded entries cannot be dropped accidentally from ordering.

### 19. Add per-library queue/status metadata

- **Audit item:** Some admin library surfaces can show server-wide queue depth
  in a library-specific context.
- **Location:** [LibraryOverview.tsx](../../web/components/admin/library/LibraryOverview.tsx)
- **Plan:**
  - Either relabel existing queue depth as server-wide or add per-library queue
    counts to the API.
  - Prefer scoped metadata if library-specific operations are common.
- **Complexity:** Low for label, Medium for scoped API
- **User impact:** Low/Medium
- **Priority:** P3
- **Acceptance criteria:**
  - Queue labels accurately describe their scope.
  - Per-library pages do not imply unrelated jobs belong to that library.

### 20. Add virtualization/pagination to large admin and library tables

- **Audit item:** Large lists rely on fixed limits, manual loading, or full
  client-side arrays.
- **Locations:** [FindingsView.tsx](../../web/components/admin/findings/FindingsView.tsx),
  [data-table.tsx](../../web/components/ui/data-table.tsx),
  [LibraryGridView.tsx](../../web/components/library/LibraryGridView.tsx)
- **Plan:**
  - Add virtualization where rows are dense and counts can grow high.
  - Prefer cursor pagination for search/result lists where stable ordering
    matters.
  - Add skeletons and background loading indicators for long-running fetches.
- **Complexity:** Medium
- **User impact:** Medium
- **Priority:** P3
- **Acceptance criteria:**
  - Large result sets remain responsive.
  - Loading more does not block existing visible content.
  - Keyboard navigation remains intact.

## Cross-cutting test plan

- **Frontend unit/component tests:** URL generation, selection toolbar labels,
  offline copy, validation states, CBL skipped-entry behavior, search labels,
  DataTable sorting.
- **Backend integration tests:** series resume beyond 100 issues, search totals
  and cursors, saved-view result contract per kind, per-library queue metadata
  if added.
- **Accessibility tests:** dialog focus management, sortable headers,
  keyboard-operable table headers, clear disabled states.
- **Regression fixtures:** long series, large CBL list, mixed collection,
  missing CBL entries, large search result category.

## Suggested delivery order

1. Phase 1 items 1-7 as a polish batch.
2. System theming, series resume API, and route fix verification.
3. Selection semantics and all-matching backend operations.
4. Search pagination, totals, and marker result thumbnails.
5. Saved-view contract cleanup.
6. DataTable and form-state refactors.
7. Performance-specific CBL, collection reorder, queue scope, and virtualization
   work.

## Review checklist

- Confirm whether Phase 1 should be delivered as one PR or several small PRs.
- Confirm whether search pagination should be category-specific endpoints or a
  combined global search endpoint.
- Confirm whether the system-theme work should be included in the Phase 1
  polish batch or delivered separately in Phase 2.
