"use client";

import { useCallback, useSyncExternalStore } from "react";

/**
 * Global toggle for the small green/amber **collection dot** that the
 * `SeriesCard` paints in its bottom-left corner. The dot signals
 * collection-ownership state (active / complete) so users browsing
 * series rails can see at a glance which books they already own — but
 * some users prefer covers without any overlays at all, and there's
 * no other surface for them to opt out of just the dot.
 *
 * Default is ON — preserves existing behavior. When OFF the dot is
 * suppressed on every series card across the app, leaving the cover
 * art unobstructed in the bottom-left corner. The kebab menu's
 * "Add to Want to Read" / "Add to Collection…" actions are unaffected
 * by this preference — they're not visible in the resting state, only
 * after the user opens the kebab.
 *
 * Same `useSyncExternalStore` shape as `use-sidebar-section-collapse`:
 * SSR-safe with a stable server snapshot, multi-subscriber updates
 * propagate via a module-level listener set, and cross-tab toggles
 * pick up via a `storage` event.
 *
 * Storage: a single localStorage key holding the literal string
 * `"off"` when disabled. Absent / unrecognized → default ON, which
 * keeps fresh installs and pre-existing accounts on the historical
 * behavior.
 */

const STORAGE_KEY = "folio:cover-collection-dot:v1";

let cachedEnabled = true;
let lastRawRead: string | null | undefined = undefined;
const listeners = new Set<() => void>();

function readFromStorage(): boolean {
  if (typeof window === "undefined") return true;
  const raw = window.localStorage.getItem(STORAGE_KEY);
  if (raw === lastRawRead) return cachedEnabled;
  lastRawRead = raw;
  cachedEnabled = raw !== "off";
  return cachedEnabled;
}

function writeToStorage(enabled: boolean): void {
  if (typeof window === "undefined") return;
  if (enabled) {
    window.localStorage.removeItem(STORAGE_KEY);
    lastRawRead = null;
  } else {
    window.localStorage.setItem(STORAGE_KEY, "off");
    lastRawRead = "off";
  }
  cachedEnabled = enabled;
  for (const l of listeners) l();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  const onStorage = (e: StorageEvent) => {
    if (e.key !== STORAGE_KEY && e.key !== null) return;
    lastRawRead = undefined;
    readFromStorage();
    for (const l of listeners) l();
  };
  if (typeof window !== "undefined") {
    window.addEventListener("storage", onStorage);
  }
  return () => {
    listeners.delete(listener);
    if (typeof window !== "undefined") {
      window.removeEventListener("storage", onStorage);
    }
  };
}

function getServerSnapshot(): boolean {
  return true;
}

export interface CoverCollectionDotApi {
  /** True iff series cards should render their collection-ownership dot. */
  enabled: boolean;
  /** Imperative setter — used by the `CardSizeOptions` toggle. */
  setEnabled: (next: boolean) => void;
}

export function useCoverCollectionDot(): CoverCollectionDotApi {
  const enabled = useSyncExternalStore(
    subscribe,
    readFromStorage,
    getServerSnapshot,
  );
  const setEnabled = useCallback((next: boolean) => {
    if (next === readFromStorage()) return;
    writeToStorage(next);
  }, []);
  return { enabled, setEnabled };
}
