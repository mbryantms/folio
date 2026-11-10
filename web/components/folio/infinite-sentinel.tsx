"use client";

import * as React from "react";

import { RailError } from "@/components/saved-views/RailError";
import {
  type InfiniteSentinelQuery,
  useInfiniteSentinel,
} from "@/lib/ui/use-infinite-sentinel";
import { cn } from "@/lib/utils";

/**
 * Tail of a `useInfiniteQuery` list: an IntersectionObserver sentinel that
 * fetches the next page as it scrolls into view (the `IssuesPanel` shape,
 * via {@link useInfiniteSentinel}) plus a polite "Loading more…" status
 * while a page is in flight. The sentinel hides itself once `next_cursor`
 * runs out, so the list never stops at the first page and never shows a
 * dead "load more" control either. A failed next page keeps the loaded
 * items and swaps in {@link LoadMoreError}; auto-fetching stays parked
 * until its Retry succeeds.
 */
export function InfiniteSentinel({
  query,
  label = "Loading more…",
  className,
}: {
  query: InfiniteSentinelQuery;
  label?: string;
  className?: string;
}) {
  const ref = useInfiniteSentinel(query);
  return (
    <>
      <div
        ref={ref}
        aria-hidden
        data-testid="infinite-sentinel"
        className={cn("h-12", query.hasNextPage ? "" : "hidden", className)}
      />
      {query.isFetchingNextPage ? (
        <p
          role="status"
          className="text-muted-foreground mt-2 text-center text-xs"
        >
          {label}
        </p>
      ) : null}
      <LoadMoreError query={query} />
    </>
  );
}

/**
 * Compact "Couldn't load more." + Retry for a list whose next page failed.
 * Renders nothing otherwise (and while the retry is in flight). Inline
 * observers that do not use {@link useInfiniteSentinel} gate their own
 * auto-fetch on `isFetchNextPageError` and render this next to their
 * sentinel.
 */
export function LoadMoreError({
  query,
  className,
}: {
  query: Pick<
    InfiniteSentinelQuery,
    "isFetchNextPageError" | "isFetchingNextPage" | "fetchNextPage"
  >;
  className?: string;
}) {
  if (!query.isFetchNextPageError || query.isFetchingNextPage) return null;
  return (
    <RailError
      message="Couldn't load more."
      onRetry={() => void query.fetchNextPage()}
      className={cn("mt-2 justify-center", className)}
    />
  );
}
