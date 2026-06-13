/**
 * Pure layout math for the window-virtualized library grid
 * (`LibraryGridView`, audit G1). The on-screen grid uses a responsive
 * CSS `repeat(auto-fill, minmax(${cardSize}px, 1fr))`; to virtualize by
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
 * How many columns the `auto-fill minmax(cardSize, 1fr)` grid yields at
 * a given container width. Inverts the CSS formula: each column needs at
 * least `cardSize`, columns are separated by `gap`, and there's one
 * fewer gap than columns — so `n` columns fit when
 * `n*cardSize + (n-1)*gap <= width`, i.e.
 * `n <= (width + gap) / (cardSize + gap)`. Always at least 1 (a single
 * card never disappears, even in a sliver of width).
 */
export function computeColumnsPerRow(
  containerWidth: number,
  cardSize: number,
  gap: number = GRID_GAP_PX,
): number {
  if (containerWidth <= 0 || cardSize <= 0) return 1;
  return Math.max(1, Math.floor((containerWidth + gap) / (cardSize + gap)));
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
 * Actual rendered card/column width once the grid stretches its columns
 * to fill (`1fr`): the leftover after gaps, split evenly. Used to derive
 * the cover height for the row-height estimate.
 */
export function computeColumnWidth(
  containerWidth: number,
  columnsPerRow: number,
  gap: number = GRID_GAP_PX,
): number {
  if (containerWidth <= 0 || columnsPerRow <= 0) return 0;
  const gaps = gap * (columnsPerRow - 1);
  return Math.max(0, (containerWidth - gaps) / columnsPerRow);
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
