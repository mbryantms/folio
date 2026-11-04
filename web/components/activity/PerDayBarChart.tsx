"use client";

import {
  Bar,
  BarChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

import { formatDurationMs } from "@/lib/activity";
import type { ReadingDayBucket } from "@/lib/api/types";

import {
  AXIS_BARE,
  GRID,
  TOOLTIP_CONTENT_STYLE,
  TOOLTIP_ITEM_STYLE,
  TOOLTIP_LABEL_STYLE,
} from "./chart-style";

/**
 * Per-day bar chart isolated in its own module so the recharts dependency
 * is loaded only when this chart is on screen — wrapped in a `dynamic()`
 * import by the parent (`ActivityStats.tsx`). Bars are the accent; axes
 * and gridlines follow the shared bare chart style (`chart-style.ts`).
 */
export function PerDayBarChart({
  data,
  height = 140,
  animate = true,
  bucket = "day",
}: {
  data: ReadonlyArray<ReadingDayBucket>;
  height?: number;
  /** Grow the bars in on mount; pass `!useReducedMotion()`. */
  animate?: boolean;
  /** `week` when each row is a Monday-start week (long ranges); only
   *  the tooltip label changes. */
  bucket?: "day" | "week";
}) {
  // Recharts wants a plain array of plain objects.
  const rows = data.map((d) => ({
    date: d.date,
    minutes: Math.round(d.active_ms / 60_000),
    active_ms: d.active_ms,
    sessions: d.sessions,
  }));
  return (
    <ResponsiveContainer width="100%" height={height}>
      <BarChart data={rows} margin={{ top: 8, right: 0, left: 0, bottom: 0 }}>
        <CartesianGrid {...GRID} />
        <XAxis
          dataKey="date"
          {...AXIS_BARE}
          tickFormatter={tickDate}
          minTickGap={24}
          tickMargin={6}
        />
        <YAxis
          {...AXIS_BARE}
          tickFormatter={(v: number) => `${v}m`}
          width={36}
          allowDecimals={false}
        />
        <Tooltip
          cursor={{ fill: "var(--muted)", fillOpacity: 0.5 }}
          contentStyle={TOOLTIP_CONTENT_STYLE}
          labelStyle={TOOLTIP_LABEL_STYLE}
          itemStyle={TOOLTIP_ITEM_STYLE}
          labelFormatter={(label) =>
            typeof label === "string"
              ? bucket === "week"
                ? `Week of ${tickDate(label)}`
                : longDate(label)
              : String(label ?? "")
          }
          formatter={(_value, _name, item) => {
            const payload = item?.payload as
              { active_ms?: number; sessions?: number } | undefined;
            const ms = payload?.active_ms ?? 0;
            const sessions = payload?.sessions ?? 0;
            return [
              ms > 0
                ? `${formatDurationMs(ms)} · ${sessions} session${sessions === 1 ? "" : "s"}`
                : "No reading",
              "Read",
            ];
          }}
        />
        <Bar
          dataKey="minutes"
          fill="var(--primary)"
          radius={[3, 3, 0, 0]}
          maxBarSize={28}
          isAnimationActive={animate}
        />
      </BarChart>
    </ResponsiveContainer>
  );
}

function tickDate(iso: string): string {
  const d = new Date(`${iso}T00:00:00`);
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function longDate(iso: string): string {
  const d = new Date(`${iso}T00:00:00`);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
  });
}
