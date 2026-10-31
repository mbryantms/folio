"use client";

import * as React from "react";

import { useInfiniteSentinel } from "@/lib/ui/use-infinite-sentinel";
import { cn } from "@/lib/utils";

/**
 * Tail of a `useInfiniteQuery` list: an IntersectionObserver sentinel that
 * fetches the next page as it scrolls into view (the `IssuesPanel` shape,
 * via {@link useInfiniteSentinel}) plus a polite "Loading more…" status
 * while a page is in flight. The sentinel hides itself once `next_cursor`
 * runs out, so the list never stops at the first page and never shows a
 * dead "load more" control either.
 */
export function InfiniteSentinel({
  query,
  label = "Loading more…",
  className,
}: {
  query: {
    hasNextPage: boolean;
    isFetchingNextPage: boolean;
    fetchNextPage: () => Promise<unknown>;
  };
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
    </>
  );
}
