"use client";

import { useEffect, useRef } from "react";

import { SerwistProvider, useSerwist } from "@serwist/next/react";

import { Button } from "@/components/ui/button";
import { toast } from "sonner";

/**
 * Service-worker bootstrap and update notifier.
 *
 * Mounted from the root layout. Wraps its children in
 * `SerwistProvider`, which is responsible for registering the
 * compiled `/sw.js` on first page load in production. Inside the
 * provider, `<UpdateToast>` listens for the `waiting` event the
 * Serwist client emits when a newer service worker has installed
 * but is being held back from activating (because the current SW
 * still controls open clients), and surfaces it as a sonner toast
 * with a "Reload" action.
 *
 * The toast is the user's chance to apply the new bundle on their
 * own terms. Skipping `skipWaiting` in the SW means a deploy
 * mid-read never silently swaps the bundle out from under the
 * reader; the user has to click Reload (or close the tab and come
 * back) for the new SW to take over.
 */
export function ServiceWorkerUpdater({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <SerwistProvider
      swUrl="/sw.js"
      // `register: true` and `disable: false` are the defaults but
      // pinned here so a future Serwist default change doesn't
      // quietly disable registration.
      register
      disable={false}
    >
      <UpdateToast />
      {children}
    </SerwistProvider>
  );
}

const TOAST_ID = "service-worker-update";

function UpdateToast() {
  const { serwist } = useSerwist();
  // React strict mode mounts effects twice in dev; the ref guards
  // against double-binding the listener in that case.
  const boundRef = useRef(false);

  useEffect(() => {
    if (!serwist) return;
    if (boundRef.current) return;
    boundRef.current = true;

    const onWaiting = () => {
      toast.message("A new version of Folio is available.", {
        id: TOAST_ID,
        duration: Infinity,
        action: (
          <Button
            size="sm"
            onClick={() => {
              // Tell the waiting SW to take over, then reload once
              // it does. `messageSkipWaiting` posts `SKIP_WAITING`
              // to the waiting worker; the `controlling` listener
              // below reloads when the new SW assumes control.
              serwist.messageSkipWaiting();
            }}
          >
            Reload
          </Button>
        ),
      });
    };

    const onControlling = () => {
      // The new SW has taken control. Reload to pick up the new
      // bundle. The toast is dismissed by the reload itself.
      window.location.reload();
    };

    serwist.addEventListener("waiting", onWaiting);
    serwist.addEventListener("controlling", onControlling);
  }, [serwist]);

  return null;
}
