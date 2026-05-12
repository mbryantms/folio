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
