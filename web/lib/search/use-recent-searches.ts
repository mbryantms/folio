"use client";

import { useCallback, useEffect, useState } from "react";

/** localStorage key for the recent-searches ring buffer. Versioned so a
 *  schema change (e.g. adding per-entry timestamps) can re-namespace
 *  without colliding with stale entries. */
export const RECENT_SEARCHES_STORAGE_KEY = "folio.search.recents.v1";

/** Hard cap on stored queries. Eight feels like the right limit — the
 *  modal renders chips inline above the result list, so anything past
 *  ~8 starts to push the categories down without earning real recall
 *  value. Older entries fall off the back. */
export const RECENT_SEARCHES_MAX = 8;

/** Drop queries shorter than the global search minimum so we don't
 *  store the user's intermediate keystrokes (`s`, `sa`). Matches
 *  `MIN_QUERY_LEN` in `use-search.ts`. */
export const RECENT_SEARCHES_MIN_LEN = 2;

/** Pure dedupe + cap. Exposed so tests (and any caller that wants to
 *  manipulate the list without DOM access) can exercise the same
 *  logic the hook applies inside its `setState`. Case-insensitive
 *  match preserves the casing of the most recent occurrence. */
export function appendRecent(prev: readonly string[], next: string): string[] {
  const trimmed = next.trim();
  if (trimmed.length < RECENT_SEARCHES_MIN_LEN) return prev.slice();
  const lower = trimmed.toLowerCase();
  return [trimmed, ...prev.filter((p) => p.toLowerCase() !== lower)].slice(
    0,
    RECENT_SEARCHES_MAX,
  );
}

/** Case-insensitive remove. */
export function removeRecent(
  prev: readonly string[],
  target: string,
): string[] {
  const lower = target.toLowerCase();
  return prev.filter((p) => p.toLowerCase() !== lower);
}

/** Recent-searches hook backed by `localStorage`. Returns the current
 *  list plus `add` / `remove` / `clear` actions; all mutations sync
 *  cross-tab via the `storage` event so opening the modal in a second
 *  window sees the same history.
 *
 *  The list is a ring buffer ordered most-recent-first. `add(q)`
 *  dedupes case-insensitively — typing the same query twice doesn't
 *  evict an older entry — and moves the existing entry to the front
 *  so the most recently used queries stay near the top. */
export function useRecentSearches(): {
  recents: readonly string[];
  add: (q: string) => void;
  remove: (q: string) => void;
  clear: () => void;
} {
  const [recents, setRecents] = useState<string[]>([]);

  // Hydrate from localStorage after mount. SSR returns `[]` so the
  // initial server-rendered DOM doesn't include any user-specific
  // strings — important because the modal renders these chips and
  // we don't want a hydration mismatch when the client hydrates with
  // a longer list than the server emitted.
  useEffect(() => {
    if (typeof window === "undefined") return;
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setRecents(read());
    function onStorage(e: StorageEvent) {
      if (e.key === RECENT_SEARCHES_STORAGE_KEY) setRecents(read());
    }
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  const add = useCallback((q: string) => {
    setRecents((prev) => {
      const next = appendRecent(prev, q);
      // Skip the write when nothing changed (short query, duplicate
      // at front). Avoids a redundant storage event.
      if (next.length === prev.length && next.every((v, i) => v === prev[i])) {
        return prev;
      }
      write(next);
      return next;
    });
  }, []);

  const remove = useCallback((q: string) => {
    setRecents((prev) => {
      const next = removeRecent(prev, q);
      write(next);
      return next;
    });
  }, []);

  const clear = useCallback(() => {
    setRecents(() => {
      write([]);
      return [];
    });
  }, []);

  return { recents, add, remove, clear };
}

function read(): string[] {
  try {
    const raw = window.localStorage.getItem(RECENT_SEARCHES_STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((v): v is string => typeof v === "string" && v.trim().length > 0)
      .slice(0, RECENT_SEARCHES_MAX);
  } catch {
    // Quota, parse, or access errors all degrade silently — recents
    // are a polish feature, not load-bearing for search itself.
    return [];
  }
}

function write(items: readonly string[]) {
  try {
    if (items.length === 0) {
      window.localStorage.removeItem(RECENT_SEARCHES_STORAGE_KEY);
      return;
    }
    window.localStorage.setItem(
      RECENT_SEARCHES_STORAGE_KEY,
      JSON.stringify(items),
    );
  } catch {
    // Same degrade rationale as `read`.
  }
}
