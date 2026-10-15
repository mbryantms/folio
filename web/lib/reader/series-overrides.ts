/**
 * Helpers for inspecting and clearing per-series reader overrides stored
 * in localStorage (key shapes: the legacy `reader:<slice>:<seriesId>` and
 * the versioned `reader.v<N>:<slice>:<seriesId>` from `prefs-storage`).
 * Pulled into a pure module so it can be unit-tested without a DOM and
 * reused by the in-reader settings popover. The account copy of the same
 * overrides is cleared separately (`useResetSeriesReaderPrefs`).
 */
export const SERIES_OVERRIDE_PREFIX = "reader:";
const VERSIONED_PREFIX = "reader.v";

const isReaderKey = (k: string) =>
  k.startsWith(SERIES_OVERRIDE_PREFIX) || k.startsWith(VERSIONED_PREFIX);

export interface KVStore {
  readonly length: number;
  key(index: number): string | null;
  removeItem(key: string): void;
}

/** Keys the reader writes for `seriesId` in localStorage, in order. */
export function seriesOverrideKeys(store: KVStore, seriesId: string): string[] {
  if (!seriesId) return [];
  const out: string[] = [];
  const suffix = `:${seriesId}`;
  for (let i = 0; i < store.length; i += 1) {
    const k = store.key(i);
    if (k && isReaderKey(k) && k.endsWith(suffix)) {
      out.push(k);
    }
  }
  return out;
}

/** True if any reader override is stored for `seriesId`. */
export function hasSeriesOverrides(store: KVStore, seriesId: string): boolean {
  return seriesOverrideKeys(store, seriesId).length > 0;
}

/** Removes every reader override for `seriesId`; returns the keys cleared. */
export function clearSeriesOverrides(
  store: KVStore & { removeItem(key: string): void },
  seriesId: string,
): string[] {
  const keys = seriesOverrideKeys(store, seriesId);
  for (const k of keys) store.removeItem(k);
  return keys;
}
