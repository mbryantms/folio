"use client";

import * as React from "react";

import { HorizontalScrollRail } from "@/components/library/HorizontalScrollRail";
import {
  SeriesCard,
  SeriesCardSkeleton,
} from "@/components/library/SeriesCard";
import { useSameUniverseInfinite } from "@/lib/api/queries";
import type { SharedUniverse } from "@/lib/api/types";

/** "Universe: Mignolaverse · Group: B.P.R.D." */
export function sharedCaption(shared: SharedUniverse[]): string {
  return shared
    .map((s) => `${s.via === "universe" ? "Universe" : "Group"}: ${s.name}`)
    .join(" · ");
}

/**
 * WP-7.7 derived **Same universe** section in the series page's Related
 * tab: series sharing a `universe` (`series_universes`) or a ComicInfo
 * `SeriesGroup` with this one (`GET /series/{slug}/same-universe`), each
 * captioned with what they share. Not curated and not suggested — it
 * replaces the pairwise `same_universe` suggestions (manual
 * `same_universe` edges still show in the grouped relationships).
 *
 * Cover-size aware (`itemWidthPx` = the issue grid's column width) and
 * cursor-paginated: a sentinel at the end of the rail fetches the next
 * page as it scrolls into view. Hidden when nothing is shared.
 */
export function SameUniverseSection({
  seriesSlug,
  itemWidthPx,
}: {
  seriesSlug: string;
  itemWidthPx: number;
}) {
  const query = useSameUniverseInfinite(seriesSlug);
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
  const total = query.data?.pages[0]?.total ?? null;
  if (!query.isLoading && items.length === 0) return null;
  const itemStyle: React.CSSProperties = { width: `${itemWidthPx}px` };

  return (
    <section
      aria-labelledby="same-universe-heading"
      className="flex flex-col gap-3"
    >
      <div className="flex items-baseline gap-2">
        <h2
          id="same-universe-heading"
          className="text-lg font-semibold tracking-tight"
        >
          Same universe
        </h2>
        {total != null && total > 0 ? (
          <span className="text-muted-foreground text-sm tabular-nums">
            {total}
          </span>
        ) : null}
      </div>
      <HorizontalScrollRail itemWidthPx={itemWidthPx}>
        {query.isLoading
          ? Array.from({ length: 4 }).map((_, i) => (
              <div key={i} style={itemStyle} className="shrink-0">
                <SeriesCardSkeleton />
              </div>
            ))
          : items.map((item) => {
              const caption = sharedCaption(item.shared);
              return (
                <div
                  key={item.series.id}
                  style={itemStyle}
                  className="flex shrink-0 flex-col"
                  data-testid="same-universe-card"
                >
                  <SeriesCard series={item.series} size="md" />
                  {caption ? (
                    <p
                      className="text-muted-foreground line-clamp-2 px-2 text-xs"
                      title={caption}
                    >
                      {caption}
                    </p>
                  ) : null}
                </div>
              );
            })}
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
