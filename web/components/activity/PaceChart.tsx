"use client";

import {
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

import { movingAverage } from "@/lib/activity";
import type { PacePoint } from "@/lib/api/types";

import {
  AXIS_BARE,
  GRID,
  TOOLTIP_CONTENT_STYLE,
  TOOLTIP_ITEM_STYLE,
  TOOLTIP_LABEL_STYLE,
} from "./chart-style";

/**
 * Per-session reading pace (seconds per page) plotted over time. The raw
 * sessions are a faint muted line with dots; the rolling 7-session average
 * is the accent line, so the trend reads first even when individual
 * sessions are spiky.
 */
export function PaceChart({
  points,
  height = 200,
}: {
  points: ReadonlyArray<PacePoint>;
  height?: number;
}) {
  if (points.length === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        Pace appears once you have a few sessions with 3+ pages read.
      </p>
    );
  }
  const raw = points.map((p) => p.sec_per_page);
  const rows = points.map((p, i) => ({
    ts: new Date(p.started_at).getTime(),
    raw: p.sec_per_page,
    avg: movingAverage(raw, i, 7),
  }));

  return (
    <ResponsiveContainer width="100%" height={height}>
      <LineChart data={rows} margin={{ top: 8, right: 4, left: 0, bottom: 0 }}>
        <CartesianGrid {...GRID} />
        <XAxis
          dataKey="ts"
          type="number"
          domain={["dataMin", "dataMax"]}
          scale="time"
          tickFormatter={tickDate}
          {...AXIS_BARE}
          minTickGap={32}
          tickMargin={6}
        />
        <YAxis
          {...AXIS_BARE}
          tickFormatter={(v: number) => `${Math.round(v)}s`}
          width={40}
        />
        <Tooltip
          cursor={{
            stroke: "var(--muted-foreground)",
            strokeOpacity: 0.4,
            strokeDasharray: "3 4",
          }}
          contentStyle={TOOLTIP_CONTENT_STYLE}
          labelStyle={TOOLTIP_LABEL_STYLE}
          itemStyle={TOOLTIP_ITEM_STYLE}
          labelFormatter={(label) =>
            typeof label === "number"
              ? new Date(label).toLocaleString(undefined, {
                  month: "short",
                  day: "numeric",
                  hour: "numeric",
                  minute: "2-digit",
                })
              : String(label ?? "")
          }
          formatter={(value: unknown, name) => [
            `${Math.round(Number(value))}s / page`,
            name === "avg" ? "7-session avg" : "Session",
          ]}
        />
        <Line
          type="monotone"
          dataKey="raw"
          stroke="var(--muted-foreground)"
          strokeOpacity={0.45}
          strokeWidth={1}
          dot={{ r: 2, fill: "var(--muted-foreground)", strokeWidth: 0 }}
          activeDot={{ r: 3 }}
          isAnimationActive={false}
        />
        <Line
          type="monotone"
          dataKey="avg"
          stroke="var(--primary)"
          strokeWidth={2.25}
          dot={false}
          activeDot={{ r: 4, fill: "var(--primary)", strokeWidth: 0 }}
          isAnimationActive={false}
        />
      </LineChart>
    </ResponsiveContainer>
  );
}

function tickDate(ms: number): string {
  return new Date(ms).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}
