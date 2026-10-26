# Phone layout (< `sm`)

How list and detail pages compose their top-of-page chrome on phones,
and the primitives that make a page phone-ready. "Phone" means below
Tailwind's `sm` breakpoint (639px and narrower); the shell's `<main>` is
`px-4` there, so content is ~358px wide on a 390px device.

## The problem this solves

Every grid page used to render one desktop toolbar made of `h-9`
controls inside `flex-wrap`. At 358px the row folded: the library grid
showed title, Series/Issues, search, sort + order + Filters, Save as
view, Select + card size and a two-row A–Z strip — eight rows, ~60% of
the first screen — before the first cover. Nothing was sticky, so the
cost was paid up front with no later benefit. The same shape repeated
on bookmarks, CBL views, system views, the series page and the entity
indexes.

## Rules

1. **Two compositions, not one that wraps.** Desktop markup stays as
   is and gets `max-sm:hidden`; the phone composition is added next to
   it with `sm:hidden`. They never both render, and the desktop output
   is unchanged.
2. **One control row on phones.** `PhoneControlsBar` is the row:
   icons for search / sort / filters / View options, an optional leading
   slot (mode toggle, stats pills, item count) and one optional inline
   `primary` icon (CBL's Up Next, a collection's Continue). Anything
   secondary — card size, Select, Save as view, Clear filters — goes in
   the View options bottom sheet via `menu={(close) => …}`.

   **Two menu icons, two meanings, at every width.** The four-squares
   glyph (`LayoutGrid`) is _View options_: a panel of presentation
   settings (card size, collection dots, layout) plus the grid-wide
   tools above. It is `CardSizeOptions`' popover on desktop and the
   bar's bottom sheet on phones, and the bar's trigger defaults to that
   glyph. `⋯` is _More actions_ on the page's subject — Edit, Export,
   Pin, Refresh — and is a dropdown list at every width (`ViewHeader`,
   `PageActionsMenu`). So the same icon always opens the same kind of
   surface, and a page never shows two `⋯`.

3. **What pins is decided by content, not by width.** A control row
   pins under the topbar at every width if and only if it holds tools
   you need _while scrolling_: search, sort, filters, bulk selection —
   and, on a rails page (Home, custom pages), the density control, since
   you tune card size while browsing a long stack of rails. Everything
   else scrolls away — titles, descriptions, breadcrumbs, tab strips,
   chip rows, the A–Z rail, and one-time settings in a title row such as
   Continue and `⋯` (a grid's card size and `⋯` ride in the pinned row
   when the page has one, but never earn a row by themselves). So the
   library grid, bookmarks, CBL reading lists, the series Issues panel
   and the rails pages pin their row; filter / system / collection
   views, search and the indexes do not. A mid-page row (the Issues panel) still qualifies: `sticky`
   only engages once the page has scrolled to it, so it never covers
   the hero.

   One pinned band per page, same offset and material everywhere:
   `top-(--topbar-h)`, `CHROME_BAND_CLASS`, `z-20`, border / backdrop
   only once actually held. Below `sm` that band is `PhoneControlsBar`
   (sticky by default; the page title may ride in its `leading` slot
   when the page hides its header — bookmarks, the Issues panel — so
   the band stays the only pinned thing). From `sm` up it is
   `StickyToolbarRow` (`components/library/StickyToolbarRow.tsx`), the
   row the desktop controls live in: it pins only while it fits on one
   line (`useStickyToolbar`'s `ResizeObserver` guard — a toolbar that has
   wrapped on an iPad with the sidebar open stays in flow rather than
   pinning 100px of chrome) and only while `enabled`. Pass
   `enabled={!selection.selectMode}` to the row and
   `hidden={selection.selectMode}` to the bar: `SelectionToolbar` is
   sticky at the same offset and owns the band while selecting. Do not
   put a qualifying row inside `PageHeader`'s actions or a title row —
   it would scroll away with the title. Rows that only hold card size,
   Continue or `⋯` stay in the title row at every width.

4. **No second search box.** The topbar already has one. On phones a
   page-local search is the bar's 🔍 icon, which expands to an input in
   place with a Cancel button. The bar keeps keystrokes local, so pass
   the caller's debounced value.
5. **A–Z is a vertical scrubber.** `AtoZScrubber` pins `#` + A–Z to the
   right edge between the topbar and the bottom tab bar (tap to jump,
   drag to scrub with a preview bubble, commit on release, tap the
   active letter to clear). Give the desktop `AtoZJumpRail`
   `max-sm:hidden` and reserve the index column on the grid with
   `max-sm:pr-2` (the 20px rail already overlaps the 16px shell gutter), so the glyphs sit
   ~11px off the covers and ~7px off the bezel like the system index.
6. **Chips scroll, they don't wrap.** `ActiveChips` is a single
   horizontally scrolling row on phones (`-mx-4 px-4` so it runs edge
   to edge, scrollbar hidden). Tab strips already do this via
   `TabsList` — never add `flex-wrap` to one, it is `h-9 overflow-x-auto`
   and wrapped tabs are clipped.
7. **Headers shrink, descriptions clamp.** `PageHeader` / `ViewHeader`
   titles are `max-sm:text-xl`, descriptions `max-sm:text-xs
max-sm:line-clamp-2`; `PageHeader` takes `phoneDescription="hidden"`
   for pages whose description only restates the title. Breadcrumbs,
   eyebrows and item counts that duplicate the title or the bottom tab
   bar are `max-sm:hidden`. When that leaves the phone header as a bare
   title above a bar that has no leading content of its own, fold the
   title into the bar: give `PageHeader` `className="max-sm:hidden"` and
   pass an `<h1>` as the bar's `leading` (bookmarks). Title and controls
   then share one sticky row instead of two stacked bordered bands; the
   desktop header is untouched.
8. **Two columns minimum.** Every cover grid uses
   `repeat(auto-fill, minmax(min(<cardSize>px, calc(50% - 0.5rem)), 1fr))`
   and the virtualized library grid applies the same half-row cap in
   `computeColumnsPerRow` (down to 96px cards), so a large saved card
   size or the reserved A–Z column can never collapse a phone grid to
   one column; desktop is unaffected because half a wide column exceeds
   any card size.
9. **One visible size, an invisible touch target.** Controls keep
   shadcn's size on every pointer — 36px toolbar controls and tab strip,
   28px chips — so iPad looks like desktop and no toolbar is taller than
   the tabs beside it. Under `(pointer: coarse)` `globals.css` gives
   every pristine shadcn control by its `data-slot` (`button`,
   `select-trigger`, `tabs-trigger`, `checkbox`, `switch`, …; the files
   under `components/ui/` are vendored verbatim and never carry a Folio
   class), plus `.folio-control` (`NativeSelect`, segmented groups, the
   CBL header pills) and `.folio-tap` (filter pills, segment buttons,
   card kebabs) on Folio-authored controls, a transparent `::after` tap
   area 44px tall and 4px wider
   on each side — Apple's 44pt minimum met the way Apple and Material
   meet it, by a hit region larger than the drawn control. The 8px
   toolbar gap keeps neighbouring tap areas from overlapping. A text
   field or native select cannot carry a pseudo-element and stays its
   36px box; as a wide target that is fine. Never floor the box itself
   (`min-height: 44px` was tried and made every toolbar taller than the
   tab strip). A new small interactive element that is not a `<Button>`
   must carry `folio-tap`; Playwright reports the tap area only with
   `hasTouch: true` by probing `getComputedStyle(el, "::after")`. The
   fine-pointer half of the same rule: a toolbar `<Button>` is
   `size="toolbar"` — `h-9` at the base 14px text, matching the
   `<Input>` / `<NativeSelect>` beside it — never `size="sm"` (32px at
   12px), and never `size="sm"` with an `h-9` override, which fixed the
   height but left the label two sizes smaller than the input. The shared controls — `SelectModeButton`,
   `NotesExportMenu`, `CardSizeOptions`, `AddWidgetMenu`, the `/log`
   `ActivityRangeSelector` — bake it in; `tests/dom/toolbar-row-height.test.tsx`
   guards them.
10. **Prefer CSS to JS.** Use `max-sm:` / `sm:` whenever the difference
    is presentational. `useIsPhone()` (`lib/ui/use-is-phone.ts`,
    `useSyncExternalStore` on `(max-width: 639px)`, `false` on SSR) is
    for the rare case where the component _tree_ differs; it is the
    single copy shared by the reader chrome and the toaster.

A control that _contains_ controls (a segmented group: `role="group"`
with buttons inside) carries `folio-control` for the row's sake but
gets no tap area of its own — `globals.css` suppresses the
pseudo-element on a `.folio-control:has(button, …)`, because it would
paint above the children and take their taps. The children carry
`folio-tap` / `folio-button` and their own tap areas.

A horizontal strip of such controls (`TabsList`, `SegmentedControl`, the
phone chip rows) must be `overflow-x-auto overflow-y-hidden`: with
`overflow-x-auto` alone the tap areas poking past the strip give it a few
px of vertical scroll, and iOS lets a finger drag the labels up and down
inside the bar. Inside a strip the tap area is bounded by the strip's
height (36px); the horizontal +4px each side still applies.

## Primitives

| Piece                      | File                                         | Notes                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| -------------------------- | -------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `PhoneControlsBar`         | `components/library/PhoneControlsBar.tsx`    | `leading`, `search`, `sort` (bottom sheet with radio list + asc/desc), `filters {count,onOpen}`, `primary`, `menu(close)`, `sticky`, `hidden`. `sort.onChange` is untyped (`string`); narrow it back to your sort union.                                                                                                                                                                                                                  |
| `PhoneMenuAction`          | same file                                    | Full-width `h-11` row button for the View options sheet; supports `asChild` for download links.                                                                                                                                                                                                                                                                                                                                           |
| `CardSizeFields`           | `components/library/CardSizeOptions.tsx`     | The card-size popover's body (slider, collection-dot switch, Reset) for use inside the sheet. Pass a unique `fieldId`.                                                                                                                                                                                                                                                                                                                    |
| `AtoZScrubber`             | `components/library/AtoZScrubber.tsx`        | Same `value` / `onSelect` contract as `AtoZJumpRail`.                                                                                                                                                                                                                                                                                                                                                                                     |
| `ModeSegmentedToggle`      | `components/library/LibraryGridToolbar.tsx`  | Series ⇄ Issues segmented control for the bar's leading slot.                                                                                                                                                                                                                                                                                                                                                                             |
| `StickyToolbarRow`         | `components/library/StickyToolbarRow.tsx`    | The `sm`-and-up pinned control row (rule 3): `enabled`, `className`, extra `data-*` pass through. Wraps `useStickyToolbar`; `data-sticky` / `data-stuck` report state. Edge to edge in the shell column.                                                                                                                                                                                                                                  |
| `useIsPhone`               | `lib/ui/use-is-phone.ts`                     | See rule 8.                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `ViewActionsMenu`          | `components/saved-views/ViewActionsMenu.tsx` | The saved view's `⋯` dropdown (Edit, `extraMenuItems`, Sidebar, Pin). `ViewHeader` renders it inline by default; `actions="none"` lets a page place it in its control row instead (CBL).                                                                                                                                                                                                                                                  |
| `ViewHeader.phoneControls` | `components/saved-views/ViewHeader.tsx`      | `phoneControlsPlacement="row"` (default) renders the bar as a sibling _after_ the header so it can stick — for bars with search or a leading slot (CBL). `"inline"` puts it in the title row beside the `⋯` actions menu — for single-icon bars (pass `sticky={false}`; the bar's trigger is already the four-squares glyph, so it never reads as a second `⋯`). `extraActions` are `max-sm:hidden`; the `⋯` menu and Built-in chip stay. |

## Page-by-page

- **Home and custom pages** (`PageRails`): from `sm` up the heading
  (title + description) is in flow and a `StickyToolbarRow` beneath it
  carries card size + `⋯`, right aligned; below `sm` the title rides in
  the sticky bar's leading slot beside View options (card size as a
  sheet) and `⋯`, with the description following in flow. Inline
  title editing is `sm`-and-up; phones rename through `⋯`.
- **Library grid** (`LibraryGridView`): title (`text-xl`) + count, sticky
  bar `[Series|Issues] … [🔍][⇅][⚲][▦]`, chips row, grid with the
  scrubber in series mode. The View options sheet holds card size,
  Select, Save as view, Clear filters.
- **Saved views** (filter / issue filter / collection / CBL / system):
  `ViewHeader` title row with the card-size icon and `⋯`, clamped
  description. Filter, issue-filter, collection and system views place
  the bar inline in the title row (one row total); collection shows
  the item count in `leading` and Continue as `primary`. CBL's title
  row is title-only (`actions="none"`): everything else rides in the
  one pinned control row at every width — the stats pills in
  `leading`, search, Up Next and the `⋯` (`ViewActionsMenu`) as
  `primary` on phones; from `sm` up a `StickyToolbarRow` with Up Next,
  search, card size, Select, then the pills and `⋯` on the right. System
  views hide the breadcrumb and eyebrow.
- **Series detail**: cover beside the title block (≈40% width) with
  the Read row under both; Writers / Publication fact blocks hidden
  (Credits tab and status badge carry them); stat row stays the 2×2 grid the issue page uses (a horizontal
  strip was tried in #1249 and reverted); the issue list comes _before_
  the metadata tabs (`max-sm:order-last` on `SeriesTabs`); the Issues
  panel is one sticky row `Issues · N … [🔍][⇅][▦]` with the heading in
  `leading`, pinned once scrolled to; from `sm` up the heading has its
  own row and the input / Sort / card size / Select cluster is a
  `StickyToolbarRow`. `SeriesTabs`' `scroll-mt` clears the pinned row.
- **Bookmarks**: no separate header — the title rides in the sticky
  bar's leading slot beside search, a Filters sheet (layout toggle,
  tags, match mode) and View options (card size, Select, export); kind
  pills scroll horizontally. From `sm` up the header keeps the title and
  description and the toolbar is a `StickyToolbarRow` beneath it.
- **Search**, **Browse**, **entity / creator indexes**: description
  hidden, tab strip scrolls, indexes use the scrubber. Search keeps
  its own search box, so its only phone control is the four-squares
  View options sheet, inline in the title row like the saved views.

## Testing

`tests/dom/phone-controls-bar.test.tsx` and `tests/dom/az-scrubber.test.tsx`
cover the primitives (jsdom + Testing Library); `tests/library/view-header-phone.test.tsx`
guards the `ViewHeader` split. When adding a page, assert that the
desktop cluster is behind `max-sm:hidden` and the bar is not inside a
`max-sm:hidden` ancestor — the two compositions must never both render.
