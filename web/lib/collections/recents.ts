/**
 * Recently-used collections (audit 3.7). Curating a library usually means
 * dropping several items into the same handful of collections in a row —
 * but the add-to-collection picker only ever showed Want to Read pinned
 * first and then an alphabetical wall, so the collection you *just* used
 * sank back into the list. This tracks a small MRU of collection ids in
 * localStorage and surfaces them in a "Recent" group at the top of the
 * picker.
 *
 * Versioned key so the format can change cleanly; capped so it can't grow.
 * SSR-/private-mode-safe (storage access is guarded).
 */

const STORAGE_KEY = "folio.collection-recents.v1";
/** How many recents to retain + surface. Small enough to stay a glanceable
 *  shortcut row, not a second full list. */
const MAX_RECENTS = 6;

function storage(): Storage | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function read(): string[] {
  const s = storage();
  if (!s) return [];
  try {
    const raw = s.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed)
      ? parsed.filter((v): v is string => typeof v === "string")
      : [];
  } catch {
    return [];
  }
}

/** Record that a collection was just used (added to / created). Moves it to
 *  the front of the MRU list, de-duped and capped. Best-effort — a failed
 *  write is silently ignored. */
export function recordCollectionUse(id: string): void {
  const s = storage();
  if (!s || !id) return;
  const next = [id, ...read().filter((x) => x !== id)].slice(0, MAX_RECENTS);
  try {
    s.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // Quota / disabled storage — recents are a convenience, not load-bearing.
  }
}

/** The recent collection ids, most-recent first. */
export function getRecentCollectionIds(): string[] {
  return read();
}

/**
 * Split a collection list into `[recent, rest]` given the MRU id list.
 * `recent` is in MRU order and contains only ids still present in
 * `collections`; `rest` keeps the input order (the caller's alpha sort)
 * minus anything promoted to `recent`. Pure — unit-tested.
 */
export function partitionByRecents<T extends { id: string }>(
  collections: readonly T[],
  recentIds: readonly string[],
): { recent: T[]; rest: T[] } {
  const byId = new Map(collections.map((c) => [c.id, c]));
  const recent: T[] = [];
  const seen = new Set<string>();
  for (const id of recentIds) {
    const c = byId.get(id);
    if (c && !seen.has(id)) {
      recent.push(c);
      seen.add(id);
    }
  }
  const rest = collections.filter((c) => !seen.has(c.id));
  return { recent, rest };
}
