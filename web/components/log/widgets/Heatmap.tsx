"use client";

import { ActivityHeatmap } from "@/components/activity/ActivityHeatmap";
import { Skeleton } from "@/components/ui/skeleton";
import { useReadingStats } from "@/lib/api/queries";

import { WidgetCard } from "../WidgetCard";
import type { HeatmapConfig, LogWidgetProps } from "./types";

/** Year heatmap — same component the activity dashboard uses,
 *  pinned to a 1y data fetch regardless of the page range so the
 *  grid is always populated. `config.weeks` is forwarded; the
 *  activity component clamps internally. */
export function Heatmap({ widget }: LogWidgetProps<HeatmapConfig>) {
  const stats = useReadingStats({ type: "all" }, "1y");
  return (
    <WidgetCard
      widget={widget}
      title="Reading heatmap"
      subtitle={
        widget.config.weeks ? `${widget.config.weeks}-week view` : undefined
      }
    >
      {stats.isLoading ? (
        <Skeleton className="h-24 w-full" />
      ) : stats.data ? (
        <ActivityHeatmap perDay={stats.data.per_day} />
      ) : (
        <p className="text-destructive text-sm">Failed to load heatmap.</p>
      )}
    </WidgetCard>
  );
}
