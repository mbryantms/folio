"use client";

import { useEffect, useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { toast } from "sonner";

export function QueryProvider({ children }: { children: React.ReactNode }) {
  const [client] = useState(
    () =>
      new QueryClient({
        defaultOptions: {
          queries: {
            staleTime: 30_000,
            refetchOnWindowFocus: false,
          },
        },
      }),
  );

  // Offline/online surface (notifications cleanup, post-ship #4).
  // Surfaces a one-time warning when the browser goes offline and a
  // confirmation when connectivity returns, so failed mutations
  // don't look like server bugs. The same `id:` on both toasts means
  // sonner reuses one element — the offline toast morphs into the
  // "back online" toast rather than stacking.
  useEffect(() => {
    if (typeof window === "undefined") return;

    const onOffline = () => {
      toast.warning("You're offline — changes will queue", {
        id: "network-status",
        duration: Infinity,
      });
    };
    const onOnline = () => {
      toast.success("Back online", {
        id: "network-status",
        duration: 3000,
      });
      // Re-fetch active queries that may have failed while offline.
      client.invalidateQueries();
    };

    // If the page loads while already offline, surface the toast
    // immediately rather than waiting for the next online→offline
    // transition.
    if (!navigator.onLine) onOffline();

    window.addEventListener("offline", onOffline);
    window.addEventListener("online", onOnline);
    return () => {
      window.removeEventListener("offline", onOffline);
      window.removeEventListener("online", onOnline);
    };
  }, [client]);

  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
