"use client";

import dynamic from "next/dynamic";

import { Skeleton } from "@/components/ui/skeleton";
import { useReadingStats } from "@/lib/api/queries";

import { WidgetCard } from "../WidgetCard";
import type { LogWidgetProps, PaceChartConfig } from "./types";

// Recharts is heavy and SSR-hostile; same dynamic-import pattern the
// activity dashboard uses so the widget contributes its bundle only
// when actually rendered.
const PaceChart = dynamic(
  () => import("@/components/activity/PaceChart").then((m) => m.PaceChart),
  { ssr: false, loading: () => <Skeleton className="h-48 w-full" /> },
);

/** Reading pace over time — wraps the activity-dashboard PaceChart
 *  scoped to the widget's range. */
export function PaceChartWidget({
  widget,
  scope,
}: LogWidgetProps<PaceChartConfig>) {
  const range = widget.config.range ?? scope.range;
  const stats = useReadingStats({ type: "all" }, range);
  return (
    <WidgetCard widget={widget} title="Reading pace" subtitle={`Last ${range}`}>
      {stats.isLoading ? (
        <Skeleton className="h-48 w-full" />
      ) : stats.data?.pace_series && stats.data.pace_series.length > 0 ? (
        <PaceChart points={stats.data.pace_series} />
      ) : (
        <p className="text-muted-foreground text-xs">
          Not enough completed sessions to plot pace yet.
        </p>
      )}
    </WidgetCard>
  );
}
