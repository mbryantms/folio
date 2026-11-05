/**
 * Sliding nav highlight geometry (folio-flair, standard).
 *
 * The pill is one absolutely positioned element per nav, translated onto
 * the active item. Pure: given the nav's and the item's client rects, the
 * box to draw in the nav's coordinate space (the pill sits at the nav's
 * top-left origin and moves by `transform`), or `null` when the item is
 * not laid out (a collapsed section, a closed sheet).
 */
export interface PillBox {
  x: number;
  y: number;
  width: number;
  height: number;
}

type RectLike = Pick<DOMRect, "left" | "top" | "width" | "height">;

export function measurePill(
  container: RectLike,
  item: RectLike,
  inset: { x?: number; y?: number } = {},
): PillBox | null {
  if (item.width <= 0 || item.height <= 0) return null;
  const ix = inset.x ?? 0;
  const iy = inset.y ?? 0;
  const width = item.width - ix * 2;
  const height = item.height - iy * 2;
  if (width <= 0 || height <= 0) return null;
  return {
    x: Math.round(item.left - container.left + ix),
    y: Math.round(item.top - container.top + iy),
    width: Math.round(width),
    height: Math.round(height),
  };
}
