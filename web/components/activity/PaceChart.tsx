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

/**
 * Per-session reading pace (seconds per page) plotted over time. We draw
 * both the raw points (low opacity) and a rolling 7-session average so the
 * trend shows even when individual sessions are spiky.
 */
export function PaceChart({ points }: { points: ReadonlyArray<PacePoint> }) {
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
    <ResponsiveContainer width="100%" height={200}>
      <LineChart data={rows} margin={{ top: 6, right: 4, left: 0, bottom: 0 }}>
        <CartesianGrid
          stroke="var(--color-border)"
          strokeDasharray="2 2"
          vertical={false}
        />
        <XAxis
          dataKey="ts"
          type="number"
          domain={["dataMin", "dataMax"]}
          tickFormatter={tickDate}
          tick={{ fill: "var(--color-muted-foreground)", fontSize: 10 }}
          stroke="var(--color-border)"
          minTickGap={32}
        />
        <YAxis
          tick={{ fill: "var(--color-muted-foreground)", fontSize: 10 }}
          tickFormatter={(v: number) => `${Math.round(v)}s`}
          stroke="var(--color-border)"
          width={40}
        />
        <Tooltip
          cursor={{ stroke: "var(--color-muted)", strokeWidth: 1 }}
          contentStyle={{
            background: "var(--color-popover)",
            border: "1px solid var(--color-border)",
            borderRadius: 6,
            fontSize: 12,
          }}
          labelFormatter={(label) =>
            typeof label === "number"
              ? new Date(label).toLocaleString()
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
          stroke="var(--color-primary)"
          strokeOpacity={0.35}
          strokeWidth={1.2}
          dot={false}
          isAnimationActive={false}
        />
        <Line
          type="monotone"
          dataKey="avg"
          stroke="var(--color-primary)"
          strokeWidth={2}
          dot={false}
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
