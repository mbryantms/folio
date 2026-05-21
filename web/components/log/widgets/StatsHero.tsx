"use client";

import { BookOpen, FileText, Flame, Gauge, Timer } from "lucide-react";

import { Skeleton } from "@/components/ui/skeleton";
import { useReadingStats } from "@/lib/api/queries";
import { formatTotalHours } from "@/lib/activity";

import { WidgetCard } from "../WidgetCard";
import type { LogWidgetProps, StatsHeroConfig, StatsHeroMetric } from "./types";

const DEFAULT_METRICS: StatsHeroMetric[] = ["issues", "hours", "streak"];

const METRIC_META: Record<
  StatsHeroMetric,
  { label: string; Icon: typeof BookOpen }
> = {
  issues: { label: "Issues", Icon: BookOpen },
  hours: { label: "Time read", Icon: Timer },
  streak: { label: "Streak", Icon: Flame },
  pages: { label: "Pages", Icon: FileText },
  pace_spp: { label: "Sec/page", Icon: Gauge },
};

/** Three-up summary of the user's reading in the page's selected
 *  range. Pulls from `/me/reading-stats`. Metrics rendered are
 *  configurable (M5 dialog); defaults are issues + hours + streak,
 *  matching the M2 hard-coded version. */
export function StatsHero({ widget, scope }: LogWidgetProps<StatsHeroConfig>) {
  const stats = useReadingStats({ type: "all" }, scope.range);
  const metrics =
    widget.config.metrics && widget.config.metrics.length > 0
      ? widget.config.metrics
      : DEFAULT_METRICS;

  return (
    <WidgetCard widget={widget} title="At a glance">
      {stats.isLoading ? (
        <div
          className="grid gap-3"
          style={{ gridTemplateColumns: `repeat(${metrics.length}, 1fr)` }}
        >
          {metrics.map((m) => (
            <Skeleton key={m} className="h-14" />
          ))}
        </div>
      ) : stats.data ? (
        <div
          className="grid gap-3"
          style={{ gridTemplateColumns: `repeat(${metrics.length}, 1fr)` }}
        >
          {metrics.map((m) => (
            <Tile key={m} metric={m} data={stats.data} />
          ))}
        </div>
      ) : (
        <p className="text-destructive text-sm">Failed to load stats.</p>
      )}
    </WidgetCard>
  );
}

function Tile({
  metric,
  data,
}: {
  metric: StatsHeroMetric;
  data: NonNullable<ReturnType<typeof useReadingStats>["data"]>;
}) {
  const meta = METRIC_META[metric];
  const value = computeMetric(metric, data);
  return (
    <div className="border-border/60 bg-muted/30 flex flex-col gap-1 rounded-md border p-2.5">
      <meta.Icon
        aria-hidden="true"
        className="text-muted-foreground h-3.5 w-3.5"
      />
      <div className="text-lg leading-tight font-semibold tabular-nums">
        {value}
      </div>
      <div className="text-muted-foreground text-[10px] tracking-wider uppercase">
        {meta.label}
      </div>
    </div>
  );
}

function computeMetric(
  m: StatsHeroMetric,
  data: NonNullable<ReturnType<typeof useReadingStats>["data"]>,
): string {
  const totals = data.totals;
  switch (m) {
    case "issues":
      return totals.distinct_issues.toLocaleString();
    case "hours":
      return formatTotalHours(totals.active_ms / 3_600_000);
    case "streak":
      return `${totals.current_streak}d`;
    case "pages":
      return totals.distinct_pages_read.toLocaleString();
    case "pace_spp": {
      // Mean seconds-per-page across the pace series. `pace_series`
      // is per-session; this is a coarse summary that's good enough
      // for a hero tile. The full chart lives in PaceChartWidget.
      const points = data.pace_series ?? [];
      if (points.length === 0) return "—";
      const total = points.reduce((acc, p) => acc + p.sec_per_page, 0);
      return `${(total / points.length).toFixed(1)}s`;
    }
  }
}
