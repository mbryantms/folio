"use client";

import { useCallback, useSyncExternalStore } from "react";

/**
 * Per-user-header sidebar-section collapse state.
 *
 * Each `kind="header"` row in the sidebar layout produces a section
 * with a stable `ref_id` (server-allocated UUID, doesn't change when
 * the header is renamed or reordered). This hook tracks, per ref_id,
 * whether the section is collapsed. The store is a single JSON map
 * in `localStorage`; absent entries default to **open** so a fresh
 * account shows everything until the user explicitly closes a
 * section.
 *
 * Storage layout: `{ [headerRefId]: true }`. Only collapsed entries
 * are persisted — toggling a collapsed section open deletes the key
 * rather than writing `false`, which keeps the JSON minimal and
 * means a deleted-then-recreated header reverts to the open default.
 *
 * SSR + multi-subscriber safety: backed by `useSyncExternalStore`
 * with a module-level subscriber set so every consumer (sidebar
 * rendering, future settings UI, etc.) sees the same value and
 * re-renders together when any one of them toggles. The server
 * snapshot is "no entries collapsed" so the SSR pass produces the
 * same markup we'd produce client-side before hydration.
 *
 * Persistence is intentionally device-local: cross-device sync of
 * collapse state would need a new column on `sidebar_entries` and a
 * write-back endpoint on every toggle. localStorage is faster to
 * ship and matches the "minor view preference" mental model — users
 * tend to want different open/closed sets on phone vs. desktop
 * anyway.
 */

const STORAGE_KEY = "folio:sidebar:section-collapse:v1";

type Snapshot = Readonly<Record<string, true>>;

const EMPTY_SNAPSHOT: Snapshot = Object.freeze({});

let cachedSnapshot: Snapshot = EMPTY_SNAPSHOT;
let lastRawRead: string | null = null;
const listeners = new Set<() => void>();

function readSnapshotFromStorage(): Snapshot {
  if (typeof window === "undefined") return EMPTY_SNAPSHOT;
  const raw = window.localStorage.getItem(STORAGE_KEY);
  if (raw === lastRawRead) return cachedSnapshot;
  lastRawRead = raw;
  if (!raw) {
    cachedSnapshot = EMPTY_SNAPSHOT;
    return cachedSnapshot;
  }
  try {
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    const next: Record<string, true> = {};
    for (const [k, v] of Object.entries(parsed)) {
      if (v === true && typeof k === "string" && k.length > 0) {
        next[k] = true;
      }
    }
    cachedSnapshot = Object.freeze(next);
  } catch {
    // Malformed value (likely from a previous schema or hand-edit) —
    // fall back to the empty default rather than crashing every
    // sidebar render.
    cachedSnapshot = EMPTY_SNAPSHOT;
  }
  return cachedSnapshot;
}

function writeSnapshot(next: Snapshot): void {
  if (typeof window === "undefined") return;
  if (Object.keys(next).length === 0) {
    window.localStorage.removeItem(STORAGE_KEY);
    lastRawRead = null;
  } else {
    const raw = JSON.stringify(next);
    window.localStorage.setItem(STORAGE_KEY, raw);
    lastRawRead = raw;
  }
  cachedSnapshot = Object.freeze(next);
  for (const l of listeners) l();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  // `storage` events fire only on OTHER tabs, but a multi-tab user
  // toggling on one tab and switching to another should see the new
  // state without a refresh. The handler refreshes the cached
  // snapshot and notifies local subscribers.
  const onStorage = (e: StorageEvent) => {
    if (e.key !== STORAGE_KEY && e.key !== null) return;
    lastRawRead = null;
    readSnapshotFromStorage();
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

function getServerSnapshot(): Snapshot {
  return EMPTY_SNAPSHOT;
}

export interface SidebarSectionCollapseApi {
  /** True iff the section opened by this header is currently collapsed. */
  isCollapsed: (headerRefId: string) => boolean;
  /** Flip the collapse state for a header. Open → collapsed → open. */
  toggle: (headerRefId: string) => void;
  /** Imperative setter, useful from settings UI flows. */
  setCollapsed: (headerRefId: string, collapsed: boolean) => void;
}

export function useSidebarSectionCollapse(): SidebarSectionCollapseApi {
  const snapshot = useSyncExternalStore(
    subscribe,
    readSnapshotFromStorage,
    getServerSnapshot,
  );
  const setCollapsed = useCallback(
    (headerRefId: string, collapsed: boolean) => {
      const current = readSnapshotFromStorage();
      const isCurrentlyCollapsed = current[headerRefId] === true;
      if (collapsed === isCurrentlyCollapsed) return;
      const next: Record<string, true> = { ...current };
      if (collapsed) {
        next[headerRefId] = true;
      } else {
        delete next[headerRefId];
      }
      writeSnapshot(Object.freeze(next));
    },
    [],
  );
  const toggle = useCallback(
    (headerRefId: string) => {
      const current = readSnapshotFromStorage();
      setCollapsed(headerRefId, !(current[headerRefId] === true));
    },
    [setCollapsed],
  );
  const isCollapsed = useCallback(
    (headerRefId: string) => snapshot[headerRefId] === true,
    [snapshot],
  );
  return { isCollapsed, toggle, setCollapsed };
}
