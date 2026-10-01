"use client";

import * as React from "react";

/** IntersectionObserver sentinel for a TanStack `useInfiniteQuery` —
 *  the same shape `IssuesPanel` / `CreatorsIndex` inline: when the
 *  returned ref's element scrolls within 400px of the viewport and
 *  another page exists, fetch it. Depends on the three fields (not the
 *  whole query object) so the observer isn't torn down every render. */
export function useInfiniteSentinel(query: {
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  fetchNextPage: () => Promise<unknown>;
}) {
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = query;
  const ref = React.useRef<HTMLDivElement | null>(null);
  React.useEffect(() => {
    const el = ref.current;
    if (!el) return;
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
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);
  return ref;
}
