/**
 * "The text boxes of a page": one definition shared by every surface that
 * reads detected text, so a pointer tap, a key press on a text-region proxy
 * and the page-text panel all read the SAME box (and so the same pixel
 * rectangle, which shares the server's OCR cache entry for it).
 *
 * The detector returns both block-level and line-level boxes. Reading both
 * would read every balloon twice, and hit-testing all of them would let a
 * tap capture one line of a balloon while the panel reads the whole of it.
 * {@link pageTextBoxes} keeps the enclosing box and drops what it contains;
 * {@link readingOrderRegions} orders that set; {@link hitTestBox} hit-tests
 * it. The marker overlay draws, hit-tests and proxies the ordered set; the
 * panel reads it.
 *
 * Imported only by lazily loaded reader code (marker overlay, page-text
 * panel), so it stays out of the reader's first load.
 */
import type { TextRegionView } from "@/lib/api/types";
import type { Direction } from "@/lib/reader/detect";

/** Fraction of a region's area that must sit inside another region for it
 *  to count as a nested duplicate. */
const NESTED_OVERLAP = 0.8;

function area(r: TextRegionView): number {
  return Math.max(0, r.w) * Math.max(0, r.h);
}

function intersection(a: TextRegionView, b: TextRegionView): number {
  const w = Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x);
  const h = Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y);
  return w > 0 && h > 0 ? w * h : 0;
}

/**
 * The page's text boxes: empty boxes dropped, and a box that sits mostly
 * (80%) inside a larger one dropped in favour of the larger. Two identical
 * boxes keep exactly one. Detector order is kept.
 */
export function pageTextBoxes(
  regions: readonly TextRegionView[],
): TextRegionView[] {
  return regions.filter((r, i) => {
    const a = area(r);
    if (a <= 0) return false;
    return !regions.some((other, j) => {
      if (i === j) return false;
      const oa = area(other);
      // Only a strictly larger (or equal-size, earlier) box can swallow
      // this one — two identical boxes keep exactly one.
      if (oa < a || (oa === a && j > i)) return false;
      return intersection(r, other) / a >= NESTED_OVERLAP;
    });
  });
}

/**
 * {@link pageTextBoxes} sorted into reading order: rows top-to-bottom, and
 * within a row left-to-right (or right-to-left for RTL / manga). Two boxes
 * share a row when their vertical centres are closer than half the shorter
 * box's height — enough slack for balloons that sit slightly staggered in
 * one tier.
 */
export function readingOrderRegions(
  regions: readonly TextRegionView[],
  direction: Direction,
): TextRegionView[] {
  const byTop = pageTextBoxes(regions).sort((a, b) => a.y - b.y || a.x - b.x);
  const rows: TextRegionView[][] = [];
  for (const r of byTop) {
    const row = rows[rows.length - 1];
    const anchor = row?.[0];
    if (
      row &&
      anchor &&
      Math.abs(r.y + r.h / 2 - (anchor.y + anchor.h / 2)) <
        Math.min(r.h, anchor.h) / 2
    ) {
      row.push(r);
    } else {
      rows.push([r]);
    }
  }
  const rtl = direction === "rtl";
  return rows.flatMap((row) =>
    [...row].sort((a, b) => (rtl ? b.x - a.x : a.x - b.x)),
  );
}

/** The smallest of `boxes` containing the point (percent coords), or
 *  `null`. Pass the {@link pageTextBoxes} set: nested line boxes are gone,
 *  so "smallest" only settles two balloons that overlap at the edges. */
export function hitTestBox(
  boxes: readonly TextRegionView[],
  x: number,
  y: number,
): TextRegionView | null {
  let best: TextRegionView | null = null;
  let bestArea = Infinity;
  for (const r of boxes) {
    if (x < r.x || x > r.x + r.w || y < r.y || y > r.y + r.h) continue;
    const a = area(r);
    if (a < bestArea) {
      bestArea = a;
      best = r;
    }
  }
  return best;
}

/** Stable cache-key segment for one box (its percent geometry). */
export function textBoxKey(r: { x: number; y: number; w: number; h: number }) {
  return [r.x, r.y, r.w, r.h].map((v) => v.toFixed(2)).join(",");
}

/** How many box OCR requests the page-text panel keeps in flight at once.
 *  Small on purpose: the recognizer runs on the server's blocking pool and
 *  the `ocr` rate bucket is 60/min/IP. */
export const PAGE_TEXT_CONCURRENCY = 2;

/** How long the client keeps a page's detected boxes and read text after
 *  nothing shows them any more: long enough that flipping back during a
 *  reading session is instant, short enough that a long session does not
 *  hold every page it passed. */
export const PAGE_TEXT_GC_MS = 30 * 60_000;

/** How long a page must stay the current page in webtoon view before the
 *  reader asks the server to detect its text, so scrolling past pages with
 *  the panel (or capture mode) open does not start a detection for every
 *  page passed (the detection bucket allows 15 a minute). */
export const WEBTOON_DETECT_DELAY_MS = 600;

/**
 * Run `fn` over `items` with at most `concurrency` calls in flight,
 * starting them in order. Stops launching new work once `signal` aborts
 * (TanStack cancels a query when its last observer unmounts — e.g. the
 * reader turned the page), but lets in-flight calls finish. A rejected
 * call does not stop the others.
 */
export async function runInOrderPool<T>(
  items: readonly T[],
  concurrency: number,
  fn: (item: T, index: number) => Promise<unknown>,
  signal?: AbortSignal,
): Promise<void> {
  let next = 0;
  const worker = async () => {
    while (next < items.length && !signal?.aborted) {
      const i = next++;
      try {
        await fn(items[i]!, i);
      } catch {
        // The failure lives on that item's own query; keep going.
      }
    }
  };
  await Promise.all(
    Array.from(
      { length: Math.max(1, Math.min(concurrency, items.length)) },
      worker,
    ),
  );
}
