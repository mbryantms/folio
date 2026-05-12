"use client";

import { Cell, Pie, PieChart, ResponsiveContainer, Tooltip } from "recharts";

import { formatDurationMs } from "@/lib/activity";
import type { TimeOfDayBuckets } from "@/lib/api/types";

const BUCKET_LABELS = {
  morning: "Morning",
  afternoon: "Afternoon",
  evening: "Evening",
  night: "Night",
} as const;

// Four steps of the same accent so the donut reads as one design unit.
const BUCKET_OPACITY = {
  morning: 0.45,
  afternoon: 0.7,
  evening: 0.9,
  night: 1,
} as const;

/**
 * When-do-you-read donut. Four slices (morning / afternoon / evening / night)
 * sized by accumulated `active_ms`. Empty when nothing has been recorded.
 */
export function TimeOfDayDonut({ data }: { data: TimeOfDayBuckets }) {
  const rows = (["morning", "afternoon", "evening", "night"] as const).map(
    (key) => ({
      key,
      label: BUCKET_LABELS[key],
      opacity: BUCKET_OPACITY[key],
      value: data[key].active_ms,
      sessions: data[key].sessions,
    }),
  );
  const total = rows.reduce((acc, r) => acc + r.value, 0);
  if (total === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        Read a few sessions to see when you usually pick up a book.
      </p>
    );
  }

  return (
    <div className="grid grid-cols-1 items-center gap-4 sm:grid-cols-2">
      <ResponsiveContainer width="100%" height={180}>
        <PieChart>
          <Pie
            data={rows}
            dataKey="value"
            nameKey="label"
            innerRadius={50}
            outerRadius={80}
            paddingAngle={2}
            stroke="var(--color-background)"
            strokeWidth={2}
          >
            {rows.map((r) => (
              <Cell
                key={r.key}
                fill="var(--color-primary)"
                fillOpacity={r.opacity}
              />
            ))}
          </Pie>
          <Tooltip
            cursor={false}
            contentStyle={{
              background: "var(--color-popover)",
              border: "1px solid var(--color-border)",
              borderRadius: 6,
              fontSize: 12,
            }}
            labelStyle={{ color: "var(--color-foreground)" }}
            formatter={(_value, _name, item) => {
              const payload = item?.payload as
                | { value?: number; sessions?: number }
                | undefined;
              const ms = payload?.value ?? 0;
              const sessions = payload?.sessions ?? 0;
              return [
                `${formatDurationMs(ms)} · ${sessions} session${sessions === 1 ? "" : "s"}`,
                "Read",
              ];
            }}
          />
        </PieChart>
      </ResponsiveContainer>
      <ul className="space-y-1">
        {rows.map((r) => {
          const pct = total > 0 ? Math.round((r.value / total) * 100) : 0;
          return (
            <li
              key={r.key}
              className="text-foreground flex items-center justify-between gap-3 text-sm"
            >
              <span className="flex items-center gap-2">
                <span
                  aria-hidden="true"
                  className="bg-primary inline-block size-2.5 rounded-sm"
                  style={{ opacity: r.opacity }}
                />
                <span className="font-medium">{r.label}</span>
              </span>
              <span className="text-muted-foreground tabular-nums">
                {pct}% · {formatDurationMs(r.value)}
              </span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
