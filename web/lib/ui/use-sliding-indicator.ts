"use client";

import * as React from "react";

/** Where the sliding indicator sits, in the container's offset space. */
export type SlideRect = {
  left: number;
  top: number;
  width: number;
  height: number;
  /** True only when the selection moved to another item: that move
   *  slides; the first placement and resizes jump. */
  animate: boolean;
};

export type SlidingIndicatorOptions = {
  /** Selector (relative to the container, `:scope` allowed) for the
   *  selected item. */
  active: string;
  /** Selector for every item, so their size changes re-measure. */
  items: string;
  /** Attributes whose change can move the selection (watched on the
   *  container's subtree). */
  attributes: string[];
  enabled?: boolean;
};

/**
 * One element that glides to the selected item of a strip (folio-flair):
 * the tab underline (`folio/tabs`), the link-nav underline (reading log,
 * admin library sections) and the segmented-control thumb. Measures the
 * selected item with `offsetLeft/Top` — so the indicator must render
 * inside the same positioned container the items are offset against —
 * and follows selection changes (a MutationObserver on `attributes`) and
 * size changes (a ResizeObserver). Reduced motion: callers animate with a
 * CSS transition, which the global reset zeroes.
 */
export function useSlidingIndicator(
  container: React.RefObject<HTMLElement | null>,
  { active, items, attributes, enabled = true }: SlidingIndicatorOptions,
): SlideRect | null {
  const [rect, setRect] = React.useState<SlideRect | null>(null);
  const attrKey = attributes.join(" ");
  React.useEffect(() => {
    const el = container.current;
    if (!enabled || !el) return;
    let current: HTMLElement | null = null;
    const place = (animate: boolean) => {
      const item = el.querySelector<HTMLElement>(active);
      if (!item) {
        current = null;
        setRect(null);
        return;
      }
      const changed = current !== null && item !== current;
      current = item;
      const next: SlideRect = {
        left: item.offsetLeft,
        top: item.offsetTop,
        width: item.offsetWidth,
        height: item.offsetHeight,
        animate: animate && changed,
      };
      setRect((prev) =>
        prev &&
        prev.left === next.left &&
        prev.top === next.top &&
        prev.width === next.width &&
        prev.height === next.height &&
        prev.animate === next.animate
          ? prev
          : next,
      );
    };
    place(false);
    const mo = new MutationObserver(() => place(true));
    mo.observe(el, {
      subtree: true,
      attributes: true,
      attributeFilter: attrKey.split(" "),
      childList: true,
    });
    const ro =
      typeof ResizeObserver !== "undefined"
        ? new ResizeObserver(() => place(false))
        : null;
    ro?.observe(el);
    for (const t of el.querySelectorAll(items)) ro?.observe(t);
    return () => {
      mo.disconnect();
      ro?.disconnect();
    };
  }, [container, active, items, attrKey, enabled]);
  return enabled ? rect : null;
}

/** The shared easing/duration of every sliding indicator (matches the
 *  tab panel entrance). */
export const SLIDE_TRANSITION =
  "transition-[translate,width,height] duration-[420ms] ease-[cubic-bezier(0.22,1,0.36,1)]";
