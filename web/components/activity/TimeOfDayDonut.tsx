"use client";

import { Cell, Pie, PieChart, ResponsiveContainer, Tooltip } from "recharts";

import { formatDurationMs } from "@/lib/activity";
import type { TimeOfDayBuckets } from "@/lib/api/types";

import {
  TOOLTIP_CONTENT_STYLE,
  TOOLTIP_ITEM_STYLE,
  TOOLTIP_LABEL_STYLE,
} from "./chart-style";

const BUCKETS = ["morning", "afternoon", "evening", "night"] as const;

const BUCKET_LABELS = {
  morning: "Morning",
  afternoon: "Afternoon",
  evening: "Evening",
  night: "Night",
} as const;

const BUCKET_HOURS = {
  morning: "5–12",
  afternoon: "12–17",
  evening: "17–22",
  night: "22–5",
} as const;

/** Accent share per rank: the busiest slice is the full accent, the rest
 *  step down toward `--muted` so the eye lands on the answer first. */
const RANK_MIX = [100, 62, 38, 20] as const;

function rankColor(rank: number): string {
  const pct = RANK_MIX[rank] ?? 20;
  return pct === 100
    ? "var(--primary)"
    : `color-mix(in oklab, var(--primary) ${pct}%, var(--muted))`;
}

/**
 * When-do-you-read donut. Four slices (morning / afternoon / evening /
 * night) sized by accumulated `active_ms`, the busiest one in the accent
 * and named in the hole. Empty when nothing has been recorded.
 */
export function TimeOfDayDonut({
  data,
  animate = true,
}: {
  data: TimeOfDayBuckets;
  /** Sweep the slices in on mount; pass `!useReducedMotion()`. */
  animate?: boolean;
}) {
  const base = BUCKETS.map((key) => ({
    key,
    label: BUCKET_LABELS[key],
    value: data[key].active_ms,
    sessions: data[key].sessions,
  }));
  const total = base.reduce((acc, r) => acc + r.value, 0);
  if (total === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        Read a few sessions to see when you usually pick up a book.
      </p>
    );
  }
  const ranked = [...base].sort((a, b) => b.value - a.value);
  const rows = base.map((r) => ({
    ...r,
    color: rankColor(ranked.indexOf(r)),
    pct: Math.round((r.value / total) * 100),
  }));
  const top = rows.find((r) => r.key === ranked[0]!.key)!;

  return (
    // Legend beside the donut only when the component itself is wide
    // enough (`@md` = 28rem): the viewport `sm:` split put the two
    // side by side inside a ~250px reading-log column, where each
    // legend row wrapped onto three lines and spilled out of the
    // card. Below that the legend stacks under the donut.
    <div className="@container">
      <div className="grid grid-cols-1 items-center gap-5 @md:grid-cols-2">
        <div className="relative">
          <ResponsiveContainer width="100%" height={184}>
            <PieChart>
              <Pie
                data={rows}
                dataKey="value"
                nameKey="label"
                innerRadius={58}
                outerRadius={84}
                paddingAngle={2}
                cornerRadius={3}
                stroke="var(--card)"
                strokeWidth={2}
                startAngle={90}
                endAngle={-270}
                isAnimationActive={animate}
              >
                {rows.map((r) => (
                  <Cell key={r.key} fill={r.color} />
                ))}
              </Pie>
              <Tooltip
                cursor={false}
                contentStyle={TOOLTIP_CONTENT_STYLE}
                labelStyle={TOOLTIP_LABEL_STYLE}
                itemStyle={TOOLTIP_ITEM_STYLE}
                formatter={(_value, name, item) => {
                  const payload = item?.payload as
                    { value?: number; sessions?: number } | undefined;
                  const ms = payload?.value ?? 0;
                  const sessions = payload?.sessions ?? 0;
                  return [
                    `${formatDurationMs(ms)} · ${sessions} session${sessions === 1 ? "" : "s"}`,
                    String(name),
                  ];
                }}
              />
            </PieChart>
          </ResponsiveContainer>
          <div
            aria-hidden="true"
            className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center"
          >
            <span className="text-foreground text-2xl font-semibold tabular-nums">
              {top.pct}%
            </span>
            <span className="text-muted-foreground text-xs">
              {top.label.toLowerCase()}
            </span>
          </div>
        </div>
        <ul className="space-y-2.5">
          {rows.map((r) => (
            <li
              key={r.key}
              className="text-foreground flex items-center justify-between gap-3 text-sm"
            >
              <span className="flex min-w-0 items-center gap-2.5">
                <span
                  aria-hidden="true"
                  className="inline-block size-2.5 shrink-0 rounded-full"
                  style={{ backgroundColor: r.color }}
                />
                <span className="truncate font-medium">{r.label}</span>
                <span className="text-muted-foreground hidden text-xs tabular-nums @xs:inline">
                  {BUCKET_HOURS[r.key]}
                </span>
              </span>
              <span className="text-muted-foreground shrink-0 text-xs whitespace-nowrap tabular-nums">
                <span className="text-foreground font-medium">{r.pct}%</span>
                {" · "}
                {formatDurationMs(r.value)}
              </span>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
