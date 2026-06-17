"use client";

import * as React from "react";

/**
 * Per-page multi-select state for list surfaces (series detail,
 * collection detail, view detail, CBL detail). The hook is
 * deliberately container-shaped — every list page owns its own
 * instance and resets on navigation. Lifting to a global store
 * was considered and rejected: cross-page selection isn't a real
 * user need, and a global selectedIds set invites stale UI
 * between tabs.
 *
 * Plan: `~/.claude/plans/multi-select-bulk-actions-1.0.md` (M1).
 *
 * ### Returned API
 *
 *   - `selected: ReadonlySet<string>` — current selection.
 *   - `count: number` — convenience, same as `selected.size`.
 *   - `isSelected(id)` — predicate; cheap.
 *   - `toggle(id, ev?)` — flip selection for `id`. When `ev` is
 *     a `MouseEvent`-like with `shiftKey === true` AND the
 *     previous toggle remembered a "last clicked" id, every
 *     item between that anchor and `id` (in the items[] order)
 *     gets selected.
 *   - `selectRange(fromId, toId)` — explicit range select
 *     primitive without needing a mouse event (used by tests).
 *   - `selectAll()` — populate with every item id; if the items
 *     iterable is paginated via `useInfiniteQuery`, the caller
 *     is responsible for walking pages before calling.
 *   - `clear()` — empty the selection; stays in select mode.
 *   - `replace(ids)` — swap the selection for an explicit id set,
 *     staying in select mode (used to keep just the skipped items
 *     selected after a partial bulk action).
 *   - `exit()` — leave select mode entirely. Distinct from
 *     `clear` so the toolbar can offer both affordances on
 *     mobile (no `Esc` key).
 *   - `selectMode: boolean` — true when select mode is active.
 *     Becomes true on first `toggle`/`selectAll`/`extend`,
 *     becomes false on `exit`. The card components key their
 *     "navigate vs. toggle" branching off this.
 *
 * Cmd/Ctrl+A handling is wired by the consuming page (it needs
 * to know whether items[] is the whole list or just one page of
 * an infinite query). The hook exposes `selectAll` as the
 * primitive; the page binds the keyboard event.
 */
export function useSelection<T extends { id: string }>(items: T[]) {
  const [selected, setSelected] = React.useState<Set<string>>(() => new Set());
  // Set by `exit()` / `clear()` callers; also flips automatically
  // when the selection becomes non-empty.
  const [selectMode, setSelectMode] = React.useState(false);
  // The last id toggled by a non-range click — anchor for Shift-click
  // ranges. Cleared on exit.
  const anchorRef = React.useRef<string | null>(null);

  // Items can change underneath us (filter changes, infinite scroll
  // adds more). Build an id→index map per render so range select
  // stays correct.
  const idToIndex = React.useMemo(() => buildIdIndex(items), [items]);

  const isSelected = React.useCallback(
    (id: string) => selected.has(id),
    [selected],
  );

  const toggle = React.useCallback(
    (id: string, ev?: { shiftKey?: boolean }) => {
      setSelected((prev) =>
        computeToggle(prev, id, items, idToIndex, anchorRef.current, ev),
      );
      // Anchor on the latest non-range click. Range clicks do not
      // update the anchor — matches Finder/Explorer behavior.
      if (!ev?.shiftKey) anchorRef.current = id;
      setSelectMode(true);
    },
    [items, idToIndex],
  );

  const selectRange = React.useCallback(
    (fromId: string, toId: string) => {
      setSelected((prev) =>
        computeRangeAdd(prev, fromId, toId, items, idToIndex),
      );
      anchorRef.current = toId;
      setSelectMode(true);
    },
    [items, idToIndex],
  );

  const selectAll = React.useCallback(() => {
    setSelected(new Set(items.map((i) => i.id)));
    setSelectMode(true);
  }, [items]);

  const enter = React.useCallback(() => {
    setSelectMode(true);
  }, []);

  const clear = React.useCallback(() => {
    setSelected(new Set());
    anchorRef.current = null;
  }, []);

  // Replace the selection with an explicit id set, staying in select mode.
  // Used after a partial bulk action to keep just the leftover (e.g.
  // skipped) items selected so the operator can fix + retry them.
  const replace = React.useCallback((ids: readonly string[]) => {
    setSelected(new Set(ids));
    setSelectMode(true);
    anchorRef.current = null;
  }, []);

  const exit = React.useCallback(() => {
    setSelected(new Set());
    setSelectMode(false);
    anchorRef.current = null;
  }, []);

  // Reset everything when items[] is replaced (e.g. user navigates
  // between series pages without unmounting the hook). Detected by
  // a swap of the items reference, which the consumer must keep
  // stable across renders. React Query already memoizes its data
  // refs across pagination updates so this is correct: paginating
  // appends new items but the prior items keep their identity.
  // A genuine new page (different series slug) replaces the array
  // wholesale and gets the reset.
  const lastItemsRef = React.useRef(items);
  React.useEffect(() => {
    if (lastItemsRef.current === items) return;
    // Don't reset on simple length-extension (infinite scroll adds
    // to the end). Only reset when the FIRST item changes — that
    // indicates a navigation, not an append.
    const prev = lastItemsRef.current;
    if (prev[0]?.id !== items[0]?.id) {
      setSelected(new Set());
      setSelectMode(false);
      anchorRef.current = null;
    }
    lastItemsRef.current = items;
  }, [items]);

  return {
    selected: selected as ReadonlySet<string>,
    count: selected.size,
    selectMode,
    isSelected,
    toggle,
    selectRange,
    selectAll,
    enter,
    clear,
    replace,
    exit,
  };
}

// ─────── Pure helpers, exported for unit tests ───────

/**
 * Compute the next selection set for a click on `id`. When the event
 * carries `shiftKey` AND an `anchor` is known AND both ids exist in
 * `items`, every item between them (inclusive, in `items` order) is
 * added to the existing selection. Otherwise it's a simple toggle.
 *
 * Pure function — no React state. Tested directly via vitest.
 */
export function computeToggle<T extends { id: string }>(
  prev: ReadonlySet<string>,
  id: string,
  items: readonly T[],
  idToIndex: ReadonlyMap<string, number>,
  anchor: string | null,
  ev?: { shiftKey?: boolean },
): Set<string> {
  const next = new Set(prev);
  const wantRange = ev?.shiftKey === true && anchor !== null && anchor !== id;
  if (wantRange) {
    const a = idToIndex.get(anchor);
    const b = idToIndex.get(id);
    if (a !== undefined && b !== undefined) {
      const [lo, hi] = a < b ? [a, b] : [b, a];
      for (let i = lo; i <= hi; i += 1) {
        const it = items[i];
        if (it) next.add(it.id);
      }
      return next;
    }
  }
  if (next.has(id)) {
    next.delete(id);
  } else {
    next.add(id);
  }
  return next;
}

/**
 * Add a closed range `[fromId..toId]` (in either order) to an
 * existing selection. Used by `selectRange` for keyboard-driven
 * extend operations. Pure.
 */
export function computeRangeAdd<T extends { id: string }>(
  prev: ReadonlySet<string>,
  fromId: string,
  toId: string,
  items: readonly T[],
  idToIndex: ReadonlyMap<string, number>,
): Set<string> {
  const a = idToIndex.get(fromId);
  const b = idToIndex.get(toId);
  if (a === undefined || b === undefined) return new Set(prev);
  const [lo, hi] = a < b ? [a, b] : [b, a];
  const next = new Set(prev);
  for (let i = lo; i <= hi; i += 1) {
    const it = items[i];
    if (it) next.add(it.id);
  }
  return next;
}

/** Build the id→index map for `computeToggle` / `computeRangeAdd`. */
export function buildIdIndex<T extends { id: string }>(
  items: readonly T[],
): Map<string, number> {
  const m = new Map<string, number>();
  items.forEach((item, i) => m.set(item.id, i));
  return m;
}
