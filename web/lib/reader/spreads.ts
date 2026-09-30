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

/**
 * Per-user, per-issue manual spread controls (WP-4.3, audit R18 / UX-2),
 * persisted server-side at `/me/issues/{id}/page-overrides`. Structurally
 * the wire `PageOverridesView` minus its bookkeeping fields, so the API
 * response can be passed straight in.
 *
 * Overrides win over both automatic signals — the ComicInfo
 * `double_page` flag and the aspect-ratio heuristic:
 *  - `spread_pages`: always solo, as a spread.
 *  - `single_pages`: always an ordinary page that pairs, even when
 *    flagged or landscape.
 *  - `shift_pairing`: the first page that would start a pair is shown
 *    solo instead, shifting every later pair by one (offset scans).
 */
export interface SpreadOverrides {
  shift_pairing: boolean;
  spread_pages: ReadonlyArray<number>;
  single_pages: ReadonlyArray<number>;
}

export const EMPTY_SPREAD_OVERRIDES: SpreadOverrides = {
  shift_pairing: false,
  spread_pages: [],
  single_pages: [],
};

/** Tri-state per-page mode the strip affordance cycles through. */
export type PageSpreadMode = "auto" | "spread" | "single";

/** The override mode currently applied to page `index`. */
export function pageSpreadMode(
  overrides: SpreadOverrides | null | undefined,
  index: number,
): PageSpreadMode {
  if (!overrides) return "auto";
  if (overrides.spread_pages.includes(index)) return "spread";
  if (overrides.single_pages.includes(index)) return "single";
  return "auto";
}

/** Next mode in the strip's cycle: auto → spread → single → auto. */
export function nextPageSpreadMode(mode: PageSpreadMode): PageSpreadMode {
  if (mode === "auto") return "spread";
  if (mode === "spread") return "single";
  return "auto";
}

/**
 * Copy of `overrides` with page `index` set to `mode`. Keeps both lists
 * sorted, unique and disjoint (the server enforces the same).
 */
export function withPageSpreadMode(
  overrides: SpreadOverrides | null | undefined,
  index: number,
  mode: PageSpreadMode,
): SpreadOverrides {
  const base = overrides ?? EMPTY_SPREAD_OVERRIDES;
  const spread = base.spread_pages.filter((p) => p !== index);
  const single = base.single_pages.filter((p) => p !== index);
  if (mode === "spread") spread.push(index);
  if (mode === "single") single.push(index);
  const byNumber = (a: number, b: number) => a - b;
  return {
    shift_pairing: base.shift_pairing,
    spread_pages: spread.sort(byNumber),
    single_pages: single.sort(byNumber),
  };
}

/** True when `overrides` changes nothing (equivalent to absent). */
export function isEmptySpreadOverrides(
  overrides: SpreadOverrides | null | undefined,
): boolean {
  return (
    !overrides ||
    (!overrides.shift_pairing &&
      overrides.spread_pages.length === 0 &&
      overrides.single_pages.length === 0)
  );
}

/**
 * {@link isSpreadPage} with the user's manual overrides applied: a forced
 * spread is always a spread, a forced single never is, anything else
 * falls through to the automatic flag + aspect detection.
 */
export function isEffectiveSpread(
  pages: ReadonlyArray<PageInfo>,
  index: number,
  overrides?: SpreadOverrides | null,
): boolean {
  const mode = pageSpreadMode(overrides, index);
  if (mode === "spread") return true;
  if (mode === "single") return false;
  return isSpreadPage(pages[index]);
}

export interface SpreadOptions {
  /** When true (default), index 0 is rendered solo and pairs sync from 1. */
  coverSolo?: boolean;
  /** Manual per-issue spread controls; see {@link SpreadOverrides}. */
  overrides?: SpreadOverrides | null;
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
 *  2. If page `i` is a spread, emit `[i]` solo and advance.
 *  3. If `i + 1 < total` and page `i + 1` is not a spread, emit
 *     `[i, i + 1]` and advance by 2 — except the first time this rule
 *     fires with `shift_pairing` on, when `[i]` is emitted solo instead
 *     (the one-page shift).
 *  4. Else emit `[i]` solo and advance.
 *
 * "Spread" = a manual `spread_pages` override, else (unless the page is
 * in `single_pages`) the `double_page` flag OR a landscape aspect ratio
 * (audit C8) — see {@link isEffectiveSpread}. Manual overrides always win
 * over the automatic signals (WP-4.3). Rule (3) avoids ever pairing a
 * page with a *following* spread — the spread takes its own group on the
 * next iteration.
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
  const overrides = opts.overrides ?? null;
  const spreadAt = (idx: number) => isEffectiveSpread(pages, idx, overrides);
  let shiftPending = overrides?.shift_pairing === true;
  const groups: number[][] = [];
  let i = 0;
  while (i < total) {
    if (coverSolo && i === 0) {
      groups.push([0]);
      i = 1;
      continue;
    }
    if (spreadAt(i)) {
      groups.push([i]);
      i += 1;
      continue;
    }
    if (i + 1 < total && !spreadAt(i + 1)) {
      if (shiftPending) {
        shiftPending = false;
        groups.push([i]);
        i += 1;
        continue;
      }
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
