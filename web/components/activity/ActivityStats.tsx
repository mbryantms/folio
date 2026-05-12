"use client";

import dynamic from "next/dynamic";

import { Skeleton } from "@/components/ui/skeleton";
import { formatDurationMs, formatTotalHours } from "@/lib/activity";
import { useReadingStats, type ReadingStatsScope } from "@/lib/api/queries";
import type { ReadingStatsRange, ReadingStatsView } from "@/lib/api/types";

import { ActivityRangeSelector } from "./ActivityRangeSelector";

// Dynamic import: recharts is ~80KB gzip and only loaded on pages that
// embed the activity stats. SSR-disabled — the chart needs the DOM to
// measure containers.
const PerDayBarChart = dynamic(
  () => import("./PerDayBarChart").then((m) => m.PerDayBarChart),
  { ssr: false, loading: () => <Skeleton className="h-32 w-full" /> },
);

export function ActivityStats({
  scope,
  range,
  onRangeChange,
  showRangeSelector = true,
}: {
  scope: ReadingStatsScope;
  range: ReadingStatsRange;
  onRangeChange?: (next: ReadingStatsRange) => void;
  /** Series and issue Activity tabs hide the range selector — they
   *  default to '90d' and pin it. The settings page exposes it. */
  showRangeSelector?: boolean;
}) {
  const stats = useReadingStats(scope, range);

  if (stats.isLoading) {
    return <Skeleton className="h-44 w-full" />;
  }
  if (stats.error || !stats.data) {
    return <p className="text-destructive text-sm">Failed to load stats.</p>;
  }
  const data = stats.data;

  return (
    <div className="space-y-4">
      {showRangeSelector && onRangeChange ? (
        <ActivityRangeSelector value={range} onChange={onRangeChange} />
      ) : null}
      <StatsCards data={data} />
      <div>
        <p className="text-muted-foreground mb-2 text-xs font-semibold tracking-wide uppercase">
          Active time per day
        </p>
        {data.per_day.length > 0 ? (
          <PerDayBarChart data={data.per_day} />
        ) : (
          <p className="text-muted-foreground text-sm">
            No reading recorded in this window yet.
          </p>
        )}
      </div>
    </div>
  );
}

function StatsCards({ data }: { data: ReadingStatsView }) {
  const totalHours = data.totals.active_ms / 3_600_000;
  const cards: ReadonlyArray<{ label: string; value: string; sub?: string }> = [
    {
      label: "Time read",
      value: formatTotalHours(totalHours),
      sub: `${data.totals.sessions} session${data.totals.sessions === 1 ? "" : "s"} · ${formatDurationMs(data.totals.active_ms)}`,
    },
    {
      label: "Pages",
      value: data.totals.distinct_pages_read.toLocaleString(),
      sub: `${data.totals.distinct_issues} issue${data.totals.distinct_issues === 1 ? "" : "s"}`,
    },
    {
      label: "Days active",
      value: `${data.totals.days_active}`,
      sub: `${data.per_day.length}-day sample`,
    },
    {
      label: "Streak",
      value: `${data.totals.current_streak}d`,
      sub: `longest ${data.totals.longest_streak}d`,
    },
  ];
  return (
    <ul className="grid grid-cols-2 gap-3 md:grid-cols-4">
      {cards.map((c) => (
        <li
          key={c.label}
          className="border-border bg-background rounded-md border p-3"
        >
          <p className="text-muted-foreground text-xs tracking-wide uppercase">
            {c.label}
          </p>
          <p className="text-foreground mt-1 text-lg font-semibold">
            {c.value}
          </p>
          {c.sub ? (
            <p className="text-muted-foreground mt-0.5 text-xs">{c.sub}</p>
          ) : null}
        </li>
      ))}
    </ul>
  );
}
