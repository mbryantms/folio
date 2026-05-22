"use client";

import { useEffect, useRef } from "react";
import { Serwist } from "@serwist/window";

import { Button } from "@/components/ui/button";
import { toast } from "sonner";

/**
 * Service-worker bootstrap + update notifier.
 *
 * Mounted as a sibling (not a wrapper) inside the root layout so
 * its import chain doesn't get pulled into every route's first-load
 * bundle. Earlier versions wrapped children via
 * `SerwistProvider` from `@serwist/next/react`; that worked but
 * dragged ~100 KB of Serwist's React context layer into the reader
 * route's bundle, which is gated by the §18.1 budget check in
 * `scripts/check-bundle-size.mjs`. This version uses
 * `@serwist/window`'s `Serwist` class directly — same registration
 * + event surface, no React context — and is paired with a
 * `next/dynamic` shim at the import site so the chunk is split out
 * of first-load entirely.
 *
 * Behaviour:
 * - Registers the compiled `/sw.js` on mount (production only;
 *   the `@serwist/next` build is `disable: true` in dev).
 * - When a newer SW is installed but held back from activating
 *   (because the current SW still controls open clients), surfaces
 *   a sonner toast with a "Reload" action.
 * - Clicking Reload posts `SKIP_WAITING` to the waiting worker;
 *   the `controlling` listener reloads the page once the new SW
 *   takes over. `skipWaiting: false` in `app/sw.ts` keeps deploys
 *   from silently swapping the bundle out from under an active
 *   reader.
 */
export function ServiceWorkerUpdater() {
  // React strict mode mounts effects twice in dev; the ref guards
  // against double-binding the listener (and a duplicate
  // registration call against the navigator).
  const boundRef = useRef(false);

  useEffect(() => {
    if (boundRef.current) return;
    if (typeof window === "undefined") return;
    if (!("serviceWorker" in navigator)) return;
    boundRef.current = true;

    const sw = new Serwist("/sw.js");

    const onWaiting = () => {
      toast.message("A new version of Folio is available.", {
        id: "service-worker-update",
        duration: Infinity,
        action: (
          <Button
            size="sm"
            onClick={() => {
              // Tell the waiting SW to take over. The `controlling`
              // listener below reloads when it assumes control.
              sw.messageSkipWaiting();
            }}
          >
            Reload
          </Button>
        ),
      });
    };

    const onControlling = () => {
      window.location.reload();
    };

    sw.addEventListener("waiting", onWaiting);
    sw.addEventListener("controlling", onControlling);
    sw.register();
  }, []);

  return null;
}
