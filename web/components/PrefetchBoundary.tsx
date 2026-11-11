"use client";

import {
  HydrationBoundary,
  hydrate,
  useQueryClient,
  type DehydratedState,
} from "@tanstack/react-query";
import * as React from "react";

/**
 * `<HydrationBoundary>` for server-prefetched page data (D18), plus one
 * case the stock boundary leaves to an effect.
 *
 * The stock boundary hydrates queries that are *new* to the cache during
 * render, but defers queries that already *exist* to an effect after
 * commit, so it never updates a component that is already showing them.
 * The app shell creates some of the same cache entries before the page
 * renders, without data: the topbar search calls `useContinueReading` /
 * `useMePages` disabled. Those empty entries count as existing, so the
 * prefetched Continue reading rail rendered its skeleton on the server
 * and on hydration, then requested the data it had been handed.
 *
 * So, once on mount, a dehydrated query whose cache entry exists but has
 * no data and no request in flight is hydrated during render, exactly
 * like a new one. Nothing is showing data for it yet, only a disabled or
 * loading state, so there is no shown value to tear; and an observer that
 * is already subscribed (the shell hydrates and commits before the page's
 * Suspense boundary) hears about it through TanStack's batched,
 * asynchronous notify, never synchronously inside this render. Everything
 * else (entries with data, in-flight fetches) goes through the stock
 * boundary unchanged. The once-only `useState` initializer mirrors
 * `HydrateAuthCache`.
 */
export function PrefetchBoundary({
  state,
  children,
}: {
  state: DehydratedState;
  children: React.ReactNode;
}) {
  const qc = useQueryClient();
  React.useState(() => {
    const cache = qc.getQueryCache();
    const placeholders = state.queries.filter((q) => {
      const existing = cache.get(q.queryHash);
      return (
        existing !== undefined &&
        existing.state.data === undefined &&
        existing.state.fetchStatus === "idle"
      );
    });
    if (placeholders.length > 0) hydrate(qc, { queries: placeholders });
  });
  return <HydrationBoundary state={state}>{children}</HydrationBoundary>;
}
