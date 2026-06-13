"use client";

import * as React from "react";

/**
 * Audit B15 — designed scroll restoration for the window-virtualized
 * grid. With virtualization the grid is short on mount (pages load
 * async), so the browser's native restoration on back-nav clamps to a
 * too-shallow offset (observed: a 2988px position restores to ~1100px).
 *
 * This hook saves the resting scroll position keyed by the grid's URL,
 * and on a back/forward `popstate` pages content in until it's tall
 * enough, then restores the exact offset. Two subtleties learned the
 * hard way:
 *
 *  - It triggers on the **popstate event**, not just on mount — the grid
 *    can stay in the App-Router cache across a back-nav, so a
 *    mount-only effect never re-fires.
 *  - It **suppresses saving while restoring**, because native
 *    restoration scrolls the window (to the wrong, shallow offset) the
 *    instant we arrive — without the guard that scroll would overwrite
 *    the saved offset before we get to read it.
 *
 * It only acts on `popstate` (back/forward), never on a forward push
 * (which `RouteChangeReset` sends to the top) or a filter change (a new
 * URL → a new storage key → nothing to restore).
 */

const KEY_PREFIX = "folio:grid:scroll:";
/** The URL fully encodes the grid's filter state (facets sync to the
 *  URL), so it's the natural restore key. */
function storageKey(): string {
  if (typeof window === "undefined") return KEY_PREFIX;
  return `${KEY_PREFIX}${window.location.pathname}${window.location.search}`;
}

// Module-level back-nav tracking. On a pop we (a) timestamp it so a
// remount can tell it followed a back-nav, and (b) open a short
// save-suppression window: the browser's native restoration scrolls the
// window the instant we arrive, and that scroll must NOT overwrite the
// saved offset before the restore reads it. Suppressing saves for a beat
// after every pop kills that race deterministically, independent of when
// the restore's `requestAnimationFrame` runs.
let lastPopAt = 0;
let suppressSaveUntil = 0;
const POP_SAVE_SUPPRESS_MS = 1500;
if (typeof window !== "undefined") {
  window.addEventListener("popstate", () => {
    lastPopAt = Date.now();
    suppressSaveUntil = lastPopAt + POP_SAVE_SUPPRESS_MS;
  });
}
function mountFollowedBackNav(withinMs = 2000): boolean {
  return lastPopAt > 0 && Date.now() - lastPopAt < withinMs;
}
function savesSuppressed(): boolean {
  return Date.now() < suppressSaveUntil;
}

/** Hard cap on the page-in loop so a corrupt/huge saved offset can't
 *  walk the whole library. */
const MAX_PAGE_INS = 40;

export function useGridScrollRestore({
  enabled,
  getTotalSize,
  growthSignal,
  hasNextPage,
  isFetchingNextPage,
  fetchNextPage,
}: {
  enabled: boolean;
  getTotalSize: () => number;
  /** Increases as pages load (row count) — drives the page-in loop. */
  growthSignal: number;
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  fetchNextPage: () => void;
}): void {
  const restoringRef = React.useRef(false);
  const targetRef = React.useRef(0);
  const pageInsRef = React.useRef(0);
  const [restoreNonce, setRestoreNonce] = React.useState(0);

  // Begin a restore: read the saved offset (before any scroll can
  // clobber it), latch `restoring` to suppress saves, and kick the
  // page-in/scroll effect.
  const begin = React.useCallback(() => {
    let saved = 0;
    try {
      saved = parseInt(sessionStorage.getItem(storageKey()) ?? "0", 10);
    } catch {
      saved = 0;
    }
    if (!Number.isFinite(saved) || saved <= 0) return;
    targetRef.current = saved;
    pageInsRef.current = 0;
    restoringRef.current = true;
    setRestoreNonce((n) => n + 1);
  }, []);

  // Trigger on popstate (component stayed mounted) AND on a mount that
  // followed a popstate (route remounted). `requestAnimationFrame` lets
  // `location` settle to the popped entry before we read its key.
  React.useEffect(() => {
    if (!enabled) return;
    if (mountFollowedBackNav()) requestAnimationFrame(begin);
    const onPop = () => requestAnimationFrame(begin);
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, [enabled, begin]);

  // Drive the restore while latched: page in until the content is tall
  // enough to honor the offset, then scroll to it.
  React.useEffect(() => {
    if (!restoringRef.current) return;
    const target = targetRef.current;
    const tallEnough = getTotalSize() >= target + window.innerHeight;
    if (tallEnough || !hasNextPage || pageInsRef.current >= MAX_PAGE_INS) {
      window.scrollTo(0, target);
      restoringRef.current = false;
    } else if (!isFetchingNextPage) {
      pageInsRef.current += 1;
      fetchNextPage();
    }
  }, [
    restoreNonce,
    growthSignal,
    getTotalSize,
    hasNextPage,
    isFetchingNextPage,
    fetchNextPage,
  ]);

  // Persist the resting scroll position (debounced), keyed by URL — but
  // never while a restore is in flight (native/our own restore scrolls
  // would otherwise clobber the saved offset).
  React.useEffect(() => {
    if (!enabled) return;
    let t: ReturnType<typeof setTimeout> | null = null;
    const onScroll = () => {
      if (restoringRef.current || savesSuppressed()) return;
      if (t) clearTimeout(t);
      t = setTimeout(() => {
        if (restoringRef.current || savesSuppressed()) return;
        try {
          sessionStorage.setItem(
            storageKey(),
            String(Math.round(window.scrollY)),
          );
        } catch {
          // private mode / blocked storage — restoration just won't fire
        }
      }, 150);
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      window.removeEventListener("scroll", onScroll);
      if (t) clearTimeout(t);
    };
  }, [enabled]);
}
