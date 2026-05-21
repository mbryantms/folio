"use client";

import { ActivityHeatmap } from "@/components/activity/ActivityHeatmap";
import { Skeleton } from "@/components/ui/skeleton";
import { useReadingStats } from "@/lib/api/queries";
import type { ReadingStatsRange } from "@/lib/api/types";

import { WidgetCard } from "../WidgetCard";
import type { HeatmapConfig, LogWidgetProps } from "./types";

/** Pick the smallest stats range that comfortably covers the
 *  requested heatmap window. A 4-week heatmap doesn't need a year
 *  of per-day buckets; pulling 30d / 90d / 1y instead keeps the
 *  query fast at the small windows users actually pick. */
function rangeForWeeks(weeks: number): ReadingStatsRange {
  if (weeks <= 4) return "30d";
  if (weeks <= 8) return "60d";
  if (weeks <= 12) return "90d";
  return "1y";
}

/** Reading heatmap — same component the activity dashboard uses,
 *  with the widget's `config.weeks` driving both the data fetch
 *  range and the column count of the rendered grid. */
export function Heatmap({ widget }: LogWidgetProps<HeatmapConfig>) {
  const weeks = widget.config.weeks ?? 52;
  const stats = useReadingStats({ type: "all" }, rangeForWeeks(weeks));
  return (
    <WidgetCard
      widget={widget}
      title="Reading heatmap"
      subtitle={`${weeks}-week view`}
    >
      {stats.isLoading ? (
        <Skeleton className="h-24 w-full" />
      ) : stats.data ? (
        <ActivityHeatmap perDay={stats.data.per_day} weeks={weeks} />
      ) : (
        <p className="text-destructive text-sm">Failed to load heatmap.</p>
      )}
    </WidgetCard>
  );
}
