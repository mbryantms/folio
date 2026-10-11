/** Minimum gap (px) we keep between a floating element and the viewport
 *  edge on every side, on top of any safe-area inset. */
export const EDGE_GUTTER = 8;

/**
 * Collision padding that keeps Radix popper-positioned content (dropdown,
 * popover, select, hover-card, tooltip, …) inside the **safe area**.
 *
 * Radix positions portalled floating content against the raw visual
 * viewport. In a `viewport-fit: cover` standalone PWA (Folio's manifest)
 * that viewport extends *under* the status bar and out to the physical
 * screen edges, so the Radix default (`collisionPadding = 0`) lets a tall
 * menu slide under the status bar — clipped, with no room to scroll into —
 * and lets side-opened content run off the screen edge.
 *
 * Feeding the `--safe-*` insets (defined on `:root` in `globals.css`,
 * resolved to px at runtime) plus a small gutter as `collisionPadding`
 * keeps content within the safe area AND shrinks the
 * `--radix-…-available-height` cap to the visible region, so genuinely tall
 * content scrolls instead of overflowing. On desktop the insets resolve to
 * 0, leaving just the gutter.
 *
 * Returns a plain number during SSR (no DOM); call from the client (popper
 * content only mounts on open) to pick up the real insets.
 */
export function safeAreaCollisionPadding():
  number | Partial<Record<string, number>> {
  if (typeof window === "undefined") return EDGE_GUTTER;
  const cs = getComputedStyle(document.documentElement);
  const inset = (name: string) =>
    Math.max(0, Math.round(parseFloat(cs.getPropertyValue(name)) || 0)) +
    EDGE_GUTTER;
  return {
    top: inset("--safe-top"),
    right: inset("--safe-right"),
    // A fixed bar along the bottom edge (the phone tab bar) covers that
    // part of the viewport as surely as the home indicator does: a menu
    // that opened over it had its last items under the bar, visible but
    // not tappable. The bar's rendered height already includes the safe
    // inset, so it replaces it rather than adding to it.
    bottom: Math.max(inset("--safe-bottom"), bottomBarInset()),
    left: inset("--safe-left"),
  };
}

/** Height (px, plus the gutter) of a visible fixed bottom bar marked
 *  `data-bottom-inset`, else 0. */
function bottomBarInset(): number {
  const bar = document.querySelector<HTMLElement>("[data-bottom-inset]");
  if (!bar) return 0;
  const rect = bar.getBoundingClientRect();
  if (rect.height <= 0 || getComputedStyle(bar).display === "none") return 0;
  // Only the part that actually overlaps the viewport's bottom edge.
  const overlap = Math.max(0, Math.round(window.innerHeight - rect.top));
  return overlap > 0 ? overlap + EDGE_GUTTER : 0;
}
