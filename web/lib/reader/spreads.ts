/**
 * Spread-group derivation for the double-page view.
 *
 * In a printed comic the front cover is a single sheet, then the rest of
 * the book reads as left/right pairs across each binding. A double-page
 * spread is one image drawn across both halves of an opening — it must
 * always render solo. Folio's reader UI treats one "spread group" as the
 * unit shown on screen at once: a solo page index `[i]` or a pair
 * `[i, i + 1]`.
 *
 * Pure helper — no React, no DOM. Intended to be called once per
 * `pages`/`coverSolo` change in `Reader.tsx`, with the result threaded
 * through navigation, the page strip, and indicator surfaces.
 */
import type { PageInfo } from "@/lib/api/types";

export type SpreadGroup = readonly number[];

export interface SpreadOptions {
  /** When true (default), index 0 is rendered solo and pairs sync from 1. */
  coverSolo?: boolean;
}

/**
 * Walk `pages` and emit spread groups. Rules, in order:
 *
 *  1. If `coverSolo` (default true) and `i === 0`, emit `[0]` and advance.
 *  2. If `pages[i].double_page === true`, emit `[i]` solo and advance.
 *  3. If `i + 1 < pages.length` and `pages[i + 1].double_page !== true`,
 *     emit `[i, i + 1]` and advance by 2.
 *  4. Else emit `[i]` solo and advance.
 *
 * Rule (3) avoids ever pairing a page with a *following* spread — the
 * spread takes its own group on the next iteration.
 */
export function computeSpreadGroups(
  pages: ReadonlyArray<PageInfo>,
  opts: SpreadOptions = {},
): ReadonlyArray<SpreadGroup> {
  const coverSolo = opts.coverSolo ?? true;
  const groups: number[][] = [];
  let i = 0;
  while (i < pages.length) {
    if (coverSolo && i === 0) {
      groups.push([0]);
      i = 1;
      continue;
    }
    if (pages[i]?.double_page === true) {
      groups.push([i]);
      i += 1;
      continue;
    }
    if (i + 1 < pages.length && pages[i + 1]?.double_page !== true) {
      groups.push([i, i + 1]);
      i += 2;
      continue;
    }
    groups.push([i]);
    i += 1;
  }
  return groups;
}

/**
 * Given a 0-indexed page, return the index of the group containing it.
 * Returns 0 when the page is past the end (defensive).
 */
export function groupIndexForPage(
  groups: ReadonlyArray<SpreadGroup>,
  page: number,
): number {
  if (groups.length === 0) return 0;
  // Linear scan is fine — groups are typically small (≤ 100s) and we
  // call this once per render. A binary search adds complexity for no
  // measurable win in typical issues.
  for (let g = 0; g < groups.length; g += 1) {
    const grp = groups[g]!;
    if (grp.includes(page)) return g;
    if (grp[0]! > page) return Math.max(0, g - 1);
  }
  return groups.length - 1;
}

/** Anchor (first) page of a group; used to drive `setPage` from nav. */
export function firstPageOfGroup(
  groups: ReadonlyArray<SpreadGroup>,
  groupIdx: number,
): number {
  const idx = Math.max(0, Math.min(groups.length - 1, groupIdx));
  return groups[idx]?.[0] ?? 0;
}

/** Pages currently visible on screen for the given group index. */
export function visiblePagesAt(
  groups: ReadonlyArray<SpreadGroup>,
  groupIdx: number,
): readonly number[] {
  const idx = Math.max(0, Math.min(groups.length - 1, groupIdx));
  return groups[idx] ?? [];
}
