"use client";

import { useCallback, useSyncExternalStore } from "react";

/**
 * Per-list "show read chapters" toggle (linked reading lists, PR 2b).
 * Off by default: finished chapters fold so the list opens at the reader's
 * chapter. A per-viewer convenience kept in localStorage; the server
 * snapshot is the default, and a blocked store just means the default.
 * Same store pattern as `use-hide-missing`. Also holds the "hide the
 * chapter rail" preference (`useChapterRailHidden`).
 */
const PREFIX = "folio.cbl.showReadChapters.";
const listeners = new Set<() => void>();

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  window.addEventListener("storage", fn);
  return () => {
    listeners.delete(fn);
    window.removeEventListener("storage", fn);
  };
}

function read(key: string): boolean {
  try {
    return window.localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}

/** A boolean kept in localStorage under `key`; off by default. */
function useStoredFlag(key: string): [boolean, (next: boolean) => void] {
  const value = useSyncExternalStore(
    subscribe,
    () => read(key),
    () => false,
  );
  const set = useCallback(
    (next: boolean) => {
      try {
        if (next) window.localStorage.setItem(key, "1");
        else window.localStorage.removeItem(key);
      } catch {
        /* storage blocked — nothing to remember */
      }
      for (const fn of listeners) fn();
    },
    [key],
  );
  return [value, set];
}

export function useShowReadChapters(
  listId: string,
): [boolean, (next: boolean) => void] {
  return useStoredFlag(PREFIX + listId);
}

/** Whether the desktop chapter rail is hidden. One setting for every
 *  list: it is about the reader's screen, not the list. */
export function useChapterRailHidden(): [boolean, (next: boolean) => void] {
  return useStoredFlag("folio.cbl.chapterRailHidden");
}
