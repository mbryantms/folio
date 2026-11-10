"use client";

import * as React from "react";

/** The slice of a TanStack `useInfiniteQuery` result an infinite-scroll
 *  sentinel reads. Every field comes straight off the query result, so
 *  callers pass the query object itself. */
export type InfiniteSentinelQuery = {
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  /** A next-page request failed. Auto-fetching parks until it clears:
   *  the user's Retry (`fetchNextPage()`) succeeding is what clears it. */
  isFetchNextPageError: boolean;
  fetchNextPage: () => Promise<unknown>;
};

/** IntersectionObserver sentinel for a TanStack `useInfiniteQuery` —
 *  the same shape `IssuesPanel` / `CreatorsIndex` inline: when the
 *  returned ref's element scrolls within 400px of the viewport and
 *  another page exists, fetch it. Depends on the individual fields (not
 *  the whole query object) so the observer isn't torn down every render.
 *
 *  A failed next page stops the auto-fetch: the observer is rebuilt when
 *  `isFetchingNextPage` flips back and would report the still-visible
 *  sentinel as intersecting straight away, so without the gate a failing
 *  server got a request loop. `<LoadMoreError>` offers the Retry. */
export function useInfiniteSentinel(query: InfiniteSentinelQuery) {
  const {
    hasNextPage,
    isFetchingNextPage,
    isFetchNextPageError,
    fetchNextPage,
  } = query;
  const ref = React.useRef<HTMLDivElement | null>(null);
  React.useEffect(() => {
    const el = ref.current;
    if (!el || isFetchNextPageError) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (
          entries.some((e) => e.isIntersecting) &&
          hasNextPage &&
          !isFetchingNextPage
        ) {
          void fetchNextPage();
        }
      },
      { rootMargin: "400px" },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [hasNextPage, isFetchingNextPage, isFetchNextPageError, fetchNextPage]);
  return ref;
}
