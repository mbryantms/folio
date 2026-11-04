import type { Direction } from "./detect";

/**
 * Pure geometry + key mapping behind the reader's page scrubber
 * (`PageScrubber.tsx`): the bar along the bottom edge of the reader
 * chrome that jumps to a page, with a floating thumbnail preview of the
 * page under the pointer.
 *
 * Pages are 0-based here (the store's `currentPage`); the UI shows them
 * 1-based. In RTL the bar runs right to left — page 1 sits at the right
 * edge, matching the progress fill and the page strip.
 */

function clamp(n: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, n));
}

/** Fraction along the bar (in reading order, 0 = first page) of `page`. */
export function scrubFraction(page: number, totalPages: number): number {
  if (totalPages <= 1) return 0;
  return clamp(page / (totalPages - 1), 0, 1);
}

/** The page under a pointer at `clientX` over a bar spanning `rect`. */
export function scrubPageAt(
  clientX: number,
  rect: { left: number; width: number },
  totalPages: number,
  direction: Direction,
): number {
  if (totalPages <= 1 || rect.width <= 0) return 0;
  let frac = clamp((clientX - rect.left) / rect.width, 0, 1);
  if (direction === "rtl") frac = 1 - frac;
  return Math.round(frac * (totalPages - 1));
}

/** Visual x offset (px from the bar's left edge) of `page`'s position. */
export function scrubPageX(
  page: number,
  totalPages: number,
  width: number,
  direction: Direction,
): number {
  const f = scrubFraction(page, totalPages);
  return (direction === "rtl" ? 1 - f : f) * width;
}

/** Left edge (px) of a preview card of `cardWidth` centred on `x`, kept
 *  `margin` px inside a bar of `width`. */
export function scrubPreviewLeft(
  x: number,
  cardWidth: number,
  width: number,
  margin = 8,
): number {
  const max = Math.max(margin, width - cardWidth - margin);
  return clamp(x - cardWidth / 2, margin, max);
}

/**
 * Target page for a key pressed on the focused scrubber, or `null` when
 * the key isn't the slider's. `visible` is the span on screen (one page,
 * or a double-page spread): forward steps count from its last page and
 * backward steps from its first, so one press always leaves the spread.
 * Arrow keys step one page; in RTL the horizontal arrows follow the
 * bar's visual direction (← moves toward the end), Up / Down never flip.
 * PageUp / PageDown step a tenth of the issue (at least one page); Home /
 * End go to the first / last page.
 */
export function scrubKeyTarget(
  key: string,
  visible: { first: number; last: number },
  totalPages: number,
  direction: Direction,
): number | null {
  if (totalPages <= 0) return null;
  const big = Math.max(1, Math.round(totalPages / 10));
  const rtl = direction === "rtl";
  let delta: number;
  switch (key) {
    case "ArrowRight":
      delta = rtl ? -1 : 1;
      break;
    case "ArrowLeft":
      delta = rtl ? 1 : -1;
      break;
    case "ArrowUp":
      delta = 1;
      break;
    case "ArrowDown":
      delta = -1;
      break;
    case "PageUp":
      delta = big;
      break;
    case "PageDown":
      delta = -big;
      break;
    case "Home":
      return 0;
    case "End":
      return totalPages - 1;
    default:
      return null;
  }
  const from = delta > 0 ? visible.last : visible.first;
  return clamp(from + delta, 0, totalPages - 1);
}
