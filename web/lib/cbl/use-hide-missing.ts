"use client";

import { useCallback, useSyncExternalStore } from "react";

/**
 * Per-list "hide missing entries" toggle, persisted in localStorage and
 * synced across consumers within the same tab via a module-level
 * listener bus. Used in two surfaces that must stay in lockstep: the
 * three-dot menu on the consumption view (`CblViewDetail`) and the
 * Settings tab inside the management sheet (`CblDetail` → SettingsTab).
 *
 * Storage key is per-list so each CBL list keeps its own preference.
 * Across tabs we rely on the browser `storage` event for sync.
 */

const STORAGE_PREFIX = "folio.cbl.hideMissing.";

const listeners = new Map<string, Set<() => void>>();

function notify(key: string) {
  const bag = listeners.get(key);
  if (!bag) return;
  for (const fn of bag) fn();
}

function subscribeKey(key: string, fn: () => void): () => void {
  let bag = listeners.get(key);
  if (!bag) {
    bag = new Set();
    listeners.set(key, bag);
  }
  bag.add(fn);
  // Cross-tab sync: storage events only fire in OTHER tabs, so this
  // keeps multiple windows agreed without us having to broadcast.
  const onStorage = (e: StorageEvent) => {
    if (e.key === key) fn();
  };
  window.addEventListener("storage", onStorage);
  return () => {
    bag!.delete(fn);
    window.removeEventListener("storage", onStorage);
  };
}

function readBool(key: string): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}

function writeBool(key: string, value: boolean): void {
  if (typeof window === "undefined") return;
  try {
    if (value) window.localStorage.setItem(key, "1");
    else window.localStorage.removeItem(key);
  } catch {
    /* private-window or storage-full — silent */
  }
}

export function useCblHideMissing(
  listId: string,
): readonly [boolean, (next: boolean) => void] {
  const key = STORAGE_PREFIX + listId;
  const subscribe = useCallback(
    (onChange: () => void) => subscribeKey(key, onChange),
    [key],
  );
  const get = useCallback(() => readBool(key), [key]);
  // Server snapshot — defaults to false so SSR renders the "show all"
  // view; client takes over on hydration.
  const value = useSyncExternalStore(subscribe, get, () => false);
  const set = useCallback(
    (next: boolean) => {
      writeBool(key, next);
      notify(key);
    },
    [key],
  );
  return [value, set] as const;
}
