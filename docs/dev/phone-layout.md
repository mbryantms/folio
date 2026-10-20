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
   icons for search / sort / filters / `⋯`, an optional leading slot
   (mode toggle, stats pills, item count) and one optional inline
   `primary` icon (CBL's Up Next, a collection's Continue). Anything
   secondary — card size, Select, Save as view, Clear filters — goes in
   the `⋯` "View options" bottom sheet via `menu={(close) => …}`.
3. **Sticky under the topbar.** The bar is `sticky top-(--topbar-h)`
   by default so the controls stay reachable after scrolling. Pass
   `sticky={false}` for a mid-page bar (the series page's Issues
   panel). `SelectionToolbar` is also sticky at `top-(--topbar-h)`, so
   pass `hidden={selection.selectMode}` — the selection toolbar takes
   the row's place while selecting.
4. **No second search box.** The topbar already has one. On phones a
   page-local search is the bar's 🔍 icon, which expands to an input in
   place with a Cancel button. The bar keeps keystrokes local, so pass
   the caller's debounced value.
5. **A–Z is a vertical scrubber.** `AtoZScrubber` pins `#` + A–Z to the
   right edge between the topbar and the bottom tab bar (tap to jump,
   drag to scrub with a preview bubble, commit on release, tap the
   active letter to clear). Give the desktop `AtoZJumpRail`
   `max-sm:hidden`; the scrubber is 16px wide and sits in the shell's right
   gutter, so the grid keeps its full width (no padding needed).
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
   bar are `max-sm:hidden`.
8. **Two columns minimum.** Every cover grid uses
   `repeat(auto-fill, minmax(min(<cardSize>px, calc(50% - 0.5rem)), 1fr))`
   so a large saved card size (a desktop preference synced to the
   phone) can never collapse the grid to one column; desktop is
   unaffected because 50% of a wide column exceeds any card size.
9. **Prefer CSS to JS.** Use `max-sm:` / `sm:` whenever the difference
   is presentational. `useIsPhone()` (`lib/ui/use-is-phone.ts`,
   `useSyncExternalStore` on `(max-width: 639px)`, `false` on SSR) is
   for the rare case where the component _tree_ differs; it is the
   single copy shared by the reader chrome and the toaster.

## Primitives

| Piece                      | File                                        | Notes                                                                                                                                                                                                                                                                                                                                                                                                       |
| -------------------------- | ------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `PhoneControlsBar`         | `components/library/PhoneControlsBar.tsx`   | `leading`, `search`, `sort` (bottom sheet with radio list + asc/desc), `filters {count,onOpen}`, `primary`, `menu(close)`, `sticky`, `hidden`. `sort.onChange` is untyped (`string`); narrow it back to your sort union.                                                                                                                                                                                    |
| `PhoneMenuAction`          | same file                                   | Full-width `h-11` row button for the `⋯` sheet; supports `asChild` for download links.                                                                                                                                                                                                                                                                                                                      |
| `CardSizeFields`           | `components/library/CardSizeOptions.tsx`    | The card-size popover's body (slider, collection-dot switch, Reset) for use inside the sheet. Pass a unique `fieldId`.                                                                                                                                                                                                                                                                                      |
| `AtoZScrubber`             | `components/library/AtoZScrubber.tsx`       | Same `value` / `onSelect` contract as `AtoZJumpRail`.                                                                                                                                                                                                                                                                                                                                                       |
| `ModeSegmentedToggle`      | `components/library/LibraryGridToolbar.tsx` | Series ⇄ Issues segmented control for the bar's leading slot.                                                                                                                                                                                                                                                                                                                                               |
| `useIsPhone`               | `lib/ui/use-is-phone.ts`                    | See rule 8.                                                                                                                                                                                                                                                                                                                                                                                                 |
| `ViewHeader.phoneControls` | `components/saved-views/ViewHeader.tsx`     | `phoneControlsPlacement="row"` (default) renders the bar as a sibling _after_ the header so it can stick — for bars with search or a leading slot (CBL). `"inline"` puts it in the title row beside the `⋯` actions menu — for single-icon bars (pass `sticky={false}` and `menuIcon={<LayoutGrid />}` so it is not a second `⋯`). `extraActions` are `max-sm:hidden`; the `⋯` menu and Built-in chip stay. |

## Page-by-page

- **Library grid** (`LibraryGridView`): title (`text-xl`) + count, sticky
  bar `[Series|Issues] … [🔍][⇅][⚲][⋯]`, chips row, grid with the
  scrubber in series mode. The `⋯` sheet holds card size, Select, Save
  as view, Clear filters.
- **Saved views** (filter / issue filter / collection / CBL / system):
  `ViewHeader` title row with the card-size icon and `⋯`, clamped
  description. Filter, issue-filter, collection and system views place
  the bar inline in the title row (one row total); collection shows
  the item count in `leading` and Continue as `primary`. CBL keeps a
  separate sticky row because its bar carries search, the stats pills
  in `leading` and Up Next as `primary`. System views hide the
  breadcrumb and eyebrow.
- **Series detail**: cover beside the title block (≈40% width) with
  the Read row under both; Writers / Publication fact blocks hidden
  (Credits tab and status badge carry them); stat row is a horizontal
  strip; the issue list comes _before_ the metadata tabs
  (`max-sm:order-last` on `SeriesTabs`); the Issues panel header is
  `h2 … [🔍][⇅][⋯]` with `sticky={false}`.
- **Bookmarks**: header without description, sticky bar with search,
  a Filters sheet (layout toggle, tags, match mode) and `⋯` (card size,
  Select, export); kind pills scroll horizontally.
- **Search**, **Browse**, **entity / creator indexes**: description
  hidden, tab strip scrolls, indexes use the scrubber.

## Testing

`tests/dom/phone-controls-bar.test.tsx` and `tests/dom/az-scrubber.test.tsx`
cover the primitives (jsdom + Testing Library); `tests/library/view-header-phone.test.tsx`
guards the `ViewHeader` split. When adding a page, assert that the
desktop cluster is behind `max-sm:hidden` and the bar is not inside a
`max-sm:hidden` ancestor — the two compositions must never both render.
