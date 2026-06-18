/**
 * Versioned, namespaced localStorage for reader preferences, with LRU
 * eviction of stale per-series buckets (audit H3).
 *
 * Before this, per-series reader prefs were written as bare
 * `reader:<slice>:<seriesId>` keys that accumulated forever: every series
 * ever opened left up to four keys behind (fitMode / viewMode / direction
 * / coverSolo), with no version to invalidate on a schema change and no
 * cap on growth. A heavy library reader could leave thousands of dead keys
 * in localStorage.
 *
 * This module:
 *   - namespaces + versions every key as `reader.v<N>:<slice>:<scope>`, so
 *     a future format change just bumps {@link SCHEMA_VERSION};
 *   - migrates the legacy unversioned `reader:*` keys once, preserving
 *     prefs, then drops them;
 *   - caps the number of *per-series* buckets via an MRU index, evicting
 *     the least-recently-used series' keys past {@link MAX_SERIES_BUCKETS}.
 *     The global `_default` scope (markersHidden / brightness / sepia) is
 *     never counted or evicted.
 *
 * The core is `Storage`-injected so it unit-tests without a DOM; the
 * `readerPref*` wrappers bind lazily to `window.localStorage`.
 */

/** Bump to invalidate every persisted reader pref at once. */
export const SCHEMA_VERSION = 1;
const NS = `reader.v${SCHEMA_VERSION}`;
const LEGACY_PREFIX = "reader:";

/** Scope used for prefs that are global, not per-series (markersHidden,
 *  brightness, sepia). Exempt from the per-series LRU cap. */
export const DEFAULT_SCOPE = "_default";

/** Per-series slices — the only buckets the LRU tracks + evicts. Kept here
 *  (not imported from the store) so eviction can delete a scope's keys
 *  without scanning all of localStorage. */
const PER_SERIES_SLICES = [
  "fitMode",
  "viewMode",
  "direction",
  "coverSolo",
] as const;

/** Max distinct series whose prefs we retain. 50 × 4 keys ≈ tiny; well
 *  past any realistic active-reading set, but bounded so the store can't
 *  grow without limit. */
export const MAX_SERIES_BUCKETS = 50;

const MIGRATED_KEY = `${NS}:__migrated`;
const LRU_KEY = `${NS}:__lru`;

const dataKey = (slice: string, scope: string) => `${NS}:${slice}:${scope}`;

function readLru(storage: Storage): string[] {
  const raw = storage.getItem(LRU_KEY);
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed)
      ? parsed.filter((v): v is string => typeof v === "string")
      : [];
  } catch {
    return [];
  }
}

function writeLru(storage: Storage, scopes: string[]): void {
  storage.setItem(LRU_KEY, JSON.stringify(scopes));
}

/** Delete every per-series key for a scope (one DELETE per known slice —
 *  no full-store scan needed). */
function evictScope(storage: Storage, scope: string): void {
  for (const slice of PER_SERIES_SLICES) {
    storage.removeItem(dataKey(slice, scope));
  }
}

/** Move `scope` to the front of the MRU list and evict any buckets that
 *  fall past the cap. No-op for the global default scope. */
function touchLru(storage: Storage, scope: string): void {
  if (scope === DEFAULT_SCOPE) return;
  const current = readLru(storage).filter((s) => s !== scope);
  current.unshift(scope);
  const kept = current.slice(0, MAX_SERIES_BUCKETS);
  for (const evicted of current.slice(MAX_SERIES_BUCKETS)) {
    evictScope(storage, evicted);
  }
  writeLru(storage, kept);
}

/** One-time migration of the legacy unversioned `reader:*` keys into the
 *  versioned namespace. Preserves values; seeds the LRU from whatever
 *  per-series buckets it finds (recency unknown, so order is arbitrary —
 *  it self-corrects as the user reads). Idempotent via a sentinel. */
function ensureMigrated(storage: Storage): void {
  if (storage.getItem(MIGRATED_KEY)) return;
  const legacy: string[] = [];
  for (let i = 0; i < storage.length; i++) {
    const k = storage.key(i);
    // Match `reader:<slice>:<scope>` but NOT the new `reader.v1:*` keys.
    if (k && k.startsWith(LEGACY_PREFIX) && !k.startsWith(`${NS}:`)) {
      legacy.push(k);
    }
  }
  const seenScopes = new Set<string>();
  for (const k of legacy) {
    // `reader:<slice>:<scope>` — scope may itself contain ':' in theory,
    // so split off the first two segments and rejoin the rest.
    const rest = k.slice(LEGACY_PREFIX.length);
    const firstColon = rest.indexOf(":");
    if (firstColon < 0) {
      storage.removeItem(k);
      continue;
    }
    const slice = rest.slice(0, firstColon);
    const scope = rest.slice(firstColon + 1);
    const value = storage.getItem(k);
    if (value !== null && storage.getItem(dataKey(slice, scope)) === null) {
      storage.setItem(dataKey(slice, scope), value);
    }
    if (
      scope !== DEFAULT_SCOPE &&
      (PER_SERIES_SLICES as readonly string[]).includes(slice)
    ) {
      seenScopes.add(scope);
    }
    storage.removeItem(k);
  }
  if (seenScopes.size > 0) {
    writeLru(storage, [...seenScopes].slice(0, MAX_SERIES_BUCKETS));
  }
  storage.setItem(MIGRATED_KEY, "1");
}

// ---- `Storage`-injected core (unit-tested directly) ----

export function prefGet(
  storage: Storage,
  slice: string,
  scope: string,
): string | null {
  ensureMigrated(storage);
  return storage.getItem(dataKey(slice, scope));
}

export function prefSet(
  storage: Storage,
  slice: string,
  scope: string,
  value: string,
): void {
  ensureMigrated(storage);
  storage.setItem(dataKey(slice, scope), value);
  // A write is the meaningful "use" of a series bucket — reads happen on
  // every init and would muddy recency. Touch (and maybe evict) here.
  touchLru(storage, scope);
}

// ---- Lazy `window.localStorage`-bound wrappers (used by the store) ----

function win(): Storage | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage;
  } catch {
    // Private-mode / disabled storage throws on access — degrade to
    // in-memory-less no-ops rather than crashing the reader.
    return null;
  }
}

export function readerPrefGet(slice: string, scope: string): string | null {
  const s = win();
  if (!s) return null;
  try {
    return prefGet(s, slice, scope);
  } catch {
    return null;
  }
}

export function readerPrefSet(
  slice: string,
  scope: string,
  value: string,
): void {
  const s = win();
  if (!s) return;
  try {
    prefSet(s, slice, scope, value);
  } catch {
    // Quota exceeded / serialization race — a dropped pref write is not
    // worth crashing the reader over.
  }
}
