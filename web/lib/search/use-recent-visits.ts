"use client";

import { useCallback, useSyncExternalStore } from "react";

import {
  RECENT_VISITS_STORAGE_KEY,
  appendVisit,
  parseVisits,
  removeVisit,
  type RecentVisit,
} from "./recent-visits";

/**
 * Recently-visited store. Unlike recent searches (written from inside
 * the search UI that reads them), visits are written by detail pages
 * while the search modal / topbar dropdown stay mounted in the layout,
 * so the store is a tiny external store: same-tab writes notify
 * subscribers directly, other tabs come in through the `storage` event.
 *
 * Storage access is wrapped in try/catch everywhere (private windows,
 * blocked site data, quota). When storage is unusable the list lives in
 * memory for the session instead.
 */

const EMPTY: readonly RecentVisit[] = Object.freeze([]);
const listeners = new Set<() => void>();

// Snapshot cache: `useSyncExternalStore` needs a stable reference while
// the stored string is unchanged.
let cachedRaw: string | null | undefined;
let cachedValue: readonly RecentVisit[] = EMPTY;
// In-memory fallback when localStorage throws.
let memory: readonly RecentVisit[] | null = null;

function readRaw(): string | null {
  return window.localStorage.getItem(RECENT_VISITS_STORAGE_KEY);
}

function getSnapshot(): readonly RecentVisit[] {
  if (memory) return memory;
  let raw: string | null;
  try {
    raw = readRaw();
  } catch {
    return cachedValue;
  }
  if (raw !== cachedRaw) {
    cachedRaw = raw;
    const parsed = parseVisits(raw);
    cachedValue = parsed.length === 0 ? EMPTY : parsed;
  }
  return cachedValue;
}

function getServerSnapshot(): readonly RecentVisit[] {
  return EMPTY;
}

function emit() {
  for (const l of listeners) l();
}

function write(next: readonly RecentVisit[]) {
  try {
    if (next.length === 0) {
      window.localStorage.removeItem(RECENT_VISITS_STORAGE_KEY);
    } else {
      window.localStorage.setItem(
        RECENT_VISITS_STORAGE_KEY,
        JSON.stringify(next),
      );
    }
    memory = null;
  } catch {
    memory = next.length === 0 ? EMPTY : next;
  }
  emit();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  function onStorage(e: StorageEvent) {
    if (e.key === null || e.key === RECENT_VISITS_STORAGE_KEY) listener();
  }
  window.addEventListener("storage", onStorage);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("storage", onStorage);
  };
}

/** Record one visit (moves an existing entry to the front). Safe to
 *  call from effects; a no-op on the server. */
export function recordVisit(visit: RecentVisit): void {
  if (typeof window === "undefined") return;
  const prev = getSnapshot();
  const next = appendVisit(prev, visit);
  const unchanged =
    next.length === prev.length && next.every((v, i) => sameEntry(v, prev[i]));
  if (!unchanged) write(next);
}

function sameEntry(a: RecentVisit, b: RecentVisit | undefined): boolean {
  return (
    !!b &&
    a.kind === b.kind &&
    a.id === b.id &&
    a.title === b.title &&
    (a.subtitle ?? null) === (b.subtitle ?? null) &&
    a.href === b.href &&
    (a.thumbUrl ?? null) === (b.thumbUrl ?? null)
  );
}

export function useRecentVisits(): {
  visits: readonly RecentVisit[];
  remove: (v: Pick<RecentVisit, "kind" | "id">) => void;
  clear: () => void;
} {
  const visits = useSyncExternalStore(
    subscribe,
    getSnapshot,
    getServerSnapshot,
  );
  const remove = useCallback(
    (v: Pick<RecentVisit, "kind" | "id">) =>
      write(removeVisit(getSnapshot(), v)),
    [],
  );
  const clear = useCallback(() => write([]), []);
  return { visits, remove, clear };
}

/** Test hook: forget the module-level caches between cases. */
export function __resetRecentVisitsStoreForTests(): void {
  cachedRaw = undefined;
  cachedValue = EMPTY;
  memory = null;
}
