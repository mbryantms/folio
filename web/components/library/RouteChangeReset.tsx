"use client";

import { useEffect, useRef } from "react";
import { usePathname } from "next/navigation";

/**
 * Route-change housekeeping for the library shell, keyed on `pathname`.
 *
 * 1. **Scroll to top on forward navigation.** Tapping a pill (a credit
 *    chip → `/creators/<slug>`, a cast/setting chip → the filtered library
 *    grid) from a scrolled-down page should open the destination at its
 *    header, not inherit the previous page's scroll offset. Next's built-in
 *    scroll-to-top is unreliable here (it races the route's `loading.tsx`
 *    Suspense boundary). Back/forward (`popstate`) is left alone so the
 *    browser's native scroll restoration keeps working.
 *
 * 2. **Clear a stuck `<body>` pointer-events lock.** Radix
 *    Dialog/Sheet/DropdownMenu/Popover set `pointer-events: none` on the
 *    body while open and clear it on close. If the close races a navigation
 *    (the trigger unmounts mid-exit-animation), the lock sticks and kills
 *    *every* click on the page ("no actions taken"). The library layout —
 *    and the `MainShell` inside it — persists across client navigations, so
 *    a mount-only reset never re-fires; clearing the lock on every route
 *    change is what actually unsticks it. (This rescues the common case, a
 *    lock stuck *during* a navigation; a lock stuck with no navigation
 *    after it is a separate, rarer Radix issue not handled here.)
 */
/** Clear a stray `pointer-events: none` left on `<body>` by a Radix
 *  overlay whose close raced a navigation. No-op when nothing is locked. */
function clearBodyLock() {
  if (
    typeof document !== "undefined" &&
    document.body.style.pointerEvents === "none"
  ) {
    document.body.style.pointerEvents = "";
  }
}

export function RouteChangeReset() {
  const pathname = usePathname();
  const isPop = useRef(false);

  useEffect(() => {
    const onPopState = () => {
      isPop.current = true;
      // Clear the body lock directly here too: on back/forward the
      // pathname effect below doesn't reliably re-fire for App Router
      // back-cache restores, so the popstate handler is what unsticks a
      // lock when the user navigates back.
      clearBodyLock();
    };
    window.addEventListener("popstate", onPopState);
    return () => window.removeEventListener("popstate", onPopState);
  }, []);

  useEffect(() => {
    // Forward navigation (push): unstick a stray Radix body lock and start
    // the destination at the top. (Pop is handled in the popstate handler
    // above; its scroll position is left to native restoration.)
    clearBodyLock();

    if (isPop.current) {
      isPop.current = false;
      return;
    }
    window.scrollTo(0, 0);
  }, [pathname]);

  return null;
}
