# Multi-select + bulk actions

How list-page multi-select is wired in Folio. This is the contract
for adding a new list surface (search results, multi-page rails,
etc.) without re-inventing the toolbar / hook / endpoint shape.

Plan archive: `~/.claude/plans/multi-select-bulk-actions-1.0.md`.

## What's covered today

| Surface                  | Cards    | Mark read/unread | Add to collection | Remove from container |
|--------------------------|----------|------------------|-------------------|-----------------------|
| Series → Issues panel    | issues   | ✓                | ✓ (overflow)      | —                     |
| Filter view detail       | series   | ✓ (series-bulk)  | ✓ (overflow)      | —                     |
| Collection view detail   | mixed    | ✓ (issues only)  | ✓                 | ✓ (destructive)       |
| CBL view detail          | CBL rows | ✓ (matched only) | —                 | —                     |

Not wired yet: home page rails (cards live inside rails, not list
pages), search results, multi-page rails, recently-read on settings.

## Primitives

### `useSelection<T extends { id: string }>(items)`

[`web/lib/selection/use-selection.ts`](../../web/lib/selection/use-selection.ts).
Per-page hook holding the selection set + select-mode flag. Stable
across paginated loads as long as `items` is the full loaded list
(infinite-query callers must pass `data.pages.flatMap(p => p.items)`,
not just the current page).

```ts
const items = data?.pages.flatMap((p) => p.items) ?? [];
const selection = useSelection(items);

selection.selectMode;    // boolean — controls toolbar visibility
selection.count;         // number — selected item count
selection.selected;      // Set<string> — selected ids
selection.isSelected(id) // boolean
selection.toggle(id, ev) // click toggle (Shift = range from anchor)
selection.selectAll()    // every id in `items`
selection.clear()        // empty selected, stay in select mode
selection.enter()        // turn on select mode
selection.exit()         // turn off; also clears selected
```

Pure helpers (`computeToggle`, `computeRangeAdd`, `buildIdIndex`) are
exported for unit-testing the math. See
[`web/tests/selection/use-selection.test.ts`](../../web/tests/selection/use-selection.test.ts).

### `<SelectionToolbar>`

[`web/components/library/SelectionToolbar.tsx`](../../web/components/library/SelectionToolbar.tsx).
Sticky top-of-list bar that appears once `selection.selectMode` is
on. Container-agnostic: each page passes its own
`primary` + `overflow` action arrays.

```tsx
<SelectionToolbar
  count={selection.count}
  total={items.length}
  primary={[{ id, label, icon, onClick, disabled }]}
  overflow={[/* same shape */]}
  onDone={() => selection.exit()}
  onClear={() => selection.clear()}
  onSelectAll={() => selection.selectAll()}
  isPending={bulkMark.isPending}
/>
```

Semantics worth knowing:
- **Done vs. Clear:** Done exits select mode (toolbar disappears).
  Clear empties the selection but keeps select mode on. Esc on
  desktop maps to Done; the X icon button is the toolbar's Done.
- **Overflow collapse:** `primary` actions render inline at every
  width. `overflow` actions render inline at `sm+` and collapse to
  a `MoreHorizontal` dropdown below `sm` so the toolbar fits a
  375 px viewport.
- **Mid-mutation lockout:** `isPending` disables every action
  button to prevent double-firing during a round-trip.

### `<SelectionCheckbox>`

[`web/components/library/SelectionCheckbox.tsx`](../../web/components/library/SelectionCheckbox.tsx).
Absolute-positioned overlay on each card. ≥44×44 px tap target.
Hover-revealed on desktop via `@media (hover: hover)`; persistent
on mobile while select mode is on.

### Card `selectMode` + `onEnterSelectMode` props

Every list card (`<IssueCard>`, `<SeriesCard>`,
`<CblIssueCard>`, sortable wrappers in `<CollectionViewDetail>`)
accepts:

```ts
selectMode?: {
  isActive: boolean;
  isSelected: boolean;
  onToggle: (ev?: React.MouseEvent) => void;
};
onEnterSelectMode?: (id: string) => void;
```

When `selectMode.isActive`:
- The outer `<Link>` is swapped for a `<button>` so taps toggle
  selection instead of navigating.
- The card's long-press wrapper isn't applied — taps don't open
  the action sheet.
- Hover-only affordances (kebab, QuickReadOverlay) are hidden.

`onEnterSelectMode` is appended as a "Select" entry to the
existing [`useCoverLongPressActions`](../../web/components/CoverLongPressActions.tsx)
sheet. The long-press gesture (400 ms threshold, 8 px touchmove
cancel) is already implemented by that hook — multi-select doesn't
introduce a new gesture layer.

## Mutation hooks

| Hook                        | Endpoint                          | Use            |
|-----------------------------|-----------------------------------|----------------|
| `useBulkMarkProgress`       | `POST /me/progress/bulk`          | issue-level    |
| `useBulkMarkSeriesProgress` | `POST /me/progress/series-bulk`   | series-level   |
| `useBulkAddToCollection`    | `POST /me/collections/{id}/members/bulk-add` | any kind |
| `useBulkRemoveFromCollection` | `POST /me/collections/{id}/members/bulk-remove` | any kind |

All four hooks:
- Toast on success with a count summary
  (`"3 marked read; 1 already read"`).
- Cap requests at 500 issue ids / 100 series ids — the server
  enforces this with `400 validation`; the UI shouldn't allow
  selections past those bounds.
- Invalidate the relevant query keys on success (progress
  invalidates `queryKeys.userProgress`; collection mutations call
  `invalidateCollectionEntries` which hits four related keys).

The summary builders are pure functions exported from
[`web/lib/api/mutations.ts`](../../web/lib/api/mutations.ts) for
unit-testing — see
[`web/tests/api/bulk-progress-summary.test.ts`](../../web/tests/api/bulk-progress-summary.test.ts).

## Server endpoints

Each endpoint follows the same shape: structured count buckets
distinguishing success, no-op, and ACL/missing drops so the toast
can be specific.

### `POST /me/progress/bulk`

```jsonc
// Request
{ "issue_ids": ["..."], "finished": true, "device": "web" }

// Response
{
  "updated": 3,        // newly transitioned to the target state
  "skipped": 1,        // already in the target state
  "forbidden": 0,      // library ACL drop (non-admin)
  "not_found": 0       // issue_id didn't resolve
}
```

Cap: 500 issue ids. Server dedupes; same id 3x counts once.

### `POST /me/progress/series-bulk`

Same shape with `series_ids` instead. Each series is expanded
server-side to its active issues, then walked through the same
`upsert_for` helper as the issue-level endpoint — so the counts
are still in issues. Cap: 100 series.

```jsonc
{
  "updated": 18,
  "skipped": 2,
  "forbidden_series": 0,
  "not_found_series": 0
}
```

### `POST /me/collections/{id}/members/bulk-add`

```jsonc
// Request
{ "targets": [{ "entry_kind": "issue", "ref_id": "..." }, ...] }

// Response
{ "added": 3, "already_present": 1, "not_found": 0, "invalid": 0 }
```

Cap: 500 targets. Owner-guarded; non-owners get 403.

### `POST /me/collections/{id}/members/bulk-remove`

Same `targets` shape. Returns `{ removed, not_present, invalid }`.

## How a new list page wires it up

The minimum recipe:

```tsx
const items = data?.pages.flatMap((p) => p.items) ?? [];
const selection = useSelection(items);
const bulkMark = useBulkMarkProgress();

// Esc + Cmd/Ctrl+A while in select mode
React.useEffect(() => {
  if (!selection.selectMode) return;
  const onKey = (e: KeyboardEvent) => {
    if (shouldSkipHotkey(e)) return;
    if (e.key === "Escape") { e.preventDefault(); selection.exit(); }
    else if (e.key === "a" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      selection.selectAll();
    }
  };
  window.addEventListener("keydown", onKey);
  return () => window.removeEventListener("keydown", onKey);
}, [selection]);

// Focus restoration: after exiting select mode, focus returns to
// the trigger button so screen readers know where they are.
const selectBtnRef = React.useRef<HTMLButtonElement | null>(null);
const wasOn = React.useRef(false);
React.useEffect(() => {
  if (wasOn.current && !selection.selectMode) selectBtnRef.current?.focus();
  wasOn.current = selection.selectMode;
}, [selection.selectMode]);

return (
  <>
    {!selection.selectMode && (
      <Button ref={selectBtnRef} onClick={() => selection.enter()}>
        <ListChecks className="mr-1.5 h-4 w-4" /> Select
      </Button>
    )}
    {selection.selectMode && (
      <SelectionToolbar
        count={selection.count}
        total={items.length}
        primary={[/* container-specific actions */]}
        onDone={() => selection.exit()}
        onClear={() => selection.clear()}
        onSelectAll={() => selection.selectAll()}
        isPending={bulkMark.isPending}
      />
    )}

    {items.map((item) => (
      <Card
        key={item.id}
        item={item}
        selectMode={selection.selectMode ? {
          isActive: true,
          isSelected: selection.isSelected(item.id),
          onToggle: (ev) => selection.toggle(item.id, ev),
        } : undefined}
        onEnterSelectMode={(id) => selection.toggle(id)}
      />
    ))}
  </>
);
```

## Non-goals (v1)

- **Cross-page selection persistence.** Selection lives in
  React state on a single page; navigating away clears it. The
  alternative (URL state, localStorage) was rejected — clobbering
  selection on tab close is the right default for an action that
  fires within seconds of opening.
- **Selection across paginated pages without auto-walking.**
  Cmd+A walks every loaded page in `useInfiniteQuery`. Selection
  is bounded by what's actually loaded — there's no "all 2 000
  rows including unloaded" pattern.
- **Drag-rectangle selection.** Would conflict with the
  collection drag-reorder flow that already owns mouse-drag in
  that surface.
- **Selection state in URL.** Same reason as persistence:
  shareable URLs to a pre-selected set haven't been a real
  request.
