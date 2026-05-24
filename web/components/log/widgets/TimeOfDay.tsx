"use client";

import dynamic from "next/dynamic";

import { Skeleton } from "@/components/ui/skeleton";
import { useReadingStats } from "@/lib/api/queries";

import { WidgetCard } from "../WidgetCard";
import type { LogWidgetProps } from "./types";

// Same dynamic-import pattern PaceChartWidget uses — keeps recharts
// out of the log-page bundle until this widget actually mounts.
const TimeOfDayDonut = dynamic(
  () =>
    import("@/components/activity/TimeOfDayDonut").then((m) => m.TimeOfDayDonut),
  { ssr: false, loading: () => <Skeleton className="h-44 w-full" /> },
);

/** When-of-day donut — morning / afternoon / evening / night
 *  buckets, computed in user-local time on the server. Wraps the
 *  activity-dashboard donut. No config besides the page-level range. */
export function TimeOfDay({
  widget,
  scope,
}: LogWidgetProps<Record<string, never>>) {
  const stats = useReadingStats({ type: "all" }, scope.range);
  return (
    <WidgetCard
      widget={widget}
      title="When you read"
      subtitle={`Last ${scope.range}`}
    >
      {stats.isLoading ? (
        <Skeleton className="h-44 w-full" />
      ) : stats.data?.time_of_day ? (
        <TimeOfDayDonut data={stats.data.time_of_day} />
      ) : (
        <p className="text-destructive text-sm">Failed to load stats.</p>
      )}
    </WidgetCard>
  );
}
