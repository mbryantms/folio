/**
 * Reader "page text" panel state + pure helpers (WP-4.8, audit AC-3).
 *
 * The panel exposes a page's OCR text to assistive tech. It reuses the
 * existing server OCR surface — `GET /me/issues/{id}/pages/{n}/text-regions`
 * for the detected bubbles, then `POST /me/issues/{id}/ocr` per bubble —
 * and only runs when the panel is opened, so no OCR work happens on page
 * load. The panel component itself is lazy-loaded; this module stays tiny
 * because the reader chrome, the skip links and the keymap all import it.
 */
import { create } from "zustand";
import type { TextRegionView } from "@/lib/api/types";
import type { Direction } from "@/lib/reader/detect";

type PageTextPanelState = {
  open: boolean;
  setOpen: (open: boolean) => void;
  toggle: () => void;
};

/** Open/closed flag for the page-text panel. Kept out of the main reader
 *  store so the panel's wiring doesn't churn `store.ts`. */
export const usePageTextPanel = create<PageTextPanelState>((set) => ({
  open: false,
  setOpen: (open) => set({ open }),
  toggle: () => set((s) => ({ open: !s.open })),
}));

/** Fraction of a region's area that must sit inside another region for it
 *  to count as a nested duplicate. The detector emits both block- and
 *  line-level boxes; OCR-ing both would read every bubble twice. */
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
 * Collapse nested detections (keep the enclosing box) and sort the rest
 * into reading order: rows top-to-bottom, and within a row left-to-right
 * (or right-to-left for RTL / manga). Two regions share a row when their
 * vertical centres are closer than half the shorter region's height —
 * enough slack for bubbles that sit slightly staggered in one tier.
 */
export function readingOrderRegions(
  regions: readonly TextRegionView[],
  direction: Direction,
): TextRegionView[] {
  const kept = regions.filter((r, i) => {
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
  const byTop = [...kept].sort((a, b) => a.y - b.y || a.x - b.x);
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

/** How many bubble OCR requests the panel keeps in flight at once. Small
 *  on purpose: the recognizer runs on the server's blocking pool and the
 *  `ocr` rate bucket is 60/min/IP. */
export const PAGE_TEXT_CONCURRENCY = 2;

/**
 * Run `fn` over `items` with at most `concurrency` calls in flight,
 * starting them in order. Stops launching new work once `signal` aborts
 * (TanStack cancels a query when its last observer unmounts — e.g. the
 * reader turned the page), but lets in-flight calls finish.
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
      await fn(items[i]!, i);
    }
  };
  await Promise.all(
    Array.from(
      { length: Math.max(1, Math.min(concurrency, items.length)) },
      worker,
    ),
  );
}

/** BCP-47 tag for the recognizer the server resolved, so screen readers
 *  pick the right voice for manga text. */
export function ocrLangTag(
  lang: string | null | undefined,
): string | undefined {
  if (lang === "manga") return "ja";
  if (lang === "western") return "en";
  return undefined;
}
