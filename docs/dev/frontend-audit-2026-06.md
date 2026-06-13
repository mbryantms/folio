# Frontend Experience Audit — June 2026

Comprehensive audit of the Folio web frontend covering UX, UI, workflows,
interactivity, accessibility, mobile, performance, and architecture, from
four personas: first-time user, power user (10k-issue library),
administrator, and mobile/PWA user.

**Methodology.** Six parallel code-review passes over `web/` (routes/IA,
library workflows, reader, admin/settings/forms, accessibility + UI
consistency, performance + architecture — every claim verified against
source with file references), plus a live Playwright pass against the dev
server at desktop (1440×900) and phone (390×844) breakpoints covering
sign-in, home, series, issue, reader, navigation, and admin. Numbers
(line counts, query counts, bundle budgets) were measured, not estimated.

Severity scale: **Critical** = blocks a core journey or silently corrupts
state · **High** = major friction on a common path · **Medium** =
noticeable friction or correctness gap · **Low** = polish.

## Executive summary

~115 consolidated findings (from ~190 raw findings across six passes + the
live session). The codebase is unusually rigorous where it has conventions
(pagination integrity, provenance, undo, secrets, reduced-motion) — the gaps
cluster in **first-run experience**, **state persistence**, **scale
repetition**, **the reader's keyboard/mobile story**, and **theme/token
discipline**.

### Top 10 highest-impact improvements

1. **First-run setup flow** — role-aware empty states, scan-on-create
   default, "you're the admin" notice (A1). The product currently looks
   broken at minute one.
2. **Reader keyboard story via `inert`** — three one-line attributes remove
   hundreds of invisible tab stops; plus tap-zone labels and focus return
   (E1, E9, C13).
3. **Power-user daily loop on the grid** — read-status filter (B1),
   URL-persisted filter state (B2), multi-select (B3). Three changes that
   compound.
4. **Webtoon mode rescue** — aspect-ratio reservation + windowed rendering
   (C1) and an inline end-of-chapter footer for touch (C2).
5. **Semantic status tokens** — `--success/--warning/--info` + ~50-site
   sweep so light/amber themes actually work (F1, E5).
6. **Metadata matching at scale** — chips open the dialog; "Unmatched"
   worklist with auto-advance; completion feedback for bulk fetch (B4, B5).
7. **Perceived-performance trio** — parallelize SSR waterfalls (G3), bounded
   `/progress` (G2), React Compiler or memoized cards (G4).
8. **Fix the honest bugs** — health filters that can never match (D1),
   library-settings Reset corruption (D2), the un-routable session beacon
   (C10), CSP violation (I1).
9. **One search implementation on cmdk** — fixes combobox ARIA and deletes
   ~600 duplicated lines in one move (E2, F3).
10. **Mobile fundamentals** — bottom tab bar (A8), horizontal pan on
    overflowing pages (C4), 24-44px touch-target floor (E7, C11).

### Quick wins (<1 day each)

`inert` on reader overlays (E1) · `Promise.all` the issue/series SSR fetches
(G3) · `"warn"`→`"warning"` severity fix (D1a) · Reset-handler field mapping
(D2) · session beacon → `fetch keepalive` on `/api/` path (C10) · query
retry policy + scoped reconnect invalidation + gated ScanResultListener (G8)
· `aria-label`s on five search inputs (E5) · FilterPill `aria-pressed` (E5)
· Cover placeholder tokens (E5) · cancelled ≠ FAILED pill (D9) ·
`/admin/metadata` copy fix (A10) · scan-on-create default (A1, partial) ·
"Clear filters" buttons on empty states (A12) · delete dead
`UserNav`/`AuthConfigClient` (F4, D9) · `React.memo` the two card components
(G4, partial) · reader `error.tsx` (H4) · finished-issue reopens at page 0
(C6) · webtoon `aspect-ratio` (C1, partial — big payoff alone).

### Medium efforts (1–5 days)

Read-status filter in FilterSheet (B1) · grid URL write-back + scroll
restore (B2, B15) · grid/search multi-select (B3) · virtualize grid +
IssuesPanel (G1) · webtoon windowing + inline next-chapter (C1, C2) ·
dynamic-import the four heavy dialogs + MarkerEditor (G6) · cmdk search
consolidation (E2/F3) · status-token sweep (F1) · bottom tab bar (A8) ·
unmatched-metadata worklist (B4) · global audit table restoration (D4) ·
field-level server validation errors (D3) · unsaved-changes guard (D6) ·
scoped+debounced WS invalidation (G5) · OPDS self-serve panel (A9) ·
A–Z jump rail (B9) · `queryKeys` extraction + first mutation shards (H1) ·
reader first-run overlay + marker-mode pill (C5, C7) · double-tap/desktop
zoom (C9).

### Large initiatives (>1 week)

Full onboarding flow (library setup → scan → provider → first read) with
smart empty states across all surfaces · offline reading (service-worker
page caching + downloaded-issues UI) · design-system completion (semantic
tokens everywhere, component consolidation, light-theme certification) ·
form-stack standardization on RHF+zod with envelope-driven field errors ·
server-side job control (cancellable scans, failed-job retry, backfills as
real jobs, notification channels) · i18n decision executed in either
direction · search 2.0 (typo tolerance, facets everywhere, entity palette).

---

## A. First-run experience & information architecture

**A1. [Critical] [UX] First-run is a dead end: a new admin is never told to create a library**
- *Problem:* After registering (the first user silently becomes admin — no
  UI ever says so outside an admin-only panel), the user lands on `/` where
  the system rails hide themselves when empty and the remaining rails show
  "Nothing matches yet." with no call-to-action
  (`web/components/saved-views/SavedViewRail.tsx:66-71,190-191`). The grid
  empty state (`LibraryGridView.tsx:414-439`) has no CTA either. The path to
  the fix — user-footer dropdown → Admin → Libraries → New library — is three
  undiscoverable hops, and the create dialog defaults **"Scan after creating"
  to off** (`NewLibraryDialog.tsx:49,181-186`), so even success can yield an
  empty library. No onboarding wizard/checklist exists anywhere.
- *Recommendation:* Role-aware home empty state: 0 libraries + admin → a
  setup card ("Create your first library" → scan → optional metadata
  provider). Flip scan-on-create default to on. One-time "you're the admin"
  notice post-registration. Put a "New library" button inside the admin
  empty state (the `EmptyState` component already takes an `action` slot).
- *Impact:* Highest-leverage change for new-install retention; today the
  product looks broken at minute one.

**A2. [High] [UX] No way back: Admin/Settings shells have no "Back to library" affordance**
- *Problem:* In `AdminShell` the only return path is the wordmark
  (`AdminShell.tsx:97-99`); the user-footer menu has Profile / Settings /
  Admin / Sign out but no Home/Library item; Settings cross-links to Admin
  but not vice-versa. Live check confirmed Admin is reachable *only* via the
  avatar dropdown.
- *Recommendation:* Pinned "← Back to library" at the top of admin and
  settings sidebars + a Library entry in the user-footer menu; "Settings"
  entry in admin nav.
- *Impact:* Removes the most common "where am I" moment in three-shell apps.

**A3. [High] [UX] Saved views / collections / CBL lists / pages / pins form a concept maze split across two contexts**
- *Problem:* Five overlapping concepts are managed in four places; filter-view
  and CBL creation live *only* under `/settings/views`
  (`AddViewButton.tsx:56-63` is the sole CBL import entry); `/views` redirects
  into settings while `/views/[id]` stays in the library; Want to Read is a
  collection but ships as a separate sidebar built-in. Users cannot predict
  whether "a Marvel reading list" is a view, collection, or page — nor where
  to create it.
- *Recommendation:* Make `/views` a real library-context index (filters, CBL,
  collections) with in-page create/import; keep settings for defaults and
  arrangement. Add "Import CBL" to `/collections`. One sentence of in-product
  copy distinguishing the three concepts.
- *Impact:* Single biggest IA cognitive-load reduction for returning users.

**A4. [High] [UX] Search affordance is invisible in Admin/Settings; palette can't find user content**
- *Problem:* `AdminShell` renders no search trigger (`MainShell.tsx:130-133`
  has one; admin doesn't) — `Mod+K` works there but nothing advertises it.
  The palette also can't find collections/saved-views/pages by name
  (`web/lib/search/types.ts:15` — series/issues/markers/people only).
- *Recommendation:* Render `TopbarSearchTrigger` in `AdminShell`; add a
  client-side palette source over the already-cached `useCollections` /
  `useSavedViews`.
- *Impact:* Makes `Mod+K` the universal navigator it pretends to be.

**A5. [Medium] [UX] Five surfaces named some flavor of "activity"**
- *Problem:* `/settings/activity` ("Activity", filed under *Reader*), `/log`
  ("Reading log"), `/log/activity` ("Activity report"), `/admin/activity`
  ("Server activity"), `/admin/findings` (labeled "Library activity" — the
  route slug doesn't match its label).
- *Recommendation:* Rename `/settings/activity` → "Reading stats" (own
  "Insights" group); `/admin/findings` → "Library events"; keep "Reading log"
  as the only user-side activity name.
- *Impact:* Users can answer "where do I see X" without trial and error.

**A6. [Medium] [Architecture] i18n is dead scaffolding with ongoing cost**
- *Problem:* `next-intl` is wired, every route nests under `[locale]`, yet
  zero `useTranslations()` calls exist; all strings are hardcoded;
  `web/messages/en.json` still contains "Comic Reader / Phase 0" placeholder
  content; nav builders thread an always-empty `localePrefix`. Sidebar
  active-state logic keys off English label strings
  (`MainSidebar.tsx:162-171`), which breaks the moment labels localize or
  users rename entries.
- *Recommendation:* Decide: delete the `[locale]` level + next-intl, or
  commit (extract strings, locale switcher, second locale). Either way, match
  sidebar active state on `kind`/`ref_id`, not labels.
- *Impact:* Removes a misleading affordance, a directory level on every
  route, and a latent active-state bug class.

**A7. [Medium] [UX] Breadcrumb / sidebar naming disagreement for the root**
- *Problem:* Series/issue crumbs say "Library" linking to `/`, which the
  sidebar calls "Home"; the actual all-series grid is `/?library=all`,
  labeled "All Libraries". Three names, two destinations, one crumb; crumbs
  never include the owning library.
- *Recommendation:* Label the root crumb "Home" (or point "Library" at
  `/?library=all`); insert the library segment when known.

**A8. [Medium] [Mobile] No bottom navigation in an install-promoted PWA**
- *Problem:* On phones all navigation is hamburger → sheet
  (`MainShell.tsx:66-107`) while the app actively pushes iOS install
  (`AddToHomeScreenBanner`). Installed-app users expect a thumb-reach tab bar.
  Live check: the mobile top bar is just hamburger + search; everything else
  is two taps away.
- *Recommendation:* Mobile-only bottom tab bar (Home / Search / Bookmarks /
  More→sheet) reusing existing safe-area patterns.
- *Impact:* Primary mobile navigation drops to one tap; matches PWA
  positioning.

**A9. [Medium] [Workflow] OPDS endpoints exist but the UI never shows the catalog URL**
- *Problem:* `/settings/api-tokens` is framed for OPDS clients and the
  palette even matches "opds"/"koreader" keywords, yet the actual feed URL is
  never displayed and there are no client recipes — for a flagship
  self-hosting feature (four personal feeds + KOReader/Komga shims).
- *Recommendation:* Show the catalog URL with copy button + 2-line recipes
  (Panels, KOReader, Chunky) on the app-passwords page.

**A10. [Medium] [UX] `/admin/metadata` and ProvidersTab signpost a settings page that doesn't exist**
- *Problem:* Page header says "Settings live under /admin/settings (filter:
  metadata.*)" (`(admin)/admin/metadata/page.tsx:9`) and ProvidersTab repeats
  it — there is no `/admin/settings` route; the settings are in the page's
  own Settings tab. Operators following the copy land on a 404.
- *Recommendation:* Fix the copy; delete the stale docstrings.

**A11. [Medium] [UX] `/creators/[slug]` is orphaned**
- *Problem:* Creator pages exist but the only inbound links are credit chips;
  no creators index, sidebar, or rail (M5 of the URL plan is unshipped).
- *Recommendation:* Minimal alphabetical, cursor-paginated `/creators` index
  linked from the search page's People rail.

**A12. [Low] [UX] Assorted IA polish**
- "Key binds" (settings nav) vs "Keyboard shortcuts" (menu/sheet) — pick one.
- "Bookmarks" page actually lists four marker kinds — consider "Markers"/"Bookmarks & notes".
- `/settings` hard-redirects to `/settings/reading`; account is the more
  common intent — redirect there or render an index.
- No Help/About surface for non-admins (version, docs link, what's-new).
- Dead code: `web/components/UserNav.tsx` is imported nowhere; the
  `placeholder: true` nav machinery has zero remaining users.
- Loading-skeleton shape mismatches: `/log` and `/search` inherit the series
  grid skeleton.
- Reading-log first-run renders zero-filled dashboards with no empty-state
  copy except ChronoFeed's.
- Filtered-empty states ("No series match these filters") lack a one-click
  "Clear filters" action.
- Series page "No active issues to read." doesn't branch finished-everything
  vs genuinely-empty.

---

## B. Library, search & curation workflows

**B1. [High] [Workflow] Read-status filtering is missing from the library grid**
- *Problem:* The grid facet state (`web/lib/library/use-grid-filters.ts:35-114`)
  covers 12 dimensions but not `read_status` / `read_progress` /
  `unread_issues` — those exist only in the saved-view builder. "Show me
  unread issues in this library", the most common curation filter, requires
  creating a persistent saved view (~8–10 interactions).
- *Recommendation:* Read-status pill row (Unread / In progress / Read) in
  `FilterSheet` backed by a `read_status` param; the server already supports
  it for views.
- *Impact:* Fixes the daily loop for every persona.

**B2. [High] [Workflow] Grid filter/sort/mode state is session-local — lost on reload, never shareable**
- *Problem:* `useLibraryGridFilters` seeds from the URL once, then owns state
  in `useState`; nothing writes back (`use-grid-filters.ts:25-27`). Sort and
  the series/issues mode toggle reset on every visit. The search page already
  does `history.replaceState` syncing (`SearchView.tsx:129-139`) — the
  pattern exists, the grid just doesn't use it. (Also surfaced independently
  by the architecture pass.)
- *Recommendation:* Write facet/sort/mode to the URL (debounced
  `router.replace`, `scroll:false`); persist mode/sort like
  `folio.libraryGrid.cardSize` already is.
- *Impact:* Back-button, refresh, and shareable filtered URLs all start
  working; highest-ROI workflow fix.

**B3. [High] [Workflow] No multi-select on the library grid, search results, or bookmarks**
- *Problem:* Selection is wired on exactly 4 surfaces (series IssuesPanel,
  filter/collection/CBL views). `LibraryGridView`, `SearchView`, and
  `MarkersList` have none — yet the cards already accept
  `selectMode`/`onEnterSelectMode` props. Filtering "all 2019 one-shots" on
  the grid cannot be bulk-marked read.
- *Recommendation:* Wire `useSelection` + `SelectionToolbar` into the grid
  (issues mode first; bulk hooks accept arbitrary ids), then search; bookmarks
  bulk-delete needs a small endpoint.
- *Impact:* Bulk curation works where users actually browse; removes the
  "checkboxes appear on some pages only" inconsistency.

**B4. [High] [Workflow] Metadata matching at scale is one-dialog-at-a-time; "needs metadata" chips never link to the fix**
- *Problem:* The per-item match dialog is excellent, but the "meta" chip on
  cards, "Needs metadata" badges, and the unmatched-count nav badge are all
  inert. Fixing 50 unmatched series = 50 × (series page → Actions → Fetch
  metadata → Match → review → apply).
- *Recommendation:* Make the chips open `MetadataMatchDialog` directly (it
  already takes a scope object); add an "Unmatched" worklist (grid filtered
  on `metadata_completeness=needs_metadata`) that auto-advances to the next
  item after each apply.
- *Impact:* The most painful scale workflow in the app becomes a loop instead
  of a maze.

**B5. [High] [Workflow] Bulk metadata fetch is fire-and-forget**
- *Problem:* Multi-select fetch dispatches N POSTs, toasts "Queued…", clears
  selection, and never reports completion or results
  (`IssuesPanel.tsx:236-277`); the user must remember to check an admin tab.
- *Recommendation:* Reuse the existing `useScanEvents` subscription to toast
  completion with a "Review results" action, or route through the batch
  endpoint the series menu already uses.

**B6. [High] [UX] Collection deletion is permanent while cheaper objects get undo**
- *Problem:* A hand-curated, ordered collection deletes behind a single
  confirm; marker deletes and collection-adds get undo. Asset value is
  inverted.
- *Recommendation:* Undo window via snapshot + recreate (bulk re-add exists),
  or type-the-name confirm for large collections.

**B7. [Medium] [UX] Search has no typo tolerance and (issues/markers/people) no facets**
- *Problem:* Series/issue search is strict FTS — "spiderman" misses
  "Spider-Man"; trigram similarity exists only for people. Facets exist only
  for `category=series`, and even those cover just year/status/publisher.
- *Recommendation:* Trigram fallback on zero FTS results (pg_trgm already
  installed); reuse the grid FilterSheet on series results; add read-status +
  series scoping to issue results.

**B8. [Medium] [UX] No keyboard navigation in any grid**
- *Problem:* No roving tabindex/arrow-key handling anywhere in
  `components/library/` — keyboard users Tab through every card; select mode
  has Shift+Click but no Shift+Arrow.
- *Recommendation:* One shared roving-tabindex grid hook (arrows move, Enter
  opens, Space toggles selection), applied to grid/IssuesPanel/MarkersList.

**B9. [Medium] [UX] No jump-to-letter on long alphabetical lists**
- *Problem:* Infinite scroll + name sort means reaching "S" in 800 series is
  scroll-only (scrollbar dragging is useless with cursor pagination).
- *Recommendation:* A–Z rail that maps to a server `name starts-with` filter
  (operator already exists in the field registry).

**B10. [Medium] [Workflow] CBL resolution is strictly one-entry-at-a-time**
- *Problem:* 50 ambiguous entries = 50 popover sessions; no auto-advance, no
  "apply this series to all similar entries", no bulk mark-missing.
- *Recommendation:* Auto-advance to next unresolved after a match; offer
  "use this series for N similar entries" when consecutive entries share a
  CBL series name.

**B11. [Medium] [Workflow] Bookmarks index: no bulk ops, no flat sort, no total count**
- *Problem:* Fixed series-grouped ordering, no multi-select, no count in the
  header (`MarkersList.tsx`).
- *Recommendation:* Grouped ⇄ flat-newest toggle; header count from the
  existing `useMarkerCount`; selection wiring.

**B12. [Medium] [UX] Single-field metadata edits require the full 30-field edit sheet**
- *Problem:* Fixing one typo'd title means the full 9-section sheet; the
  provenance table — where errors are noticed — has no inline edit.
- *Recommendation:* Click-to-edit popovers on provenance/details rows that
  PATCH one field; keep the sheet for batch edits.

**B13. [Medium] [Workflow] Provider quota exhaustion gives no ETA and no pre-flight warning**
- *Problem:* "Providers are out of quota — try again shortly." with no reset
  time; quota numbers live on an unlinked admin tab; nothing warns before a
  200-issue batch burns the budget.
- *Recommendation:* Include quota state in the search-kick response; render
  remaining/reset in the dialog; pre-flight unconfigured-provider notice.

**B14. [Medium] [Workflow] Auto-applied / weekly-refresh changes have no user-visible audit surface**
- *Problem:* Server-side auto-apply runs silently; no "what changed last
  Sunday" feed exists, so cautious operators leave automation off.
- *Recommendation:* "Recent applies" list on the metadata dashboard from
  existing audit/provenance data, filterable by run.

**B15. [Medium] [UX] Infinite-scroll position restore is incidental, not designed**
- *Problem:* Back-nav restore depends on TanStack cache lifetimes; after
  gcTime or grid state reset, users land at the top of page 1.
- *Recommendation:* sessionStorage scroll-offset + page-count per list key;
  raise gcTime for list queries; pairs with B2.

**B16. [Medium] [Mobile] Hover-gated card actions rely on an untaught long-press**
- *Problem:* Kebabs, quick-read overlays, and selection checkboxes are
  hover-revealed; touch users get a long-press sheet nothing advertises.
- *Recommendation:* Persistent compact kebab on coarse pointers + one-time
  hint.

**B17. [Low] [Workflow/UX] Smaller workflow gaps (each cheap to fix)**
- "Select all matching" exists only on IssuesPanel; Cmd+A elsewhere silently
  means "all loaded".
- Sort is disabled whenever a search query is active (no relevance override).
- Add-to-collection dialog has no recents/favorites shortcut.
- Marker tag autocomplete caps at 6 with no rename/merge management.
- No hover-preview cards (shadcn HoverCard is already in the tree).
- No copy-link/share affordance anywhere despite clean slug URLs.
- No "Recently added" default rail, no random/surprise-me pick.
- No export for reading log or collections (CBL lists export; nothing else).
- Issue dead-ends ("Cannot read — issue state: archived") offer no "continue
  with next readable issue" via the existing resume resolver.
- Error states lack retry buttons; "Loading more…" rows are unanimated text;
  Want-to-Read seeding race surfaces an error-ish toast on day one.
- Bulk archive edit reports "M skipped" without which or why, and clears the
  selection so the remainder can't be retried.

---

## C. Reader

**C1. [High] [Performance/Mobile] Webtoon mode renders every page with zero reserved height — no virtualization, broken resume, defeated lazy-loading**
- *Problem:* `WebtoonView` (Reader.tsx:1082-1222) mounts all pages; images
  have no `aspect-ratio` reservation even though `PageInfo.image_width/height`
  is available. Consequences: all pages fetch at once (0-height images all sit
  inside the lazy-load margin), resume-at-page-N lands wrong as layout shifts,
  the IntersectionObserver can persist a *regressed* progress page, and a
  200-page chapter mounts 200 `MarkerOverlay`s/ResizeObservers. (Confirmed
  independently by the performance pass.)
- *Recommendation:* Reserve `aspect-ratio: w/h` per page (alone fixes lazy
  loading + resume), then window rendering to ±N pages with sized
  placeholders — PageStrip already implements exactly this pattern.
- *Impact:* The longest-form format on the weakest devices stops degrading;
  fixes silent progress corruption.

**C2. [High] [Mobile/Workflow] Webtoon on touch has no path to the next issue**
- *Problem:* The end-of-issue card only triggers from `goNext()`; webtoon
  disables swipe and replaces tap zones with a chrome toggle. A phone reader
  scrolling to the bottom of a chapter hits nothing — no end card, no Up
  Next.
- *Recommendation:* Inline end-of-chapter footer after the last page (cover +
  "Read next" from the already-prefetched resolver data), or trigger the end
  card on last-page-visible + overscroll.
- *Impact:* Restores the binge loop for the dominant mobile format.

**C3. [High] [UX] Failed page loads have no error state and no retry**
- *Problem:* `PageImage.tsx:89` — `onError={() => setLoaded(true)}` hides the
  spinner and leaves a broken-image glyph; no message, retry, or backoff.
- *Recommendation:* Error state with "Tap to retry" (cache-busted re-set of
  `src`), one auto-retry, "Still loading…" hint after ~8s.

**C4. [High] [Mobile] Wide/overflowing pages can't be panned on touch — horizontal drags turn the page**
- *Problem:* Container sets `touch-action: pan-y pinch-zoom`; the swipe
  handler claims horizontal drags >30px. At fit=height/original the page
  overflows horizontally with no way to see the cropped region except
  pinch-zoom first. **Live-confirmed:** at 390×844 with fit=height the page
  renders cropped on both sides.
- *Recommendation:* When rendered width > viewport, allow native horizontal
  pan and require edge-proximity/velocity for page-turn swipes.

**C5. [High] [UX] Zero first-run guidance in the reader**
- *Problem:* Chrome starts hidden; no tap-zone overlay, no "press ? for
  shortcuts" hint, nothing teaches gestures, fit modes, or markers (also
  flagged by the IA pass). A stale comment even documents the wrong chrome
  key.
- *Recommendation:* One-time localStorage-keyed overlay (zone diagram +
  swipe/shortcut hints), dismissed on first interaction.

**C6. [Medium] [Workflow] Re-opening a finished issue resumes on the last page**
- *Problem:* `initialPage` ignores `finished` (read page.tsx:107-112) — a
  finished issue opens on its final page; next tap opens the end card.
- *Recommendation:* `finished && page >= last` → start at 0 (Komga behavior)
  or a one-shot "start over / resume at end?" prompt.

**C7. [Medium] [UX] Marker modes have no visible indicator or touch exit; editor discards drafts silently**
- *Problem:* Entering select-rect/image only changes the cursor (meaningless
  on touch) while navigation silently stops; exit is Esc or an undocumented
  micro-drag. Separately the marker editor sheet drops half-written notes on
  Esc/overlay-click with no dirty guard.
- *Recommendation:* Persistent mode pill with a ✕ Cancel whenever
  `markerMode !== "idle"`; dirty-state confirm or "Note discarded" undo
  toast.

**C8. [Medium] [UX] Spread handling trusts the ComicInfo flag and ignores actual page dimensions**
- *Problem:* `computeSpreadGroups` only honors `double_page === true`;
  flag-less landscape spreads (the common case) get squeezed into half-width
  panes — while `detectViewMode` already uses a dimension threshold.
- *Recommendation:* Treat aspect > 1.2 as solo-spread in the grouping walker,
  flag as override. Also: solo portrait pages in width-fit double mode render
  at full viewport width (2× neighbors) — cap at `max-w-[50vw]`.

**C9. [Medium] [UX] No zoom beyond native pinch**
- *Problem:* No double-tap zoom on touch; nothing at all on desktop (no
  Ctrl+wheel, keybind, or buttons). Reading small lettering means switching
  fit modes.
- *Recommendation:* Double-tap/double-click 2× transform zoom with
  drag-to-pan, `+`/`-`/`0` keybinds; keep the existing visualViewport guard
  pattern.

**C10. [Medium] [Workflow] Stats-feeding plumbing has three correctness holes**
- *Problem:* (a) the session final flush posts `sendBeacon("/me/reading-sessions")`
  — the route is mounted under `/api/`, so the beacon 404s into the Next
  fallback and can never succeed (session.ts:274); (b) one tab-hide
  permanently disarms the final flush while heartbeats keep running; (c) the
  progress writer snapshots the CSRF token once per mount and silently 403s
  forever after rotation, and the trailing debounced write is lost on tab
  close.
- *Recommendation:* Replace the beacon with `fetch(..., { keepalive: true })`
  + CSRF header on the `/api/` path; re-arm the flush on visibility-visible;
  read the CSRF cookie inside the write callback and flush pending writes on
  `pagehide`.
- *Impact:* Session end-times and resume positions stop quietly drifting for
  mobile readers.

**C11. [Medium] [Mobile] Touch-target and platform gaps in reader chrome**
- *Problem:* End-card close 28px, page pins 28px (delete hidden in a tooltip
  — effectively desktop-only), editor copy button 24px, tag-remove 12px;
  chrome icons 36px. The fullscreen button renders and silently no-ops on
  iPhone (no `requestFullscreen`).
- *Recommendation:* 44px hit areas via padding; route pin-delete through the
  editor; hide fullscreen when unsupported (suggest A2HS instead).

**C12. [Medium] [Performance] Webtoon re-render and prefetch churn**
- *Problem:* Scroll-driven `setPage` reconciles all N unmemoized
  `WebtoonPage`s per change; `useReaderPrefetch` also runs in webtoon mode,
  double-decoding pages whose `<img>`s are already in the DOM.
- *Recommendation:* `React.memo(WebtoonPage)`; skip the prefetch hook when
  `viewMode === "webtoon"`.

**C13. [Low] [UX] Keyboard & escape-layer polish**
- Space ignores modifiers (no Shift+Space = previous); no PageUp/PageDown;
  Space doesn't scroll-then-turn on overflowing fit-width pages.
- Esc exits the reader immediately rather than first closing chrome/strip
  (the end card already has the two-step).
- End-of-issue card steals focus on open but never restores it on dismiss.
- Brightness/sepia settings are per-tab only (not persisted like fit/view).
- Page-turn animation can't be changed from inside the reader settings
  popover.
- Peek banner and chrome contest the same top edge (both `fixed top-0 z-30`).
- Bookmark/favorite toggle logic is duplicated between Reader.tsx and
  ReaderChrome.tsx with divergent toast copy — extract shared hooks.
- Page-strip thumbnails pre-warm ±12 on every page change even when the
  strip is never opened.
- No blur-up placeholder despite the strip-variant thumbs being right there.
- Webtoon's full-viewport tap toggles chrome; no tap-bottom-to-advance zones.

---

## D. Admin & settings

**D1. [High] [UX] Health-issue filters are broken two ways**
- *Problem:* (a) The per-library severity pill filters on `"warn"` while the
  server emits `"warning"` (`HealthIssuesTable.tsx:32` vs `health.rs:308`) —
  the Warn pill always shows an empty table (the cross-library page gets it
  right). (b) The `resolved`/`dismissed` pills filter client-side over a
  fetch that excludes those rows by default — both permanently show 0, and a
  dismissed row vanishes with no way to find or un-dismiss it.
- *Recommendation:* Share one `SEVERITIES` constant; pass
  `include_resolved/include_dismissed` when those pills are active; add
  un-dismiss.
- *Impact:* Two of four state filters and one of three severity filters are
  dead UI on the triage surface; dismiss is silently irreversible.

**D2. [High] [UX] Library settings "Reset" silently corrupts half the form**
- *Problem:* The reset handler re-seeds only 9 of 15 fields
  (`LibrarySettingsForm.tsx:629-644`); the rest become `undefined` — switches
  flip off regardless of saved values; saving after Reset would disable
  auto-apply/filename heuristics the admin had on.
- *Recommendation:* Extract the `lib.data → FormValues` mapping used by the
  hydrate effect and call it from Reset. One-function fix.

**D3. [High] [Workflow] Server-side validation errors never reach the failing field**
- *Problem:* `_core.ts:90-97` reduces the 422 envelope to a toast string; no
  form maps server errors onto fields. Cross-field rules the client can't
  check (TTL ordering, OIDC completeness) surface as context-free toasts over
  six-card forms.
- *Recommendation:* Add `error.details: [{field, message}]` to the envelope,
  carry it through `ApiMutationError`, and a shared
  `applyServerErrors(form, err)` → `form.setError`.

**D4. [High] [Workflow] The global audit log lost its query tools**
- *Problem:* `/admin/audit` now redirects to a two-chip activity feed; the
  full `AuditTable` (action filter, actor filter, since filter, payload
  expansion, IP column) is only reachable pinned to a single user. "Every
  `library_access.set` this week" is unanswerable; timestamps in the feed
  show time-of-day only, even pages back (`formatTime`,
  `ActivityFeedClient.tsx:206-212`).
- *Recommendation:* Render the unpinned AuditTable when the audit chip is
  exclusive (or port its filters + payload expansion + dates into the feed).

**D5. [Medium] [Workflow] History surfaces silently truncate**
- *Problem:* Scan history renders the server's default 50 runs with no
  pagination or truncation notice (`ScanRunsTable.tsx:79`; the project's own
  no-silent-truncation rule); the metadata Runs tab supports a `before` param
  it never passes; the per-library health endpoint is unbounded and the
  table renders every row through an unvirtualized `DataTable`.
- *Recommendation:* Cursor pagination + load-older on both; move per-library
  health onto the existing infinite cross-library endpoint filtered by
  library.

**D6. [Medium] [Workflow] No unsaved-changes protection anywhere**
- *Problem:* Zero `beforeunload`/route guards; long admin forms sit one click
  from sibling tabs and discard silently.
- *Recommendation:* `useUnsavedChangesGuard(isDirty)` hook on the dirty-state
  forms.

**D7. [Medium] [UX] Mixed save models inside one form; stale-state and feedback bugs**
- *Problem:* The instant-apply thumbnail card renders inside the save-gated
  library settings `<form>` with identical styling; the email form stays
  dirty after a successful save (password lingers in the DOM);
  `/admin/server` cards seed `useState(initial)` once and never resync, so a
  second admin's change produces a phantom-dirty overwrite; removed-items
  optimistic hide never rolls back on error; one pending dismiss disables
  every Dismiss button.
- *Recommendation:* Move instant-apply controls out of the form with an
  "applies immediately" affordance; `form.reset` on save success; key cards
  on their settings snapshot (the `ProviderConfigForm` pattern); clear
  optimistic entries in `onError`; track pending per row id.

**D8. [Medium] [Workflow] Queue page can't manage the queue; scans can't actually be cancelled**
- *Problem:* The Queue page is a read-only depth display — clear lives only
  in a topbar beacon that disappears when the queue drains; no failed-job
  surface. "Cancel scan" only flips the DB row; a live worker finishes anyway
  (the dialog admits it). Browser-loop backfills (pHash, variant covers) die
  silently on navigation despite apalis existing.
- *Recommendation:* Per-queue clear + failed-jobs section on the Queue page;
  a cooperative cancellation flag checked between scan batches; server-side
  backfill jobs reporting via the existing WS/queue plumbing.

**D9. [Medium] [UX] Operator-facing copy/affordance bugs**
- Cancelled scans display as red **FAILED** (`statusFromRun` maps
  `cancelled → "failed"`).
- Two cron editors: library scan gets validation + humanization + next-runs;
  metadata weekly-refresh is a bare 6-field input with a buried
  restart-required caveat — and nothing anywhere tracks "saved but restart
  pending".
- Audit actor filter demands a raw UUID (no user autocomplete).
- "New users must be invited or provisioned by an admin" — no invite or
  create-user flow exists; the only path is temporarily re-opening
  registration.
- Provider "Test" is disabled until the provider is enabled — inverts the
  natural paste → test → enable order.
- Library overview shows the raw cron string as "Next scheduled scan"
  (`validateCron` already computes next runs).
- Dead `AuthConfigClient.tsx` component claims auth config is env-only — the
  opposite of shipped behavior; imported nowhere; delete it.
- Users table: no total count, pager shows even for one page.
- Dialog copy leaks schema names (`scan_runs`, `reading_sessions`) to
  operators.

**D10. [Medium] [Mobile] Admin-on-phone sharp edges**
- *Problem:* Foundation is good (sheet drawer, safe areas, pull-to-refresh),
  but the findings scan-runs table is a raw 7-column `<table>` with no
  overflow wrapper (every other table uses the wrapped primitive); expanded
  JSON `<pre>` creates double scroll traps; the queue beacon's clear button
  is a ~20px target.
- *Recommendation:* Use the shared Table primitive (or stacked cards under
  `sm:`); `max-w-full` on `<pre>`; 44px hit area on the beacon.

---

## E. Accessibility

**E1. [High] [Accessibility] Hidden reader surfaces stay in the tab order (aria-hidden over focusable content)**
- *Problem:* `ReaderChrome` (header, 6+ buttons), `PageStrip` (potentially
  *hundreds* of page-thumbnail buttons), `EndOfIssueCard`, and `TapZones`
  all hide via translate/`pointer-events-none`/`aria-hidden` while their
  focusable content remains tabbable — a WCAG 4.1.2 failure and a maze of
  invisible tab stops (`ReaderChrome.tsx:136`, `PageStrip.tsx:268`,
  `EndOfIssueCard.tsx:97`, `Reader.tsx:1286`). TapZones additionally wraps
  real buttons in a permanently `aria-hidden` div with labels like "Left
  zone" that don't describe the action (and flip meaning in RTL).
- *Recommendation:* React 19's native `inert` attribute on the same
  condition that sets `data-state="closed"` — one line per surface. Make tap
  zones pointer-only (`tabIndex={-1}`; the keymap already covers keyboard
  paging) or label them "Previous/Next page".
- *Impact:* The single highest-leverage a11y fix in the app — repairs the
  reader's entire keyboard story.

**E2. [High] [Accessibility] Global search is invisible to screen readers**
- *Problem:* SearchModal's input has no combobox role / `aria-expanded` /
  `aria-activedescendant`; arrow-key highlight is purely visual; the listbox
  owns section/h3/ul wrappers (invalid ownership); options have no ids.
  `TopbarSearchInline` puts `role="combobox"` on a wrapper div. No async
  announcements ("Searching…", result counts) anywhere
  (`SearchModal.tsx:139-191,313`).
- *Recommendation:* Rebuild both shells on the already-vendored
  `ui/command.tsx` (cmdk implements the full pattern) — which also collapses
  the ~600-line duplicated implementations (see F3).
- *Impact:* One of the two primary navigation paths becomes usable with AT.

**E3. [High] [Accessibility] Hand-rolled radiogroups without radio keyboard semantics**
- *Problem:* `SegmentedControl`, MarkersList kind filter, and a
  LibrarySettingsForm group emit `role="radiogroup"/"radio"` over plain
  buttons — no roving tabindex, arrow keys do nothing, every option is a tab
  stop. SRs announce instructions that don't work.
- *Recommendation:* Switch to `aria-pressed` toggle semantics (cheapest), or
  the vendored Radix RadioGroup/Tabs. One component fix covers many
  surfaces.

**E4. [High] [Accessibility] Marker interactions are pointer-only**
- *Problem:* Saved region markers are `<g onClick>` with no tabIndex/role;
  highlight creation requires a pointer drag; tap-to-OCR is pointer-only
  (`MarkerOverlay.tsx:481-484,716-724`). A headline feature fails WCAG
  2.1.1.
- *Recommendation:* Focusable proxies for saved markers (the `PagePin`
  button pattern already exists and is accessible); document `/bookmarks` as
  the conforming alternative; offer a whole-page fallback for creation.

**E5. [Medium] [Accessibility] Contrast failures, color-only state, and focus gaps**
- *Problem:* Cover placeholder text is ≈2.3:1 (`Cover.tsx:22-23,75` —
  neutral-600 on neutral-900, the cover-less tile for the whole library);
  `FilterPill` conveys active state by color only and never delivers the
  `aria-pressed` its own comment promises; light-theme `--primary` as text
  ≈2.3:1, `amber-600` small text ≈3.4:1; marker pins are white-on-amber
  ≈2.1:1; the main search page input has `focus:outline-none` with no
  replacement ring (`SearchView.tsx:238`), as do recents buttons, an
  activity link, and card menu buttons.
- *Recommendation:* `bg-muted/text-muted-foreground` for Cover; bake
  `aria-pressed` + a filled active state into FilterPill; darken light-theme
  warning/CTA text; `focus-within:ring` on composed inputs.

**E6. [Medium] [Accessibility] Async state never announced; live regions mounted too late**
- *Problem:* Infinite-scroll "Loading more…", search "Searching…"/"No
  matches" have no `role="status"`; the peek-mode banner mounts its
  `aria-live` together with its content (unreliable); SelectionToolbar puts
  aria-live on the toolbar element itself.
- *Recommendation:* `role="status"` wrappers; route announcements through
  the reader's existing persistent sr-only announcer (`Reader.tsx:790`) or a
  small global one.

**E7. [Medium] [Accessibility] Touch targets below 24px on destructive controls**
- *Problem:* Recent-search remove ✕ is **16px** (fails even WCAG 2.5.8's
  24px floor; duplicated in both search shells); topbar clear 24px;
  ExternalIds delete 24px; MarkerEditor copy 24px; EndOfIssueCard close /
  PagePins 28px; PageEditor row buttons 28px. Chrome auto-hide can also hide
  the currently-focused control (focus isn't treated as "pinned").
- *Recommendation:* Floor interactive elements at `h-8 w-8` (negative
  margins where tight); treat header focus-within as pinned.

**E8. [Medium] [Accessibility] Keyboard-dead drag handles; expandable admin rows mouse-only**
- *Problem:* Log-widget drag handle is `hidden` until hover → can never
  receive keyboard focus, making the configured `KeyboardSensor` dead code
  (`WidgetCard.tsx:98-109`); `DataTable` expandable rows are bare row
  `onClick`s with no button, keyboard path, `aria-expanded`, or visual
  chevron (`ui/data-table.tsx:113-124` — affects audit payloads and scan-run
  details).
- *Recommendation:* opacity-based reveal + `focus-visible:opacity-100` for
  the handle; a dedicated expander-cell `<button aria-expanded>` for tables.

**E9. [Low] [Accessibility] Semantics polish**
- `aria-label` on non-interactive role-less elements (skeleton divs,
  incognito chip, marker dots) is ignored by AT — use sr-only text.
- Home rails have no `<h2>`s (page outline is h1-then-flat); series page
  jumps h1→h3.
- CoverViewer arrow-paging updates "2 / 5" without announcing.
- `SelectionCheckbox` renders a real `<button>` inside card `<a>`s — invalid
  nesting the sibling overlays explicitly avoid with `role="button"` spans.
- Sign-in page heading is a styled div, not an `<h1>` (live-confirmed in the
  a11y tree).

---

## F. UI consistency & design system

**F1. [High] [UI] Dark-only status colors break the shipped light and amber themes**
- *Problem:* The app ships three themes, but ~50 sites hardcode
  dark-theme-only palette text: `text-emerald-300`/`text-red-300`
  (EmailStatusCard), `text-amber-200/80` (AuthConfigForm — ≈1.3:1 on white,
  effectively invisible), TokensCard, StatCard, LiveScanProgress status
  pills, LogsClient level tones, CronInput, ScanEventBeacon. Some files do
  dual variants correctly; the inconsistency is per-file.
- *Recommendation:* Mint semantic `--success`/`--warning`/`--info` (+
  foreground) tokens in all three theme blocks of `globals.css`, one shared
  `statusTone()` helper, mechanical sweep; guard with a grep CI rule
  (`text-(emerald|amber|red)-[23]00` without `dark:`).
- *Impact:* The entire admin status vocabulary currently disappears on two
  of three shipped themes; this is the largest token-discipline violation
  cluster.

**F2. [Medium] [UI] Reader surfaces bypass their own theme tokens**
- *Problem:* `--reader-bg`/`--reader-chrome` were minted precisely so the
  loading skeleton "can never drift from the live background" — the skeleton
  uses them; the live chrome, page strip, and settings popover hardcode
  `bg-neutral-950/85` (`ReaderChrome.tsx:135`, `PageStrip.tsx:279`).
  `ShortcutsSheet` hardcodes the reader-dark palette but mounts globally —
  light-theme users get a hard-dark sheet on any page. Marker colors are
  rgb() literals outside the accent system; rating stars hardcode amber-400
  (≈1.9:1 on light, ignores the accent setting).
- *Recommendation:* Route reader chrome through `bg-reader-chrome/…`;
  ShortcutsSheet onto `bg-popover` tokens; centralize a marker palette;
  decide stars (`text-primary` vs a deliberate `--rating` token).

**F3. [Medium] [UI] Duplicated component families to consolidate**
- *Problem (five families, all verified):*
  1. `NativeSelect` copy-pasted verbatim in SeriesEditDrawer and
     IssueActions, plus three divergent raw `<select>`s, plus shadcn Select —
     three idioms for one control.
  2. Copy-to-clipboard implemented four times (two near-identical siblings
     in the same file, AppPasswordsCard).
  3. SearchModal vs TopbarSearchInline: parallel ~600-line implementations
     with a character-for-character duplicated `Thumb`; already drifted.
  4. EmptyState exists but is admin-namespaced; library surfaces hand-roll
     five different empty-state patterns.
  5. Pills/chips/kbd: FilterPill vs hand-rolled toggle chips vs recents
     chips vs three `<kbd>` styles.
- *Recommendation:* Promote to `ui/`: `native-select.tsx`,
  `useCopyToClipboard` + `<CopyButton>`, `empty-state.tsx`, `kbd.tsx`; fold
  toggle chips into FilterPill (after it gains `aria-pressed`); extract one
  `useSearchController` consumed by both search shells (or move both onto
  cmdk per E2).
- *Impact:* Halves the surface area where search/a11y/UX bugs get fixed;
  design drift stops compounding.

**F4. [Low] [UI] Convention drift (cheap codemods)**
- Icon sizing split three ways (274× `h-4 w-4`, 27× `size-4`, 85×
  `h-3.5 w-3.5`) while `button.tsx` already enforces `[&_svg]:size-4`;
  kebab metaphor mixes MoreHorizontal/MoreVertical.
- Card padding: seven scales (`p-1.5`…`p-6`) with no rule — encode default +
  `compact` variants.
- Vendored ui/ components retain legacy `focus:` rings (badge, dialog/sheet
  close buttons) where the system standardized on `focus-visible:`.
- Sonner is fed `theme="amber"` (invalid for its `light|dark|system` union)
  — map amber → light.
- Dead `UserNav.tsx` (off-convention colors + ad-hoc fetch) — delete.

---

## G. Performance

**G1. [High] [Performance] Unvirtualized infinite lists keep every fetched page mounted**
- *Problem:* The grid, IssuesPanel, MarkersList, and ChronoFeed flatMap all
  fetched pages and render every card; `@tanstack/react-virtual` is installed
  but used only in cbl-detail and the PageStrip. The pagination invariant
  guarantees users *can* reach 10k mounted cards.
- *Recommendation:* `useVirtualizer` row-windowing for the grid +
  IssuesPanel first (card height derivable from `cardSize` × 3/2); keep the
  sentinel for fetching.
- *Impact:* Long-session browsing becomes constant-cost on large libraries.

**G2. [High] [Performance] `/progress` is fetched unbounded on three hot paths**
- *Problem:* `useUserProgress` fetches every progress record the user has;
  the SSR issue page and reader page each fetch the full list server-side to
  `.find()` one record; every debounced page-turn write invalidates it.
  Multi-hundred-KB JSON for heavy readers on the most-trafficked routes.
- *Recommendation:* `GET /progress?issue_id=` (or fold progress into
  `IssueDetailView`) for SSR; embed progress in list responses or paginate
  by `updated_at` for cards.

**G3. [High] [Performance] Issue-page SSR is a 7-deep sequential fetch waterfall**
- *Problem:* `issues/[issueSlug]/page.tsx:90-166` awaits seven fetches in
  series where six are independent; at 80ms RTT that's ~500ms of pure
  serialized latency. The reader page in the same codebase already uses
  `Promise.all` with an explanatory comment. Series page has a 3-deep
  version.
- *Recommendation:* `Promise.all` with per-fetch `.catch(() => null)`.
  An hour of work, directly cuts TTFB on the two most-visited pages.

**G4. [High] [Performance] Grid re-renders wholesale on every search keystroke**
- *Problem:* The raw `q` input state lives in the same hook the grid
  consumes; the 200ms debounce protects the network, not the render; cards
  aren't memoized; the React Compiler is not enabled. 500 mounted cards
  re-render per keystroke.
- *Recommendation:* Enable the React Compiler (React 19 is already in place
  — the biggest single lever), or hoist input state into the toolbar +
  `memo()` the cards.

**G5. [High] [Performance] WS scan events nuke the whole series/issues cache — and sidecar writeback multiplies events**
- *Problem:* `scan-events.ts:183,227-228` invalidates `["series"]` (and
  `["issues"]`) on every `scan.completed`/`metadata.applied`; a 50-issue
  sidecar apply enqueues ~50 scoped rescans → 50 full-cache refetch storms
  while the server is busiest. Admin surfaces also poll on intervals that
  overlap the same WS signal.
- *Recommendation:* Scope invalidations to the event's library/series ids and
  debounce-coalesce into a ~2s flush window (the `TOASTED_SCANS` dedupe is
  precedent); drop intervals where WS covers the signal.

**G6. [Medium] [Performance] Heavy dialogs statically bundled into content routes**
- *Problem:* `MetadataMatchDialog` (1,208 lines), `PageEditor` (+dnd-kit),
  `EditMetadataDialog`, `BulkArchiveEditDialog` are static imports on
  series/issue pages; the admin dashboard already demonstrates the
  `dynamic()` pattern. The reader ships `MarkerEditor`/OCR overlay in its
  initial chunk — the bundle-budget script documents the plan (158KB gz vs
  150KB target) but it's unexecuted.
- *Recommendation:* `next/dynamic` on first-open for the four dialogs;
  lazy-mount MarkerEditor when `pendingMarker !== null`; ratchet the budget
  back to 150KB.

**G7. [Medium] [Performance] No SSR→client cache handoff**
- *Problem:* Zero `HydrationBoundary`/`initialData` usage; `/auth/me` and the
  sidebar layout are fetched server-side per render and immediately refetched
  client-side on hydration.
- *Recommendation:* Seed `queryClient` with the layout's `me` + sidebar
  payloads (`initialData` is enough; full dehydration optional).

**G8. [Medium] [Performance] Global query retry/online policies amplify failures**
- *Problem:* TanStack default `retry: 3` applies to deterministic 4xx
  failures (~3.5s of backoff before errors surface; only the OCR text-regions
  query opts out); the `online` handler invalidates the *entire* cache on
  every offline→online flap; `ScanResultListener` in the root layout fires
  `/auth/me` (+ refresh + retries) for anonymous visitors — **live-confirmed:
  the sign-in page logs ~8 doomed 401/403 requests**.
- *Recommendation:* `retry: (n, err) => n < 2 && err.status >= 500`;
  reconnect-invalidate only errored queries; gate the listener on an auth
  signal.

**G9. [Medium] [Performance] Image bytes are one-size-fits-all**
- *Problem:* Covers render from a single thumb variant with no
  `srcset`/`sizes` while the card slider goes to 280px (560px at 2× DPR);
  reader pages have no blur-up despite strip thumbs existing; PageStrip
  pre-warms ±12 thumbs per page-turn even when hidden.
- *Recommendation:* Emit 1×/2× cover variants + `srcset`; strip-thumb
  blur-up under the full page; gate strip warming on visibility.

**G10. [Low] [Performance] Misc**
- Legacy `/?q=` search path fetches `limit=100` with no pagination — the
  exact anti-pattern the repo's review heuristics ban; redirect to `/search`
  and delete it. (Also an IA dupe — one search surface.)
- Double-page transitions remount both panes per flip (keyed on the joined
  spread) — key on page index and drive animation via a token.
- IntersectionObserver effects depend on the whole `query` object identity —
  observers are torn down/rebuilt every render; depend on the three stable
  fields.
- `useDeleteMarker("")` hooks constructed per page-turn in the reader hot
  path.

---

## H. Frontend architecture

**H1. [High] [Architecture] `queries.ts` (2,568 lines) and `mutations/index.ts` (2,704 lines) are acknowledged monoliths**
- *Problem:* The M5 shard-out stalled after one domain; 108 query hooks + the
  whole `queryKeys` registry in one file; constant PR blast radius; dev-mode
  compile cost.
- *Recommendation:* Move `queryKeys` out first (it's the hub), then shard by
  domain behind the existing barrel.

**H2. [Medium] [Architecture] Forms: two stacks, hand-duplicated validation**
- *Problem:* RHF+zod covers 8 forms; ~22 others are hand-rolled controlled
  state; zod re-states garde rules with no codegen link (silent drift); no
  field-level server-error mapping (see D3).
- *Recommendation:* Standardize new forms on RHF+zod; lean on the 422
  envelope as the source of truth for cross-field rules.

**H3. [Medium] [Architecture] State/storage hygiene**
- *Problem:* Reader localStorage writes 5 raw keys per series forever (no
  versioning, no eviction — ~10k keys for a 2k-series sampler); ~20 files
  use ad-hoc localStorage key schemes; 8 call sites build inline query-key
  tuples bypassing the `queryKeys` registry (the known invalidation-miss
  vector); tab state is URL-synced on one admin page, half-synced on another,
  absent elsewhere.
- *Recommendation:* Namespaced versioned storage helper with LRU cap;
  add the missing registry entries + a grep CI gate (the repo already runs
  grep gates); standardize the `FindingsView` URL-sync pattern.

**H4. [Medium] [Architecture] Error-boundary gap in the reader; component monoliths**
- *Problem:* Every route group has `error.tsx` except `read/` — a mid-read
  crash drops to the generic boundary with no "back to this issue" and no
  reader styling. Seven components exceed 900 lines (LiveScanProgress 1,443;
  IssueActions 1,215; MetadataMatchDialog 1,208 …) post-cleanup.
- *Recommendation:* Add `read/.../error.tsx` (exit-to-issue + `--reader-bg`);
  apply the proven LibraryGridView decomposition recipe to the top three.

**H5. [Low] [Architecture] Dev/prod bundler divergence**
- *Problem:* Dev pins webpack (justified — Turbopack leak) while prod builds
  use Turbopack; devs never see production chunk topology unless they run
  the budget script.
- *Recommendation:* Document `pnpm check-bundle-size` as the canonical check;
  revisit when the upstream leak is fixed.

---

## I. Live findings (environment & console)

**I1. [High] [Architecture] CSP violation on every page load**
- *Problem:* The browser console logs a blocked inline script against the
  strict nonce CSP on every navigation (`Executing inline script violates …
  'nonce-…' 'strict-dynamic'`). Likely a Next dev-runtime inline; if it can
  fire in prod builds, whatever it powers silently doesn't run.
- *Recommendation:* Identify the script (hash in the violation), add it to
  the nonce path or hash allowlist; add a CSP-report assertion to the
  Playwright harness so regressions surface.

**I2. [Medium] [UX] Unauthenticated visits hammer the API before redirecting**
- *Problem:* The sign-in page fires `/api/auth/me`, `/api/series`,
  `/api/auth/refresh` in repeated retry rounds (~8 requests of 401/403)
  before settling. Same root cause as G8's retry policy + root-layout
  listener.

**I3. [Note] [Dev-env] `compose.dev.yml` Postgres bump broke existing dev volumes**
- The deps PR #85 moved dev Postgres to `postgres:18-alpine`; the 18 image
  changes the data-directory layout, so recreating the container against an
  existing PG17 volume crash-loops. Pinned back to `17-alpine` in this
  audit's working tree; moving to 18 needs a `pg_upgrade` pass (or a
  documented volume reset).

---

## J. Strengths (what's already best-in-class)

- **Reader image pipeline**: decode-ahead prefetch with retained-element LRU,
  priority hints, synchronous cache probes — better than most commercial
  readers. RTL/direction rigor across keys/swipe/zones/strip is rare.
- **Pagination integrity**: cursor + infinite query + sentinel everywhere,
  with regression tests anchoring the no-silent-truncation invariant.
- **Metadata transparency**: per-field provenance, pinned-field display,
  apply checkboxes, multi-provider compare — best-in-class.
- **Live-scan observability**: single WS subscription, tested reducer, phase
  chips, rate/ETA, deduped toasts.
- **Secret handling**: `<set>` sentinels, type-to-replace placeholders,
  encrypted at rest — uniformly applied.
- **Destructive-action discipline**: type-the-name deletes, accurate
  AlertDialog consequences, undo toasts; `disabled={!isDirty}` near-universal.
- **Keyboard story**: 21 rebindable reader actions, conflict detection,
  context-aware `?` sheet; palette with role-gated admin commands.
- **Error/loading boundary discipline** per route group, digest-tagged
  errors, stall-recovery watchdog; theme-cookie SSR to kill FOUC; per-tab
  `dynamic()` splitting of chart code.
- **Mobile/PWA craft**: safe-area handling, dvh fixes, iOS install banner +
  splash set, navigation-bypassing service worker that never caches per-user
  data.

---

## K. Product vision recommendations

**1. Own the first ten minutes (Plex/Jellyfin-class onboarding).**
Folio's depth is invisible until a library is scanned and matched. A guided
setup — create library → watch the live scan (the observability is already
best-in-class, point it at the user) → connect a provider → "here's your
first unread issue" — converts the strongest parts of the backend into the
first impression. Jellyfin's setup wizard and Plex's library-add flow are
the reference points.

**2. Become a real mobile reading app (Tachiyomi/Mihon parity).**
The PWA scaffolding (install banner, splash screens, safe areas, service
worker) is already there; what's missing is the payoff: bottom tab bar,
offline downloads, wake-lock toggle, double-tap zoom, tap-bottom-to-advance
in webtoon, and an end-of-chapter flow on touch. These six items are the gap
between "website that installs" and "reading app".

**3. Make ⌘K the universal surface (Linear/Raycast).**
The palette already has command mode and role-gated admin actions — extend
it to every entity (collections, views, pages, libraries, settings panes,
users for admins) and every action ("scan Marvel", "toggle theme", "match
this series"). Linear's palette is the bar: anything you can click, you can
type.

**4. Zero-silence automation (Vercel-style activity feed).**
Scans, bulk matches, backfills, and weekly refreshes currently end in
silence or an admin tab. A single job/notification center — every queued
thing, its progress, its outcome, "review results" actions, optional
webhook/email — turns automation from "trust me" into a feed. The WS
plumbing and audit data already exist; this is presentation.

**5. Health as a triage inbox, not a report (Notion/Linear inbox).**
Findings, unmatched series, drift, duplicates, and quota states are scattered
status pages. Unify them into one prioritized inbox where every row carries
its fix action (match, rescan, dismiss-with-undo) and bulk operations. The
"see problem → fix problem in one click" loop is what separates admin tools
people like from ones they tolerate.

**6. Data liberation as a trust feature.**
Self-hosters choose this category for ownership: exports (reading log CSV,
collections → CBL, full account JSON), copy-link/share everywhere, and the
OPDS panel (A9) are cheap and signal the product's values.

### Where the product currently feels dated or fragmented

- The **settings-vs-library split of views/collections/CBL** (A3) — feels
  like two products.
- **Five "activity" surfaces** (A5) and the Library/Home/All-Libraries
  naming knot (A7).
- **Two parallel search implementations** with drifted behavior (F3).
- **Dead i18n scaffolding** and placeholder dictionary content (A6).
- **Theme support that only really works in dark** (F1, F2) despite shipping
  three themes.
- **Modal-only metadata workflows** at library scale (B4, B10).

### What already beats the competition (protect these)

Per-field provenance + multi-provider compare, the reader's
prefetch/decode pipeline and RTL rigor, live scan observability, pagination
integrity, the customizable sidebar/rails system, and the keybinding
infrastructure. None of Komga/Kavita/Plex do these as well; the vision items
above are about making the first hour and the phone experience worthy of
that core.

---

*Generated 2026-06-12 from six parallel code-review passes + a live
Playwright session (desktop 1440×900, mobile 390×844) against commit
`9249730` + uncommitted end-card work. Companion environment fix applied
during the audit: `compose.dev.yml` Postgres pinned back to `17-alpine`
(see I3).*
