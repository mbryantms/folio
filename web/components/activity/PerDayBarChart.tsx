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

/**
 * Per-day bar chart isolated in its own module so the recharts dependency
 * is loaded only when this chart is on screen — wrapped in a `dynamic()`
 * import by the parent (`ActivityStats.tsx`).
 */
export function PerDayBarChart({
  data,
}: {
  data: ReadonlyArray<ReadingDayBucket>;
}) {
  // Recharts wants a plain array of plain objects.
  const rows = data.map((d) => ({
    date: d.date,
    minutes: Math.round(d.active_ms / 60_000),
    active_ms: d.active_ms,
    sessions: d.sessions,
  }));
  return (
    <ResponsiveContainer width="100%" height={140}>
      <BarChart data={rows} margin={{ top: 4, right: 4, left: 0, bottom: 0 }}>
        <CartesianGrid
          stroke="var(--color-border)"
          strokeDasharray="2 2"
          vertical={false}
        />
        <XAxis
          dataKey="date"
          tick={{ fill: "var(--color-muted-foreground)", fontSize: 10 }}
          tickFormatter={tickDate}
          minTickGap={20}
          stroke="var(--color-border)"
        />
        <YAxis
          tick={{ fill: "var(--color-muted-foreground)", fontSize: 10 }}
          tickFormatter={(v: number) => `${v}m`}
          stroke="var(--color-border)"
          width={32}
        />
        <Tooltip
          cursor={{ fill: "var(--color-muted)" }}
          contentStyle={{
            background: "var(--color-popover)",
            border: "1px solid var(--color-border)",
            borderRadius: 6,
            fontSize: 12,
          }}
          labelStyle={{ color: "var(--color-foreground)" }}
          formatter={(_value, _name, item) => {
            const payload = item?.payload as
              | { active_ms?: number; sessions?: number }
              | undefined;
            const ms = payload?.active_ms ?? 0;
            const sessions = payload?.sessions ?? 0;
            return [
              `${formatDurationMs(ms)} · ${sessions} session${sessions === 1 ? "" : "s"}`,
              "Read",
            ];
          }}
        />
        <Bar
          dataKey="minutes"
          fill="var(--color-primary)"
          radius={[3, 3, 0, 0]}
        />
      </BarChart>
    </ResponsiveContainer>
  );
}

function tickDate(iso: string): string {
  const d = new Date(`${iso}T00:00:00`);
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}
