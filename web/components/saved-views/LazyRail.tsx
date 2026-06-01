"use client";

import * as React from "react";

import { SavedViewRail } from "./SavedViewRail";
import type { SavedViewView } from "@/lib/api/types";

/** Lazy-mount wrapper around `<SavedViewRail>`.
 *
 *  Each rail makes its own data fetch (`useSavedViewResults`,
 *  `useCblListWindowInfinite`, etc.) on mount and renders up to ~30
 *  cover cards. With the per-user `max_rails_per_page` cap raised
 *  from a fixed 12, mounting every rail at home-page hydration starts
 *  to dominate first-paint bytes (each rail = 12+ cover thumbnails)
 *  and concurrent network slots.
 *
 *  The wrapper renders an empty same-height placeholder until the
 *  rail is within ~one viewport of the user's scroll position, then
 *  swaps in the real rail. The IntersectionObserver is one-shot —
 *  once mounted, the rail stays mounted, so scrolling past and back
 *  doesn't refetch.
 *
 *  Placeholder height is estimated from the rail's card-size
 *  preference: roughly `cardSize` (the visible card height) plus a
 *  constant for the rail title row and gap. Off by a bit is fine
 *  because the swap happens while the rail is still off-screen — the
 *  user never sees the layout reflow.
 */
export function LazyRail({
  view,
  cardSize,
  priority = false,
}: {
  view: SavedViewView;
  cardSize: number;
  /** First/above-the-fold rail: mount immediately (skip the
   *  intersection-observer wait) and eager-load its covers, so the LCP
   *  cover paints fast instead of being deferred. */
  priority?: boolean;
}) {
  const ref = React.useRef<HTMLDivElement | null>(null);
  // The priority rail is in view at load — mount now rather than waiting
  // for the observer to fire.
  const [mounted, setMounted] = React.useState(priority);

  React.useEffect(() => {
    if (mounted) return;
    const el = ref.current;
    if (!el) return;
    // Belt-and-suspenders: in any environment that lacks
    // IntersectionObserver (a much-older Safari that somehow gets
    // here, or a stripped-down test harness), fall back to
    // immediate mount so the rails never silently fail to render.
    if (typeof IntersectionObserver === "undefined") {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setMounted(true);
      return;
    }
    const obs = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            setMounted(true);
            obs.disconnect();
            return;
          }
        }
      },
      {
        // Mount one full viewport before the rail enters view so its
        // data fetch + thumbnail loads complete by the time the user
        // actually scrolls to it. Empirically this keeps a scroll
        // through 30+ rails feeling like the data was always there.
        rootMargin: "100% 0px",
      },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [mounted]);

  if (mounted) {
    return <SavedViewRail view={view} cardSize={cardSize} priority={priority} />;
  }

  // Height estimate: one card-height row + ~120px for the title row
  // + density gaps. Doesn't have to be exact because the swap fires
  // while the rail is still off-screen.
  const placeholderHeight = cardSize + 120;
  return (
    <section
      ref={ref}
      aria-busy="true"
      aria-label={view.name}
      data-testid="lazy-rail-placeholder"
      style={{ minHeight: `${placeholderHeight}px` }}
      className="flex flex-col"
    />
  );
}
