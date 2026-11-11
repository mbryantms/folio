/**
 * One cover size for the whole app (bookmarks excepted: they keep their own
 * px slider, `BOOKMARKS_CARD_SIZE_KEY`).
 *
 * The library grid drives it. The reader does not pick a pixel width; they
 * pick **how many covers fit a row of the library**, and every cover grid
 * stretches its columns to fill the row, so a row never ends in a blank
 * strip. What is stored is a *target* width `T` (`COVER_SIZE_KEY`): a grid
 * fits as many columns of at least `T` as its own width allows and shares
 * the leftover between them (`COVER_GRID_COLUMNS`). The slider's stops are
 * the column counts that give a sensible cover width at the library's
 * current width (`coverStops`), and choosing a stop stores the middle of the
 * band of targets that produce it (`coverTargetForColumns`), so a few
 * pixels of width change (a scrollbar, the phone A–Z gutter) do not flip
 * the count.
 *
 * Rails and strips have no row to fill, so they size each card at the
 * library grid's real column width, published as `--cover-w`
 * (`COVER_WIDTH`). Both variables are set on `<html>` before first paint by
 * `COVER_SIZE_BOOTSTRAP` and kept current by `use-cover-size.ts`, so route
 * skeletons (server components) are the right size from the first frame.
 */
import { computeColumnWidth, computeColumnsPerRow } from "./grid-window";

/** The stored target width, px. */
export const COVER_SIZE_KEY = "folio.cover.size";
/** The library grid's last measured column width, px (for the bootstrap). */
export const COVER_WIDTH_KEY = "folio.cover.width";
/** Pre-unification library slider; read once as the starting size. */
export const LEGACY_LIBRARY_SIZE_KEY = "folio.libraryGrid.cardSize";
/** Bookmarks keep their own slider (components/markers/MarkersList.tsx). */
export const BOOKMARKS_CARD_SIZE_KEY = "folio.bookmarks.cardSize";

export const DEFAULT_COVER_SIZE_PX = 160;
/** Bounds of a stored target; anything outside is junk. */
export const COVER_SIZE_MIN_PX = 64;
export const COVER_SIZE_MAX_PX = 480;
/** A slider stop is a column count whose cover width lands in this range. */
export const COVER_STOP_MIN_PX = 100;
export const COVER_STOP_MAX_PX = 300;

export const COVER_MIN_VAR = "--cover-min";
export const COVER_WIDTH_VAR = "--cover-w";

/** A rail card's width: the library grid's column width. */
export const COVER_WIDTH = `var(${COVER_WIDTH_VAR}, var(${COVER_MIN_VAR}, ${DEFAULT_COVER_SIZE_PX}px))`;

/** The template every cover grid shares: as many columns of at least the
 *  target as fit, stretched to fill the row, never fewer than two
 *  (docs/dev/phone-layout.md). `computeColumnsPerRow` /
 *  `computeColumnWidth` mirror it for the virtualized grid. */
export const COVER_GRID_COLUMNS = `repeat(auto-fill, minmax(min(var(${COVER_MIN_VAR}, ${DEFAULT_COVER_SIZE_PX}px), calc(50% - 0.5rem)), 1fr))`;

/**
 * `sizes` for a cover image at card size where `sizes="auto"` cannot apply
 * (eager / priority covers). `sizes` cannot read `--cover-w`, so this is a
 * static stand-in for the default size: two (sometimes three) per row on a
 * phone (< `sm`, about 44vw at the default), and at most ~190px from `sm` up
 * (the default target is 160px; the grid stretches columns up to the next
 * break). Against the 300w / 600w candidates (`coverThumbSrcSet`) that means
 * a default card loads the 300 variant at 1x and the 600 one only at a DPR
 * high enough to need it. A reader on a larger size (up to the 300px stop)
 * still gets 300 at 1x — exactly enough — but between 1x and ~1.6x a card
 * wider than 190px is told 300 is enough when it would show 600 sharper.
 */
export const COVER_CARD_SIZES = "(max-width: 639px) 45vw, 190px";

export const COVER_GRID_STYLE = {
  gridTemplateColumns: COVER_GRID_COLUMNS,
} as const;
export const COVER_ITEM_STYLE = { width: COVER_WIDTH } as const;

export function parseCoverSize(raw: string | null): number | null {
  if (raw === null || raw === "") return null;
  const n = Number(raw);
  if (!Number.isFinite(n) || n < COVER_SIZE_MIN_PX || n > COVER_SIZE_MAX_PX)
    return null;
  return n;
}

/** Real cover width of a fill-the-row grid `containerWidth` wide. Falls
 *  back to the target before the first measurement. */
export function coverColumnWidth(
  containerWidth: number,
  target: number,
): number {
  if (containerWidth <= 0) return target;
  const cols = computeColumnsPerRow(containerWidth, target);
  // Floored to 1/100 px so N columns can never add up past the row.
  return Math.floor(computeColumnWidth(containerWidth, cols) * 100) / 100;
}

/**
 * The slider's stops: the column counts (ascending) whose cover width at
 * `containerWidth` is within the stop range. One column is offered only
 * where the two-column floor does not apply (a sliver). `current` is always
 * included so the thumb has a position even for a size saved elsewhere.
 */
export function coverStops(containerWidth: number, current?: number): number[] {
  const out: number[] = [];
  if (containerWidth > 0) {
    for (let n = 1; n <= 64; n++) {
      const w = computeColumnWidth(containerWidth, n);
      if (w < COVER_STOP_MIN_PX) break;
      if (w > COVER_STOP_MAX_PX) continue;
      if (n === 1 && computeColumnsPerRow(containerWidth, w) !== 1) continue;
      out.push(n);
    }
  }
  if (current && current > 0 && !out.includes(current)) {
    out.push(current);
    out.sort((a, b) => a - b);
  }
  return out;
}

/** The target to store so a grid `containerWidth` wide shows `columns`
 *  per row: halfway between that count's cover width and the next one's. */
export function coverTargetForColumns(
  containerWidth: number,
  columns: number,
): number {
  if (containerWidth <= 0 || columns <= 0) return DEFAULT_COVER_SIZE_PX;
  const here = computeColumnWidth(containerWidth, columns);
  const next = computeColumnWidth(containerWidth, columns + 1);
  const mid = Math.round((here + next) / 2);
  return Math.min(COVER_SIZE_MAX_PX, Math.max(COVER_SIZE_MIN_PX, mid));
}

/**
 * Mirror the stored cover size onto `<html>` as `--cover-min` / `--cover-w`.
 * Serialized into the pre-paint inline script, so it must stay
 * self-contained: no imports, no closures over module scope, no syntax a
 * compiler would rewrite into a helper call.
 */
export function syncCoverSizeVars(): void {
  try {
    const ls = window.localStorage;
    const style = document.documentElement.style;
    const read = function (key: string, lo: number, hi: number) {
      const raw = ls.getItem(key);
      const n = Number(raw);
      return raw === null || raw === "" || !isFinite(n) || n < lo || n > hi
        ? null
        : n;
    };
    let size = read("folio.cover.size", 64, 480);
    if (size === null) size = read("folio.libraryGrid.cardSize", 64, 480);
    if (size === null) style.removeProperty("--cover-min");
    else style.setProperty("--cover-min", size + "px");
    const width = read("folio.cover.width", 48, 960);
    if (width === null) style.removeProperty("--cover-w");
    else style.setProperty("--cover-w", width + "px");
  } catch {
    // Storage blocked (private mode): covers use the default size.
  }
}

/** Inline-script body for the root layout's `<head>`. */
export const COVER_SIZE_BOOTSTRAP = `(${syncCoverSizeVars.toString()})()`;
