/**
 * Pure helpers behind the reading-lists tree table (`ReadingListRows`):
 * a folder's roll-up of its lists' counts, the read-progress sort and the
 * captions the narrower layouts fold the columns into. Free of React so
 * `tests/library/list-rows.test.ts` can pin them.
 */
import type { CblStatsView, SavedViewView } from "@/lib/api/types";

export type RowStats = Pick<
  CblStatsView,
  "total" | "matched" | "read_count" | "ambiguous" | "missing" | "manual"
>;

const ZERO: RowStats = {
  total: 0,
  matched: 0,
  read_count: 0,
  ambiguous: 0,
  missing: 0,
  manual: 0,
};

/** Sum the counts of every list in a folder. Lists whose stats have not
 *  loaded (`null`) add nothing. */
export function rollupStats(
  stats: ReadonlyArray<RowStats | null | undefined>,
): RowStats {
  const out = { ...ZERO };
  for (const s of stats) {
    if (!s) continue;
    out.total += s.total;
    out.matched += s.matched;
    out.read_count += Math.min(s.read_count, s.matched);
    out.ambiguous += s.ambiguous;
    out.missing += s.missing;
    out.manual += s.manual;
  }
  return out;
}

/** Share of the list's in-library issues the caller has read, 0–1;
 *  `null` when nothing is known (no stats, or no issue in the library). */
export function readFraction(s: RowStats | null | undefined): number | null {
  if (!s || s.matched <= 0) return null;
  return Math.min(s.read_count, s.matched) / s.matched;
}

/** Sort key `read`: most-read first, lists with nothing to read after the
 *  rest, ties by name. `statsOf` resolves a view's counts from the
 *  complete `/me/cbl-lists` response (one uncapped query), so sorting
 *  in memory never works on a truncated set. */
export function compareByRead(
  statsOf: (view: SavedViewView) => RowStats | null | undefined,
): (a: SavedViewView, b: SavedViewView) => number {
  return (a, b) => {
    const fa = readFraction(statsOf(a));
    const fb = readFraction(statsOf(b));
    if (fa !== fb) {
      if (fa === null) return 1;
      if (fb === null) return -1;
      if (fa !== fb) return fb - fa;
    }
    return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
  };
}

/** "42 of 102 read": the narrow layouts' caption head. */
export function readCaption(s: RowStats): string {
  return `${Math.min(s.read_count, s.matched)} of ${s.matched} read`;
}

/** "102 of 118": in-library over the list's length. */
export function inLibraryLabel(s: RowStats): string {
  return `${s.matched} of ${s.total}`;
}

/** "42 of 102": read over in-library. */
export function readLabel(s: RowStats): string {
  return `${Math.min(s.read_count, s.matched)} of ${s.matched}`;
}
