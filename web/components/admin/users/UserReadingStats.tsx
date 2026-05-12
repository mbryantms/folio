"use client";

import dynamic from "next/dynamic";
import { useState } from "react";

import { ActivityRangeSelector } from "@/components/activity/ActivityRangeSelector";
import { TopRankingsList } from "@/components/activity/TopRankings";
import { Skeleton } from "@/components/ui/skeleton";
import { formatDurationMs, formatTotalHours } from "@/lib/activity";
import { useAdminUserReadingStats } from "@/lib/api/queries";
import type { ReadingStatsRange, ReadingStatsView } from "@/lib/api/types";

const PerDayBarChart = dynamic(
  () =>
    import("@/components/activity/PerDayBarChart").then(
      (m) => m.PerDayBarChart,
    ),
  { ssr: false, loading: () => <Skeleton className="h-32 w-full" /> },
);

/**
 * Admin-only per-user reading stats — fired from the user-detail Reading
 * tab. Each successful fetch writes an `admin.user.activity.view` audit
 * row server-side, so this component is intentionally not pre-fetched on
 * the parent page; it only mounts when the tab opens.
 */
export function UserReadingStats({ userId }: { userId: string }) {
  const [range, setRange] = useState<ReadingStatsRange>("30d");
  const stats = useAdminUserReadingStats(userId, range);

  if (stats.isLoading) {
    return <Skeleton className="h-72 w-full" />;
  }
  if (stats.error || !stats.data) {
    return <p className="text-destructive text-sm">Failed to load stats.</p>;
  }
  const data = stats.data;

  return (
    <div className="space-y-5">
      <div className="rounded-md border border-amber-500/30 bg-amber-500/5 p-3 text-xs text-amber-200/80">
        Viewing another user&rsquo;s reading activity is audit-logged. Each load
        of this tab writes an{" "}
        <code className="font-mono">admin.user.activity.view</code> entry.
      </div>

      <ActivityRangeSelector value={range} onChange={setRange} />

      <TotalsCards data={data} />

      <div>
        <p className="text-muted-foreground mb-2 text-xs font-semibold tracking-wide uppercase">
          Active time per day
        </p>
        {data.per_day.length > 0 ? (
          <PerDayBarChart data={data.per_day} />
        ) : (
          <p className="text-muted-foreground text-sm">
            No reading recorded in this window.
          </p>
        )}
      </div>

      <div>
        <p className="text-muted-foreground mb-2 text-xs font-semibold tracking-wide uppercase">
          Top across this user&rsquo;s window
        </p>
        <TopRankingsList data={data} />
      </div>
    </div>
  );
}

function TotalsCards({ data }: { data: ReadingStatsView }) {
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
