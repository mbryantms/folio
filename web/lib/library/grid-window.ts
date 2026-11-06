/**
 * Pure layout math for the window-virtualized library grid
 * (`LibraryGridView`, audit G1). The on-screen grid uses a responsive
 * CSS `cardGridColumns` template; to virtualize by
 * row we have to reproduce the column count the browser would pick, then
 * window the resulting rows.
 *
 * Kept side-effect-free so it's unit-testable in the node-env harness
 * (the rendering + measurement live in the component).
 */

/** Gap between cards, in px — must match the grid's Tailwind `gap-4`. */
export const GRID_GAP_PX = 16;

/**
 * Approximate height (px) of a card's text block below the cover, per
 * card type. Used ONLY as the virtualizer's initial row-height estimate
 * — `measureElement` corrects the real height after first paint, so
 * these don't need to be exact, just close enough to avoid a visible
 * first-paint jump. Series = title line + meta line; issue cards run a
 * touch taller (number line + title + finished-state row).
 */
export const SERIES_TEXT_H = 56;
export const ISSUE_TEXT_H = 64;

/**
 * How many columns the `cardGridColumns` grid yields at
 * a given container width. Inverts the CSS formula: each column needs at
 * least `cardSize`, columns are separated by `gap`, and there's one
 * fewer gap than columns — so `n` columns fit when
 * `n*cardSize + (n-1)*gap <= width`, i.e.
 * `n <= (width + gap) / (cardSize + gap)`. Always at least 1 (a single
 * card never disappears, even in a sliver of width).
 */
/** Smallest card the two-column floor will shrink to. Below this a
 *  container is a sliver (a collapsed pane), not a phone. */
const MIN_CARD_PX = 96;

export function computeColumnsPerRow(
  containerWidth: number,
  cardSize: number,
  gap: number = GRID_GAP_PX,
): number {
  if (containerWidth <= 0 || cardSize <= 0) return 1;
  // Two-column floor (phone-layout.md): a card size saved on a wide
  // screen, or the gutter reserved for the phone A–Z rail, must never
  // collapse a phone grid to one column. The effective card width is
  // capped at half the row (never below `MIN_CARD_PX`, so a genuine
  // sliver still gets one card). Mirrors the CSS grids'
  // `min(<cardSize>px, calc(50% - 0.5rem))` tracks; wide
  // containers are unaffected because half their width exceeds any
  // card size.
  const half = Math.max(MIN_CARD_PX, (containerWidth - gap) / 2);
  const effective = Math.min(cardSize, half);
  return Math.max(1, Math.floor((containerWidth + gap) / (effective + gap)));
}

/** Number of virtual rows for `itemCount` items at `columnsPerRow`. */
export function computeRowCount(
  itemCount: number,
  columnsPerRow: number,
): number {
  if (itemCount <= 0 || columnsPerRow <= 0) return 0;
  return Math.ceil(itemCount / columnsPerRow);
}

/**
 * Rendered card/column width. Columns are fixed at the card size (they
 * do NOT stretch to fill the row), so "200px" on the slider is a 200px
 * cover here exactly as it is on the home rails, which size each card
 * with `width: <cardSize>px`. The only exception is the two-column
 * phone floor: once half the row is narrower than the card size, each
 * column is half the row. Mirrors `cardGridColumns`.
 */
export function computeColumnWidth(
  containerWidth: number,
  columnsPerRow: number,
  cardSize: number,
  gap: number = GRID_GAP_PX,
): number {
  if (containerWidth <= 0 || columnsPerRow <= 0) return 0;
  const half = (containerWidth - gap) / 2;
  return Math.max(0, Math.min(cardSize, half > 0 ? half : containerWidth));
}

/**
 * The `[start, end)` slice of the flat `items` array rendered in a given
 * virtual row. The last row may be partial. `end` is clamped to
 * `itemCount` so the final row never over-reads.
 */
export function rowItemRange(
  rowIndex: number,
  columnsPerRow: number,
  itemCount: number,
): { start: number; end: number } {
  const start = rowIndex * columnsPerRow;
  const end = Math.min(start + columnsPerRow, itemCount);
  return { start, end };
}

/**
 * Initial row-height estimate (px) for the virtualizer before
 * `measureElement` corrects it. Cover is `aspect-[2/3]` of the column
 * width (height = width × 1.5); the text block + paddings + row gap add
 * a roughly constant tail. Only an estimate — the real height is
 * measured per row at render time.
 */
export function estimateRowHeight(
  columnWidth: number,
  textBlockHeight: number,
  rowGap: number = GRID_GAP_PX,
): number {
  const cover = columnWidth * 1.5;
  return cover + textBlockHeight + rowGap;
}

/**
 * The real column width of a `cardGridColumns` grid at
 * `containerWidth` (WP-7.7): the card size, or half the row once the
 * two-column phone floor kicks in. Lets surfaces that aren't that grid
 * (rails, strips) render covers at exactly the issue grid's width.
 * Falls back to `minSize` before the first measurement.
 */
export function effectiveColumnWidth(
  containerWidth: number,
  minSize: number,
  gap: number = GRID_GAP_PX,
): number {
  if (containerWidth <= 0) return minSize;
  const cols = computeColumnsPerRow(containerWidth, minSize, gap);
  return computeColumnWidth(containerWidth, cols, minSize, gap);
}
