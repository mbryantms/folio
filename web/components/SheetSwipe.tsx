"use client";

import { useEffect } from "react";

/** Mounts drag-down-to-dismiss for every bottom sheet (lib/ui/sheet-swipe).
 *
 *  The gesture code is fetched after hydration rather than shipped with the
 *  root layout, which every page loads (reader bundle budget,
 *  `docs/dev/pwa-performance.md`). `installSheetSwipe` wires sheets that
 *  are already open when it runs, so one opened before the chunk lands
 *  still gets the gesture. If the chunk never arrives (offline with a cold
 *  cache), sheets keep every other way to close: their close button,
 *  Escape and the overlay. */
export function SheetSwipe() {
  useEffect(() => {
    let live = true;
    let uninstall: (() => void) | undefined;
    void import("@/lib/ui/sheet-swipe")
      .then(({ installSheetSwipe }) => {
        if (live) uninstall = installSheetSwipe();
      })
      .catch(() => {
        /* Chunk unavailable: no drag-to-dismiss until the next load. */
      });
    return () => {
      live = false;
      uninstall?.();
    };
  }, []);
  return null;
}
