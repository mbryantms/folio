"use client";

import * as React from "react";

/** Covers or rows on the Upcoming page. Saved per browser. */
export type UpcomingLayout = "grid" | "list";

export const UPCOMING_LAYOUT_KEY = "folio.upcoming.layout";

const listeners = new Set<() => void>();
// The choice for this page load when storage is unavailable (private mode).
let memory: UpcomingLayout | null = null;

function read(): UpcomingLayout {
  if (memory) return memory;
  try {
    return window.localStorage.getItem(UPCOMING_LAYOUT_KEY) === "list"
      ? "list"
      : "grid";
  } catch {
    return "grid";
  }
}

function subscribe(onChange: () => void): () => void {
  listeners.add(onChange);
  window.addEventListener("storage", onChange);
  return () => {
    listeners.delete(onChange);
    window.removeEventListener("storage", onChange);
  };
}

/** The saved layout. The server and the first client render use the
 *  grid, so a saved "list" switches in after hydration without a
 *  mismatch. */
export function useUpcomingLayout(): [
  UpcomingLayout,
  (next: UpcomingLayout) => void,
] {
  const layout = React.useSyncExternalStore<UpcomingLayout>(
    subscribe,
    read,
    () => "grid",
  );
  const set = React.useCallback((next: UpcomingLayout) => {
    memory = next;
    try {
      window.localStorage.setItem(UPCOMING_LAYOUT_KEY, next);
    } catch {
      // Storage is unavailable: `memory` holds the choice.
    }
    for (const notify of listeners) notify();
  }, []);
  return [layout, set];
}
