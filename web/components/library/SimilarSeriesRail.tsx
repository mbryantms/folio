"use client";

import * as React from "react";

import { HorizontalScrollRail } from "@/components/library/HorizontalScrollRail";
import { SeriesCardSkeleton } from "@/components/library/SeriesCard";
import { SimilarSeriesCard } from "@/components/library/SimilarSeriesCard";
import { useSimilarSeriesInfinite } from "@/lib/api/queries";

/** Card width when the caller doesn't size the rail (the series card
 *  grid's default `minmax`). */
const DEFAULT_CARD_WIDTH_PX = 160;

/**
 * WP-7.4 "Similar series" rail on the series page. Content-based
 * neighbours (shared creators / characters / teams / arcs / genres /
 * tags / publisher), each card captioned with why it matched. Hides
 * itself when there are no visible neighbours.
 *
 * Paging: the endpoint is cursor-paginated; a sentinel at the end of
 * the horizontal scroller fetches the next page when it scrolls into
 * view (IntersectionObserver honours the scroller's clipping), so the
 * rail never silently truncates.
 *
 * WP-7.7: lives in the series page's Related tab (mounted only when the
 * tab opens, so the query is lazy) and takes `itemWidthPx` — the issue
 * grid's effective column width — so its covers match the page's
 * card-size slider instead of a fixed 160 px.
 */
export function SimilarSeriesRail({
  seriesSlug,
  itemWidthPx = DEFAULT_CARD_WIDTH_PX,
}: {
  seriesSlug: string;
  itemWidthPx?: number;
}) {
  const query = useSimilarSeriesInfinite(seriesSlug);
  const sentinelRef = React.useRef<HTMLDivElement | null>(null);
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = query;

  React.useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          if (hasNextPage && !isFetchingNextPage) void fetchNextPage();
        }
      },
      { rootMargin: "0px 400px 0px 0px" },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  const items = query.data?.pages.flatMap((p) => p.items) ?? [];
  if (!query.isLoading && items.length === 0) return null;
  const itemStyle: React.CSSProperties = { width: `${itemWidthPx}px` };

  return (
    <section
      aria-labelledby="similar-series-heading"
      className="flex flex-col gap-3"
    >
      <h2
        id="similar-series-heading"
        className="text-lg font-semibold tracking-tight"
      >
        Similar series
      </h2>
      <HorizontalScrollRail itemWidthPx={itemWidthPx}>
        {query.isLoading
          ? Array.from({ length: 6 }).map((_, i) => (
              <div key={i} style={itemStyle} className="shrink-0">
                <SeriesCardSkeleton />
              </div>
            ))
          : items.map((item) => (
              <div key={item.series.id} style={itemStyle} className="shrink-0">
                <SimilarSeriesCard item={item} />
              </div>
            ))}
        {isFetchingNextPage ? (
          <div style={itemStyle} className="shrink-0">
            <SeriesCardSkeleton />
          </div>
        ) : null}
        <div ref={sentinelRef} aria-hidden className="h-px w-px shrink-0" />
      </HorizontalScrollRail>
    </section>
  );
}
