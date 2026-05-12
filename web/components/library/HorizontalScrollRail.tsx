"use client";

import * as React from "react";
import { ArrowRight, ChevronLeft, ChevronRight } from "lucide-react";
import Link from "next/link";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** Width of the left / right edge-fade gradients in pixels. The inline
 *  Tailwind class (`w-9`) MUST stay in sync — JS uses this constant to
 *  offset the initial scroll so the anchor card clears the gradient.
 *  Smaller than the previous 48px so the gradient feels like a hint
 *  rather than a strip of dimmed cover art. */
const LEFT_FADE_PX = 36;

/**
 * Horizontal scrolling rail used by every home/detail surface that
 * surfaces a strip of cards.
 *
 * The native scrollbar is hidden — overflow is still scrollable (mouse
 * wheel, drag, keyboard arrows on focused children, programmatic). Two
 * affordances tell the user "there's more" without the scrollbar UI:
 *
 *   1. Edge fade gradients on the left + right that key off the current
 *      scroll position. They animate in when their direction has
 *      content to reveal and out when the rail is at that boundary.
 *   2. Round chevron buttons that programmatically scroll one rail-
 *      width on click. They share the fade's visibility logic so they
 *      only appear when scrolling that direction is actually possible.
 *
 * For rails that have more items than the preview window can show, set
 * `viewAllHref` so the rail ends with an outline "View all" tile that
 * matches the affordance used by the issue page's "Next in series"
 * carousel. Pass `itemWidthPx` (the per-card pixel width set by the
 * density slider) so the tile aligns visually with its siblings.
 */
export function HorizontalScrollRail({
  children,
  viewAllHref,
  viewAllLabel = "View all",
  itemWidthPx,
  initialScrollIndex,
  className,
}: {
  children: React.ReactNode;
  /** When set, a trailing tile linking to the full view is appended. */
  viewAllHref?: string;
  viewAllLabel?: string;
  /** Per-card pixel width (so the trailing tile vertically aligns with
   *  siblings). Optional; the tile uses an intrinsic width when omitted. */
  itemWidthPx?: number;
  /** When set, on mount the rail is scrolled so the child at this index
   *  sits at the left edge of the visible viewport. Used by the CBL
   *  reading window to anchor on the user's next-to-read entry instead
   *  of starting at the previously-finished context.
   *
   *  Prefer `data-rail-current="true"` on the anchor child when the
   *  index isn't known up front — the rail will auto-find it. */
  initialScrollIndex?: number;
  className?: string;
}) {
  const scrollerRef = React.useRef<HTMLDivElement>(null);
  const [canLeft, setCanLeft] = React.useState(false);
  const [canRight, setCanRight] = React.useState(false);
  // Track whether we've applied the initial scroll yet so reruns of the
  // effect (caused by ResizeObserver flux during layout) don't keep
  // resetting the user's manual scroll position.
  const didInitialScroll = React.useRef(false);

  // Recompute scroll-affordance flags whenever the scroller's content
  // size or scroll position changes. `1px` slop on the right comparison
  // avoids flicker from sub-pixel rounding on zoomed displays.
  const recompute = React.useCallback(() => {
    const el = scrollerRef.current;
    if (!el) return;
    setCanLeft(el.scrollLeft > 4);
    setCanRight(el.scrollLeft + el.clientWidth < el.scrollWidth - 4);
  }, []);

  React.useEffect(() => {
    recompute();
    const el = scrollerRef.current;
    if (!el) return;

    // Apply the initial scroll once on first layout. Anchor selection,
    // in priority order:
    //   1. Explicit `initialScrollIndex` prop (when caller knows the index).
    //   2. A child carrying `data-rail-current="true"` (preferred for
    //      async cases like the CBL window — the body component flags
    //      one card and the rail finds it after layout).
    if (!didInitialScroll.current) {
      const row = el.firstElementChild;
      let anchor: HTMLElement | null = null;
      if (
        row &&
        typeof initialScrollIndex === "number" &&
        initialScrollIndex > 0
      ) {
        const child = row.children.item(initialScrollIndex);
        if (child instanceof HTMLElement) anchor = child;
      }
      if (!anchor && row) {
        anchor = row.querySelector<HTMLElement>('[data-rail-current="true"]');
      }
      if (anchor && row) {
        // Subtract the row's content edge so the card aligns with the
        // viewport's left padding (the inner row uses `px-1`). Then
        // pull back by `LEFT_FADE_PX` so the card sits to the *right*
        // of the left-edge fade gradient — without this offset the
        // anchor lands directly under the fade and reads as darkened
        // on first load.
        const rowRect = row.getBoundingClientRect();
        const cardRect = anchor.getBoundingClientRect();
        const desired = Math.max(
          0,
          cardRect.left - rowRect.left - LEFT_FADE_PX,
        );
        // Skip when the anchor is already at column 0 (or very close)
        // — avoids a no-op scroll write that would still mark the
        // effect as "done."
        if (desired > 1) {
          el.scrollLeft = desired;
          didInitialScroll.current = true;
          recompute();
        }
      }
    }

    const ro = new ResizeObserver(() => recompute());
    ro.observe(el);
    // Observe each child so a card resizing also re-runs the check.
    for (const child of Array.from(el.children)) ro.observe(child);
    el.addEventListener("scroll", recompute, { passive: true });
    return () => {
      ro.disconnect();
      el.removeEventListener("scroll", recompute);
    };
  }, [recompute, children, initialScrollIndex]);

  const scrollBy = (dir: "left" | "right") => {
    const el = scrollerRef.current;
    if (!el) return;
    // Step one card at a time so users can keep their place — the
    // previous 85%-of-viewport jump was disorienting on rails with
    // many small cards. Card width is read from the first child so
    // the step matches whatever density the user has picked; gap-3
    // (12px) is added so consecutive clicks land cleanly on cell
    // boundaries.
    const row = el.firstElementChild;
    const firstCard = row?.children.item(0);
    const cardWidth =
      firstCard instanceof HTMLElement
        ? firstCard.offsetWidth + 12
        : el.clientWidth * 0.5;
    const delta = cardWidth * (dir === "left" ? -1 : 1);
    el.scrollBy({ left: delta, behavior: "smooth" });
  };

  return (
    <div className={cn("group/rail relative", className)}>
      <div
        ref={scrollerRef}
        // overflow-x-auto + scrollbar-hidden utility ([no-scrollbar]
        // matches a Tailwind plugin used elsewhere; falls back to inline
        // styles below if the plugin isn't loaded). The bottom padding
        // is density-driven so compact mode can trim the otherwise-
        // empty strip beneath the cards.
        className="overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
        style={{
          scrollbarWidth: "none",
          paddingBottom: "var(--density-rail-pb)",
        }}
      >
        <div className="flex gap-3 px-1">
          {children}
          {viewAllHref && (
            <div
              className="flex shrink-0 items-center"
              style={itemWidthPx ? { width: `${itemWidthPx}px` } : undefined}
            >
              <Button asChild variant="outline" size="sm" className="shrink-0">
                <Link href={viewAllHref} aria-label={viewAllLabel}>
                  <span>{viewAllLabel}</span>
                  <ArrowRight aria-hidden="true" className="ml-1 h-3.5 w-3.5" />
                </Link>
              </Button>
            </div>
          )}
        </div>
      </div>

      {/* Edge fades — pointer-events disabled so they never intercept
       *  card clicks. Use a horizontal gradient from background to
       *  transparent so they blend regardless of theme. Width MUST
       *  match `LEFT_FADE_PX` above (`w-9` ≈ 36px). */}
      <div
        aria-hidden="true"
        style={{ bottom: "var(--density-rail-pb)" }}
        className={cn(
          "from-background pointer-events-none absolute top-0 left-0 w-9 bg-gradient-to-r to-transparent transition-opacity duration-200",
          canLeft ? "opacity-100" : "opacity-0",
        )}
      />
      <div
        aria-hidden="true"
        style={{ bottom: "var(--density-rail-pb)" }}
        className={cn(
          "from-background pointer-events-none absolute top-0 right-0 w-9 bg-gradient-to-l to-transparent transition-opacity duration-200",
          canRight ? "opacity-100" : "opacity-0",
        )}
      />

      {/* Scroll buttons. Only render once content overflows; hover-only
       *  to keep the rail uncluttered when the user isn't engaging. The
       *  buttons sit on top of the edge fades. */}
      {/* Elongated vertical pills (`h-14 w-7 rounded-md`) — distinct
       *  from the round `h-8 w-8 rounded-full` play overlay so the two
       *  affordances never get confused for each other. The vertical
       *  anchor is `top-[42%]` (not `top-1/2`) so the pill centers on
       *  the cover art rather than the whole rail (which extends
       *  further down through the title + meta rows). */}
      <button
        type="button"
        aria-label="Scroll left"
        tabIndex={canLeft ? 0 : -1}
        onClick={() => scrollBy("left")}
        className={cn(
          // Primary-tinted pill — matches the play overlay's accent
          // treatment + the project's standard `bg-primary` button
          // styling so the scroll affordance reads as a real button
          // against any cover art behind it.
          "bg-primary/90 text-primary-foreground hover:bg-primary absolute top-[42%] left-1 z-10 inline-flex h-14 w-7 -translate-y-1/2 items-center justify-center rounded-md ring-2 shadow-md ring-white/20 backdrop-blur transition-all duration-150 ease-out focus-visible:ring-offset-2 focus-visible:outline-none",
          canLeft
            ? "scale-100 opacity-0 group-hover/rail:opacity-100 focus-visible:opacity-100"
            : "pointer-events-none scale-95 opacity-0",
        )}
      >
        <ChevronLeft aria-hidden="true" className="h-4 w-4" />
      </button>
      <button
        type="button"
        aria-label="Scroll right"
        tabIndex={canRight ? 0 : -1}
        onClick={() => scrollBy("right")}
        className={cn(
          "bg-primary/90 text-primary-foreground hover:bg-primary absolute top-[42%] right-1 z-10 inline-flex h-14 w-7 -translate-y-1/2 items-center justify-center rounded-md ring-2 shadow-md ring-white/20 backdrop-blur transition-all duration-150 ease-out focus-visible:ring-offset-2 focus-visible:outline-none",
          canRight
            ? "scale-100 opacity-0 group-hover/rail:opacity-100 focus-visible:opacity-100"
            : "pointer-events-none scale-95 opacity-0",
        )}
      >
        <ChevronRight aria-hidden="true" className="h-4 w-4" />
      </button>
    </div>
  );
}
