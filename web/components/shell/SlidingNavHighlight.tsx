"use client";

import { usePathname, useSearchParams } from "next/navigation";
import { useEffect, useLayoutEffect, useRef } from "react";

import { measurePill } from "@/lib/ui/nav-pill";
import { cn } from "@/lib/utils";

/** The attribute the container carries once the pill owns the active
 *  look; the nav's CSS drops the item's own active background then. */
export const NAV_PILL_ATTR = "data-nav-pill";

/**
 * Sliding active-item highlight (folio-flair, standard). Render it as the
 * first child of a `relative isolate` nav container: it finds the item
 * marked `aria-current="page"` and glides one pill onto it whenever the
 * route (and so the marked item) changes, instead of the highlight
 * jumping. Pure decoration (`aria-hidden`); `aria-current` and the item's
 * text styling stay on the links.
 *
 * First placement, a re-appearance and resizes (rail collapse, viewport)
 * snap without animating. Reduced motion is the global CSS reset: the
 * transition collapses to an instant move. Until the pill is placed the
 * container lacks `data-nav-pill`, so the items' own active background
 * still shows (SSR, no JS).
 */
export function SlidingNavHighlight({
  containerRef,
  inset,
  className,
}: {
  containerRef: React.RefObject<HTMLElement | null>;
  /** Shrinks the pill inside the item's box (px per side). */
  inset?: { x?: number; y?: number };
  className?: string;
}) {
  const pillRef = useRef<HTMLDivElement | null>(null);
  const placed = useRef(false);
  const insetX = inset?.x ?? 0;
  const insetY = inset?.y ?? 0;
  const placeRef = useRef<(animate: boolean) => void>(() => {});
  // The route is what moves `aria-current`. Subscribing here (rather than
  // relying on the owning nav re-rendering this child, which the React
  // Compiler memoizes away) re-places the pill in the navigation's commit.
  const pathname = usePathname();
  const search = useSearchParams()?.toString() ?? "";

  useLayoutEffect(() => {
    placeRef.current = (animate: boolean) => {
      const container = containerRef.current;
      const pill = pillRef.current;
      if (!container || !pill) return;
      const item = container.querySelector<HTMLElement>(
        '[aria-current="page"]',
      );
      const box = item
        ? measurePill(
            container.getBoundingClientRect(),
            item.getBoundingClientRect(),
            { x: insetX, y: insetY },
          )
        : null;
      if (!box) {
        pill.style.opacity = "0";
        placed.current = false;
        container.removeAttribute(NAV_PILL_ATTR);
        return;
      }
      const snap = !animate || !placed.current;
      if (snap) pill.style.transition = "none";
      pill.style.transform = `translate(${box.x}px, ${box.y}px)`;
      pill.style.width = `${box.width}px`;
      pill.style.height = `${box.height}px`;
      pill.style.opacity = "1";
      if (snap) {
        // Flush the snapped position before re-enabling the transition.
        void pill.offsetWidth;
        pill.style.transition = "";
      }
      placed.current = true;
      container.setAttribute(NAV_PILL_ATTR, "ready");
    };
    placeRef.current(true);
  }, [pathname, search, containerRef, insetX, insetY]);

  // Layout changes that don't re-render the nav (rail collapse animating
  // its width, viewport resize, a sibling section expanding) snap it.
  useEffect(() => {
    const container = containerRef.current;
    if (!container || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => placeRef.current(false));
    ro.observe(container);
    return () => {
      ro.disconnect();
      container.removeAttribute(NAV_PILL_ATTR);
    };
  }, [containerRef]);

  return (
    <div
      ref={pillRef}
      aria-hidden="true"
      data-slot="nav-pill"
      style={{ opacity: 0 }}
      className={cn(
        "pointer-events-none absolute top-0 left-0 -z-10 transition-[transform,width,height,opacity] duration-200 ease-[cubic-bezier(0.2,0.8,0.2,1)]",
        className,
      )}
    />
  );
}
