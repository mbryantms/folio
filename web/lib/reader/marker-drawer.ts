/**
 * In-reader marker drawer state + pure helpers (roadmap WP-5.2).
 *
 * The drawer lists every marker the user has on the open issue, in page
 * order, and jumps the reader to a marker's page on activation. It reads
 * the same `GET /me/issues/{id}/markers` cache the marker overlay already
 * fills, so opening it costs no extra request. The component is
 * lazy-loaded; this module stays tiny because the chrome button and the
 * keymap import it.
 */
import { create } from "zustand";

import type { MarkerView } from "@/lib/api/types";

type MarkerDrawerState = {
  open: boolean;
  setOpen: (open: boolean) => void;
  toggle: () => void;
};

/** Open/closed flag, kept out of the main reader store (same pattern as
 *  the page-text panel). */
export const useMarkerDrawer = create<MarkerDrawerState>((set) => ({
  open: false,
  setOpen: (open) => set({ open }),
  toggle: () => set((s) => ({ open: !s.open })),
}));

/** Reading order: page ascending, then creation time, then id. */
export function sortDrawerMarkers(items: readonly MarkerView[]): MarkerView[] {
  return [...items].sort(
    (a, b) =>
      a.page_index - b.page_index ||
      Date.parse(a.created_at) - Date.parse(b.created_at) ||
      a.id.localeCompare(b.id),
  );
}

/** Keys the drawer list owns while focus is inside it. They are stopped
 *  from reaching the reader keymap so e.g. `Home` moves focus to the first
 *  marker instead of turning to the first page, and `Space` / `Enter`
 *  activate the focused item instead of advancing the page. */
export const DRAWER_NAV_KEYS = new Set([
  "ArrowDown",
  "ArrowUp",
  "Home",
  "End",
  "Enter",
  " ",
]);

/** Roving-focus target for a navigation key, or `null` when the key
 *  doesn't move focus. Wraps at both ends. */
export function nextDrawerIndex(
  key: string,
  current: number,
  count: number,
): number | null {
  if (count === 0) return null;
  switch (key) {
    case "ArrowDown":
      return (current + 1) % count;
    case "ArrowUp":
      return (current - 1 + count) % count;
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}

/** One-line summary for a drawer row: note body, else captured text. */
export function drawerSnippet(m: MarkerView): string | null {
  return m.body?.trim() || m.selection?.text?.trim() || null;
}
