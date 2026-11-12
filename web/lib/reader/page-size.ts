/**
 * The real pixel size of a page, and the conversions between the percent
 * boxes the reader draws and the pixel rectangles the server reads.
 *
 * The server crops the ORIGINAL decoded page (`POST /me/issues/{id}/ocr`,
 * `crates/server/src/api/issue_ocr.rs`), so every pixel rectangle the
 * client sends must be in original page pixels. The `<img>` on screen is
 * not a source for that: with a `srcSet` of width-descriptor variants
 * (`pageBytesSrcSet`) the browser reports `naturalWidth` / `naturalHeight`
 * density-corrected, so they follow the loaded variant and the layout, not
 * the file. A rectangle computed from them is smaller than the one the
 * user marked and shifted toward the top-left.
 *
 * {@link resolvePageSize} is the one definition of "the page's size", in
 * order of trust:
 *
 *  1. `detected` — `page_w` / `page_h` of the text-regions response: the
 *     very decode the server crops. Also what the page-text panel uses, so
 *     a box read by the panel, a tap and a key press map to the same pixel
 *     rectangle and share the server's cache entry.
 *  2. `server` — the page's `image_width` / `image_height` from the issue's
 *     page list (what the reader lays the page out with).
 *  3. `measured` — the `<img>`'s natural size, only when that element
 *     loaded the original bytes (no `srcset`; see
 *     {@link measuredOriginalSize}).
 *
 * When none is known the answer is `null` and the caller says so; it never
 * guesses from a variant's size.
 *
 * Imported only by lazily loaded reader code (the marker overlay, the
 * marker editor, the page-text panel), so none of it is in the reader's
 * first load.
 */

export type PageSize = { width: number; height: number };

/** A rectangle in page pixels. */
export type PixelRect = { x: number; y: number; w: number; h: number };

/** A box in percent of the page (0–100 on each axis). */
export type PercentBox = { x: number; y: number; w: number; h: number };

function valid(size: PageSize | null | undefined): PageSize | null {
  if (!size) return null;
  const { width, height } = size;
  return Number.isFinite(width) &&
    Number.isFinite(height) &&
    width > 0 &&
    height > 0
    ? { width, height }
    : null;
}

/** The page's size from the issue's page list, when the scanner recorded
 *  both dimensions. */
export function serverPageSize(
  info:
    | { image_width?: number | null; image_height?: number | null }
    | null
    | undefined,
): PageSize | null {
  return valid(
    info?.image_width && info?.image_height
      ? { width: info.image_width, height: info.image_height }
      : null,
  );
}

/** The `<img>`'s natural size, but only when it is the original's: an
 *  element with a `srcset` reports a density-corrected size that follows
 *  the chosen variant and the layout, so it is refused. */
export function measuredOriginalSize(
  img: HTMLImageElement | null | undefined,
): PageSize | null {
  if (!img || !img.complete) return null;
  if (img.getAttribute("srcset")) return null;
  return valid({ width: img.naturalWidth, height: img.naturalHeight });
}

export type PageSizeSources = {
  detected?: PageSize | null;
  server?: PageSize | null;
  measured?: PageSize | null;
};

/** The page's real pixel size: the first known of detected, server,
 *  measured (see the module comment), or `null`. */
export function resolvePageSize(sources: PageSizeSources): PageSize | null {
  return (
    valid(sources.detected) ??
    valid(sources.server) ??
    valid(sources.measured) ??
    null
  );
}

/** A percent box as the integer pixel rectangle the server expects,
 *  clamped to the page so a rounding overshoot at the right or bottom edge
 *  cannot produce an `invalid_region`. */
export function regionToPagePixels(box: PercentBox, size: PageSize): PixelRect {
  const W = size.width;
  const H = size.height;
  const x = Math.max(0, Math.min(W - 1, Math.round((box.x / 100) * W)));
  const y = Math.max(0, Math.min(H - 1, Math.round((box.y / 100) * H)));
  const w = Math.max(1, Math.min(W - x, Math.round((box.w / 100) * W)));
  const h = Math.max(1, Math.min(H - y, Math.round((box.h / 100) * H)));
  return { x, y, w, h };
}

/** A pixel rectangle from the server (`refined_bbox`) back to a percent
 *  box, clamped so it always fits on the page (the server rejects a saved
 *  region whose `x + w` or `y + h` passes 100). */
export function pagePixelsToRegion(
  rect: PixelRect,
  size: PageSize,
): PercentBox {
  const clamp = (v: number, lo: number, hi: number) =>
    Math.max(lo, Math.min(hi, v));
  const x = clamp((rect.x / size.width) * 100, 0, 100);
  const y = clamp((rect.y / size.height) * 100, 0, 100);
  return {
    x,
    y,
    w: clamp((rect.w / size.width) * 100, 0, 100 - x),
    h: clamp((rect.h / size.height) * 100, 0, 100 - y),
  };
}
