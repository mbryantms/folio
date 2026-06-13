"use client";

import { useEffect, useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { toast } from "sonner";

import { HttpError } from "@/lib/api/queries";

export function QueryProvider({ children }: { children: React.ReactNode }) {
  const [client] = useState(
    () =>
      new QueryClient({
        defaultOptions: {
          queries: {
            staleTime: 30_000,
            refetchOnWindowFocus: false,
            // TanStack's default (retry: 3) re-ran deterministic 4xx
            // failures — a 403 admin query meant four requests and
            // ~3.5s of exponential backoff before the error surfaced.
            // Retry once, and only for transport errors / 5xx.
            retry: (failureCount, error) => {
              if (failureCount >= 1) return false;
              if (error instanceof HttpError) return error.status >= 500;
              return true;
            },
          },
        },
      }),
  );

  // Offline/online surface. Surfaces a one-time warning when the
  // browser goes offline and a confirmation when connectivity
  // returns, so failed mutations don't look like server bugs. The
  // same `id:` on both toasts means
  // sonner reuses one element — the offline toast morphs into the
  // "back online" toast rather than stacking.
  useEffect(() => {
    if (typeof window === "undefined") return;

    const onOffline = () => {
      toast.warning("You're offline — changes may fail until you reconnect", {
        id: "network-status",
        duration: Infinity,
      });
    };
    const onOnline = () => {
      toast.success("Back online", {
        id: "network-status",
        duration: 3000,
      });
      // Resurrect only the queries that actually failed while
      // offline. A blanket invalidateQueries() refetched every active
      // query on each offline→online flap — a self-inflicted
      // thundering herd on exactly the flaky connections that flap
      // the most. TanStack's default refetchOnReconnect already
      // covers stale-on-reconnect for the rest.
      client.invalidateQueries({
        refetchType: "active",
        predicate: (q) => q.state.status === "error",
      });
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
