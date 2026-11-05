/**
 * DOM lookup for the page images the paged reader is showing right now
 * (single page, or both panes of a spread), shared by the ambient glow and
 * the page curl. Both views mark their zoom surface with
 * `data-zoom-surface`; the real page `<img>` inside it is the one without
 * `aria-hidden` (the blur-up thumb that shares its grid cell is hidden).
 * The single view's outgoing-page overlay sits outside the zoom surface,
 * so it is never picked up.
 */
export function readerPageImages(): HTMLImageElement[] {
  if (typeof document === "undefined") return [];
  const imgs = Array.from(
    document.querySelectorAll<HTMLImageElement>(
      "[data-zoom-surface] img:not([aria-hidden])",
    ),
  );
  // Visual order (RTL spreads render with `flex-row-reverse`).
  return imgs
    .map((img) => ({ img, left: img.getBoundingClientRect().left }))
    .sort((a, b) => a.left - b.left)
    .map((x) => x.img);
}
