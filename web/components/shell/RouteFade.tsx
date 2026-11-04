"use client";

import { usePathname, useSearchParams } from "next/navigation";
import { useEffect, useState, ViewTransition } from "react";

import { routeFadeKey } from "@/lib/ui/route-fade";

/**
 * Page cross-fade (folio-flair, standard): wraps a shell's content column
 * so a navigation to another top-level destination (`routeFadeKey`)
 * cross-fades it (~180ms, `::view-transition-*(.route-fade)` in
 * globals.css). The topbar, sidebar and bottom bar are outside this
 * boundary and carry their own static view-transition names, so they
 * stay put and the sliding nav highlight reads cleanly.
 *
 * Next route navigations are React transitions, so React's
 * `<ViewTransition>` runs them through `document.startViewTransition`.
 * This boundary is deliberately *not* keyed (a key would remount the
 * whole page tree, including `RouteChangeReset`, whose popstate memory
 * keeps back-navigation scroll restoration working). Instead its
 * `update` class is `route-fade` only for the commit that changes the
 * key and drops back to `none` right after, so Suspense reveals and
 * in-page transitions (filters, tabs) don't fade.
 *
 * Coexists with the cover morph: the card's named `<ViewTransition
 * share="morph">` pairs with the destination skeleton's slot as before;
 * a named descendant is captured in its own group, so it flies above
 * this fade instead of fading with it.
 */
export function RouteFade({ children }: { children: React.ReactNode }) {
  const pathname = usePathname() ?? "";
  const library = useSearchParams()?.get("library") ?? null;
  const key = routeFadeKey(pathname, library);
  const [state, setState] = useState({ key, fade: false });
  if (state.key !== key) {
    // Render-phase update: the navigation's own commit carries the class.
    setState({ key, fade: true });
  }
  useEffect(() => {
    if (!state.fade) return;
    // A plain (non-transition) update: no view transition runs for it.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setState((s) => (s.fade ? { ...s, fade: false } : s));
  }, [state.fade]);

  return (
    <ViewTransition
      update={state.fade ? "route-fade" : "none"}
      enter="none"
      exit="none"
      default="none"
    >
      {/* Mirrors the inset's flex column so pages lay out as before. */}
      <div data-route-fade="" className="flex min-w-0 flex-1 flex-col">
        {children}
      </div>
    </ViewTransition>
  );
}
