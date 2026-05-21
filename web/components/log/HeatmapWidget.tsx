"use client";

import { ActivityHeatmap } from "@/components/activity/ActivityHeatmap";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { useReadingStats } from "@/lib/api/queries";

/** Year heatmap — same component the activity dashboard uses,
 *  pinned to a 1y range so the year grid is always populated even
 *  when the page's range selector is shorter. The component renders
 *  a horizontally scrollable 53-week grid that survives narrow rails
 *  on its own. */
export function HeatmapWidget() {
  const stats = useReadingStats({ type: "all" }, "1y");
  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="text-base">Reading heatmap</CardTitle>
      </CardHeader>
      <CardContent>
        {stats.isLoading ? (
          <Skeleton className="h-24 w-full" />
        ) : stats.data ? (
          <ActivityHeatmap perDay={stats.data.per_day} />
        ) : (
          <p className="text-destructive text-sm">Failed to load heatmap.</p>
        )}
      </CardContent>
    </Card>
  );
}
