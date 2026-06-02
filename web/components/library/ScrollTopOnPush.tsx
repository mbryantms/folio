"use client";

import { useEffect, useRef } from "react";
import { usePathname } from "next/navigation";

/**
 * Scrolls the window to the top on **forward** (push) navigations within
 * the library shell — so a page reached by tapping a pill (a credit chip
 * → `/creators/<slug>`, a cast/setting chip → the filtered library grid)
 * always starts at its header instead of inheriting the previous page's
 * scroll offset. The reported symptom was the destination loading scrolled
 * down with its header clipped off the top; Next's built-in scroll-to-top
 * is unreliable here (it races the route's `loading.tsx` Suspense
 * boundary).
 *
 * Back/forward navigations are left alone: a `popstate` sets a one-shot
 * flag that suppresses the next scroll reset, so the browser's native
 * scroll restoration keeps working when the user navigates back.
 */
export function ScrollTopOnPush() {
  const pathname = usePathname();
  const isPop = useRef(false);

  useEffect(() => {
    const onPopState = () => {
      isPop.current = true;
    };
    window.addEventListener("popstate", onPopState);
    return () => window.removeEventListener("popstate", onPopState);
  }, []);

  useEffect(() => {
    if (isPop.current) {
      // Back/forward — let the browser restore the prior scroll position.
      isPop.current = false;
      return;
    }
    window.scrollTo(0, 0);
  }, [pathname]);

  return null;
}
