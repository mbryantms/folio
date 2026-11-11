/**
 * Pure helpers for the library-access matrix. Extracted so the dirty-state and
 * diff logic can be unit-tested without rendering a DOM.
 */

import { ladderRank } from "@/lib/age-rating";

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
 * selected ids plus the caps of selected libraries only. `hideUnrated` is
 * the set of libraries whose grant hides unrated series (the per-library
 * switch); the server turns the option off for any granted library left out.
 */
export function buildAccessRequest(
  selected: ReadonlySet<string>,
  caps: CapMap,
  hideUnrated: ReadonlySet<string> = new Set(),
): {
  library_ids: string[];
  age_rating_caps: Record<string, string>;
  hide_unrated: Record<string, boolean>;
} {
  const library_ids = Array.from(selected);
  const age_rating_caps: Record<string, string> = {};
  const hide_unrated: Record<string, boolean> = {};
  for (const id of library_ids) {
    const cap = caps.get(id);
    if (cap) age_rating_caps[id] = cap;
    if (hideUnrated.has(id)) hide_unrated[id] = true;
  }
  return { library_ids, age_rating_caps, hide_unrated };
}

/** Turn one library's "hide unrated" switch on or off. */
export function setHideUnrated(
  current: ReadonlySet<string>,
  libraryId: string,
  on: boolean,
): Set<string> {
  const next = new Set(current);
  if (on) {
    next.add(libraryId);
  } else {
    next.delete(libraryId);
  }
  return next;
}

/**
 * True when any *selected* library's "hide unrated" switch differs from the
 * saved one. Like caps, switches on unselected libraries are dropped on save.
 */
export function hideUnratedDirty(
  original: ReadonlySet<string>,
  current: ReadonlySet<string>,
  selected: ReadonlySet<string>,
): boolean {
  for (const id of selected) {
    if (original.has(id) !== current.has(id)) return true;
  }
  return false;
}

/** The per-library histogram `GET /admin/libraries/age-ratings` returns. */
export interface AgeRatingHistogram {
  rungs: ReadonlyArray<{ rating: string; series_count: number }>;
  unrated_count: number;
  series_count: number;
}

/** What one grant does to a library's series, from its histogram. */
export interface CapEffect {
  /** Series rated above the cap. */
  aboveCap: number;
  /** Series with no rating. */
  unrated: number;
  /** True when the unrated series are hidden (`hide_unrated` on). */
  unratedHidden: boolean;
  /** Every series the grant hides: `aboveCap`, plus `unrated` when hidden. */
  hidden: number;
  /** Every active series in the library. */
  total: number;
}

/**
 * Work out what a cap and the "hide unrated" switch hide, at the series
 * level, the way the server's rule does: a series ranked above the cap is
 * hidden; an unrated one is hidden only with the switch on. A cap that is
 * not a ladder rung counts as no cap (as on the server).
 */
export function capEffect(
  histogram: AgeRatingHistogram,
  cap: string | null | undefined,
  hideUnrated: boolean,
): CapEffect {
  const capRank = ladderRank(cap);
  let aboveCap = 0;
  if (capRank !== null) {
    for (const rung of histogram.rungs) {
      const rank = ladderRank(rung.rating);
      if (rank !== null && rank > capRank) aboveCap += rung.series_count;
    }
  }
  const unrated = histogram.unrated_count;
  return {
    aboveCap,
    unrated,
    unratedHidden: hideUnrated,
    hidden: aboveCap + (hideUnrated ? unrated : 0),
    total: histogram.series_count,
  };
}

/** One plain sentence describing a {@link CapEffect}. */
export function describeCapEffect(effect: CapEffect): string {
  const hides =
    effect.hidden === 0
      ? `Hides none of ${effect.total.toLocaleString()} series`
      : `Hides ${effect.hidden.toLocaleString()} of ${effect.total.toLocaleString()} series`;
  if (effect.unrated === 0) return `${hides}.`;
  const unrated = `${effect.unrated.toLocaleString()} unrated ${
    effect.unratedHidden ? "hidden" : "still shown"
  }`;
  return `${hides}; ${unrated}.`;
}
