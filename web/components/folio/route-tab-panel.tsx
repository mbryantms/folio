"use client";

import * as React from "react";

import { cn } from "@/lib/utils";

/**
 * The panel under a link-tab nav (reading log, admin library sections):
 * when the active tab's `index` changes, the new route's content enters
 * from the side its tab sits on, like `folio/tabs`' panels
 * (`tab-panel-fwd` / `tab-panel-back`, 420ms). The first render does not
 * animate, a move within one tab (`index` unchanged) does not either, and
 * `index < 0` (a route no tab owns) never animates. The children are not
 * remounted: the animation restarts on the same wrapper. Reduced motion:
 * the global reset makes it an instant swap.
 */
export function RouteTabPanel({
  index,
  className,
  children,
}: {
  index: number;
  className?: string;
  children: React.ReactNode;
}) {
  const ref = React.useRef<HTMLDivElement | null>(null);
  const prev = React.useRef(index);
  React.useLayoutEffect(() => {
    const el = ref.current;
    const from = prev.current;
    prev.current = index;
    if (!el || from === index || from < 0 || index < 0) return;
    el.dataset.routeTabMotion = index > from ? "fwd" : "back";
    // Restart the entrance: drop the class, force a style flush, re-add.
    el.classList.remove("route-tab-in");
    void el.offsetWidth;
    el.classList.add("route-tab-in");
  }, [index]);
  return (
    <div
      ref={ref}
      className={cn(className)}
      onAnimationEnd={(e) => {
        if (e.target === e.currentTarget)
          e.currentTarget.classList.remove("route-tab-in");
      }}
    >
      {children}
    </div>
  );
}
