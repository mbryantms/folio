"use client";

import * as React from "react";

import { SameUniverseSection } from "@/components/library/SameUniverseSection";
import { SeriesRelatedSection } from "@/components/library/SeriesRelatedSection";
import { SimilarSeriesRail } from "@/components/library/SimilarSeriesRail";
import { useCoverTarget } from "@/components/library/use-cover-size";
import { useGridColumnWidth } from "@/lib/library/use-grid-column-width";

/**
 * WP-7.7: body of the series page's **Related** tab — relationships
 * (reading order, grouped links, admin suggestions, "Part of event"),
 * the derived "Same universe" section, and the "Similar series" rail.
 *
 * Lazy by construction: the tab panel unmounts while inactive
 * (`StackedTabsPanel`, no `forceMount`), so none of the relationships /
 * same-universe / similar queries fire until the tab opens.
 *
 * Cover sizing: reads the app-wide cover size (`useCoverTarget`) and
 * measures this panel — the same width as the issue grid — to derive the
 * grid's effective column width, so every cover here matches the grid
 * exactly and follows the size control live.
 */
export function SeriesRelatedTab({
  seriesSlug,
  seriesId,
  seriesName,
}: {
  seriesSlug: string;
  seriesId: string;
  seriesName?: string;
}) {
  const target = useCoverTarget();
  const [ref, coverWidth] = useGridColumnWidth<HTMLDivElement>(target);
  return (
    <div
      ref={ref}
      className="space-y-10"
      data-testid="series-related-tab"
      data-cover-width={Math.round(coverWidth * 100) / 100}
    >
      <SeriesRelatedSection
        seriesSlug={seriesSlug}
        seriesId={seriesId}
        seriesName={seriesName}
        coverWidth={coverWidth}
      />
      <SameUniverseSection seriesSlug={seriesSlug} itemWidthPx={coverWidth} />
      <SimilarSeriesRail seriesSlug={seriesSlug} itemWidthPx={coverWidth} />
    </div>
  );
}
