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

/**
 * Width ÷ height at or above which a page is treated as a two-page spread
 * even when `double_page` metadata is absent (audit C8). A single comic
 * page is portrait (~0.65); a spread is landscape (~1.3+). 1.2 sits safely
 * between, so a slightly-wide single page never trips it but any genuine
 * double-wide scan does.
 */
export const SPREAD_ASPECT_RATIO = 1.2;

/**
 * Does this page read as a two-page spread? True when the `double_page`
 * flag is set OR, failing that, the intrinsic dimensions are landscape
 * past {@link SPREAD_ASPECT_RATIO}. Dimensionless pages with no flag fall
 * back to `false` (paired normally). Many archives omit `double_page`, so
 * the aspect heuristic is what stops a wide spread from being jammed into
 * half a pane next to an unrelated page.
 */
export function isSpreadPage(page: PageInfo | undefined): boolean {
  if (!page) return false;
  if (page.double_page === true) return true;
  const w = page.image_width;
  const h = page.image_height;
  if (typeof w === "number" && typeof h === "number" && w > 0 && h > 0) {
    return w / h >= SPREAD_ASPECT_RATIO;
  }
  return false;
}

export interface SpreadOptions {
  /** When true (default), index 0 is rendered solo and pairs sync from 1. */
  coverSolo?: boolean;
  /**
   * Authoritative page count for the issue. ComicInfo's `<Pages>`
   * element is *optional metadata* — some publishers ship it truncated
   * (e.g. only the cover) even when `<PageCount>` is the full count.
   * When `totalPages` is provided the walker iterates [0, totalPages),
   * looking up `pages[i]?.double_page` defensively (missing entries
   * default to false). When omitted, falls back to `pages.length` for
   * backward compatibility with callers that don't know the count.
   */
  totalPages?: number;
}

/**
 * Walk pages and emit spread groups. Rules, in order:
 *
 *  1. If `coverSolo` (default true) and `i === 0`, emit `[0]` and advance.
 *  2. If `isSpreadPage(pages[i])`, emit `[i]` solo and advance.
 *  3. If `i + 1 < total` and `!isSpreadPage(pages[i + 1])`,
 *     emit `[i, i + 1]` and advance by 2.
 *  4. Else emit `[i]` solo and advance.
 *
 * "Spread" = the `double_page` flag OR a landscape aspect ratio (audit
 * C8) — see {@link isSpreadPage}. Rule (3) avoids ever pairing a page with
 * a *following* spread — the spread takes its own group on the next
 * iteration.
 *
 * `total` is `opts.totalPages ?? pages.length`. The `pages[]` array is
 * a metadata side-table consulted for the `double_page` flag; missing
 * entries are treated as `double_page: false`.
 */
export function computeSpreadGroups(
  pages: ReadonlyArray<PageInfo>,
  opts: SpreadOptions = {},
): ReadonlyArray<SpreadGroup> {
  const coverSolo = opts.coverSolo ?? true;
  const total = Math.max(0, opts.totalPages ?? pages.length);
  const groups: number[][] = [];
  let i = 0;
  while (i < total) {
    if (coverSolo && i === 0) {
      groups.push([0]);
      i = 1;
      continue;
    }
    if (isSpreadPage(pages[i])) {
      groups.push([i]);
      i += 1;
      continue;
    }
    if (i + 1 < total && !isSpreadPage(pages[i + 1])) {
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
