"use client";

import * as React from "react";

import { HorizontalScrollRail } from "@/components/library/HorizontalScrollRail";
import { SeriesCardSkeleton } from "@/components/library/SeriesCard";
import { SimilarSeriesCard } from "@/components/library/SimilarSeriesCard";
import { useSimilarSeriesInfinite } from "@/lib/api/queries";

const CARD_WIDTH_PX = 160;

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
 */
export function SimilarSeriesRail({ seriesSlug }: { seriesSlug: string }) {
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
  const itemStyle: React.CSSProperties = { width: `${CARD_WIDTH_PX}px` };

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
      <HorizontalScrollRail itemWidthPx={CARD_WIDTH_PX}>
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
