/**
 * Pure helpers for the library-access matrix. Extracted so the dirty-state and
 * diff logic can be unit-tested without rendering a DOM.
 */

export function toggleSelection(
  current: ReadonlySet<string>,
  libraryId: string,
): Set<string> {
  const next = new Set(current);
  if (next.has(libraryId)) {
    next.delete(libraryId);
  } else {
    next.add(libraryId);
  }
  return next;
}

export function selectionDiff(
  original: ReadonlySet<string>,
  selected: ReadonlySet<string>,
): { added: string[]; removed: string[] } {
  const added: string[] = [];
  const removed: string[] = [];
  for (const id of selected) {
    if (!original.has(id)) added.push(id);
  }
  for (const id of original) {
    if (!selected.has(id)) removed.push(id);
  }
  return { added, removed };
}

export function isDirty(
  original: ReadonlySet<string>,
  selected: ReadonlySet<string>,
): boolean {
  if (original.size !== selected.size) return true;
  for (const id of selected) {
    if (!original.has(id)) return true;
  }
  return false;
}

/** Per-library age-rating cap; a missing key = uncapped. */
export type CapMap = ReadonlyMap<string, string>;

/**
 * True when any *selected* library's cap differs from the saved one. Caps on
 * unselected libraries are ignored — they're dropped on save, so they can't
 * make the form dirty on their own.
 */
export function capsDirty(
  original: CapMap,
  caps: CapMap,
  selected: ReadonlySet<string>,
): boolean {
  for (const id of selected) {
    if ((original.get(id) ?? null) !== (caps.get(id) ?? null)) return true;
  }
  return false;
}

/** Set (non-empty) or clear (empty string / null) one library's cap. */
export function setCap(
  current: CapMap,
  libraryId: string,
  cap: string | null,
): Map<string, string> {
  const next = new Map(current);
  if (cap) {
    next.set(libraryId, cap);
  } else {
    next.delete(libraryId);
  }
  return next;
}

/**
 * Request-body shape for `POST /admin/users/{id}/library-access`: the
 * selected ids plus the caps of selected libraries only.
 */
export function buildAccessRequest(
  selected: ReadonlySet<string>,
  caps: CapMap,
): { library_ids: string[]; age_rating_caps: Record<string, string> } {
  const library_ids = Array.from(selected);
  const age_rating_caps: Record<string, string> = {};
  for (const id of library_ids) {
    const cap = caps.get(id);
    if (cap) age_rating_caps[id] = cap;
  }
  return { library_ids, age_rating_caps };
}
