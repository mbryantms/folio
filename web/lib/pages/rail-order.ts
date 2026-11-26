/**
 * Pure order math for a page's rails (inline rail editing).
 */

/** `ids` with `id` moved `delta` places (clamped to the ends). Returns the
 *  same array when nothing moves, so callers can skip a save. */
export function moveRail(
  ids: readonly string[],
  id: string,
  delta: number,
): readonly string[] {
  const from = ids.indexOf(id);
  if (from < 0) return ids;
  const to = Math.max(0, Math.min(ids.length - 1, from + delta));
  if (to === from) return ids;
  const next = [...ids];
  next.splice(from, 1);
  next.splice(to, 0, id);
  return next;
}

/** `ids` with `id` put back at `index` (clamped); used to undo a removal. */
export function restoreRail(
  ids: readonly string[],
  id: string,
  index: number,
): string[] {
  const next = ids.filter((x) => x !== id);
  next.splice(Math.max(0, Math.min(next.length, index)), 0, id);
  return next;
}
