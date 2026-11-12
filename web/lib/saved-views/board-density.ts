"use client";

import { useCallback, useSyncExternalStore } from "react";

/**
 * Per-browser row density of a board's rows presentation (the reading
 * lists table): `compact` (one line per row where it fits) or
 * `comfortable` (name over description, the default). Stored in `localStorage` under
 * one key per board; every access is guarded, so a private window or
 * blocked storage keeps the choice in memory for the page's life and
 * starts from the default. The server snapshot is the default too, so
 * SSR and the first client render agree.
 */
export type BoardDensity = "compact" | "comfortable";

export const DEFAULT_DENSITY: BoardDensity = "comfortable";

const listeners = new Set<() => void>();
/** Choices made while storage throws. */
const memory = new Map<string, BoardDensity>();

function isDensity(v: unknown): v is BoardDensity {
  return v === "compact" || v === "comfortable";
}

export function readDensity(key: string): BoardDensity {
  try {
    const v = window.localStorage.getItem(key);
    return isDensity(v) ? v : DEFAULT_DENSITY;
  } catch {
    // Storage unavailable: the choice made on this page, if any.
    return memory.get(key) ?? DEFAULT_DENSITY;
  }
}

export function writeDensity(key: string, next: BoardDensity) {
  memory.set(key, next);
  try {
    window.localStorage.setItem(key, next);
  } catch {
    // storage unavailable: `memory` carries it
  }
  for (const l of listeners) l();
}

function subscribe(cb: () => void) {
  listeners.add(cb);
  window.addEventListener("storage", cb);
  return () => {
    listeners.delete(cb);
    window.removeEventListener("storage", cb);
  };
}

export function useBoardDensity(
  key: string,
): [BoardDensity, (next: BoardDensity) => void] {
  const density = useSyncExternalStore(
    subscribe,
    () => readDensity(key),
    () => DEFAULT_DENSITY,
  );
  const set = useCallback(
    (next: BoardDensity) => writeDensity(key, next),
    [key],
  );
  return [density, set];
}
