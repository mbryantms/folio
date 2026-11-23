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
 *  server got a request loop. `<LoadMoreError>` offers the Retry.
 *
 *  Returns a callback ref, not a `useRef`: the element lives in state, so
 *  the observer attaches whenever the sentinel mounts. With a plain ref
 *  the effect only re-ran on query-state changes — a page that rendered
 *  the sentinel later than the query settled (an early-return skeleton
 *  while a sibling query loads, as on the CBL view) never observed it and
 *  stopped at the first page. */
export function useInfiniteSentinel(
  query: InfiniteSentinelQuery,
  {
    rootMargin = "400px",
  }: {
    /** How far outside the viewport the sentinel triggers a fetch.
     *  Horizontal rails pass a right-edge margin (`"0px 400px 0px 0px"`). */
    rootMargin?: string;
  } = {},
) {
  const {
    hasNextPage,
    isFetchingNextPage,
    isFetchNextPageError,
    fetchNextPage,
  } = query;
  const [el, setEl] = React.useState<HTMLDivElement | null>(null);
  React.useEffect(() => {
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
      { rootMargin },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [
    el,
    rootMargin,
    hasNextPage,
    isFetchingNextPage,
    isFetchNextPageError,
    fetchNextPage,
  ]);
  return setEl;
}
