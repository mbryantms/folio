"use client";

import { dowHourBucket, formatDurationMs } from "@/lib/activity";
import type { DowHourCell } from "@/lib/api/types";

/**
 * 7×24 day-of-week × hour heatmap (in user's timezone, server-bucketed).
 * Each cell's opacity scales against the max-value cell so a single
 * outlier doesn't wash everything out. The server returns only non-zero
 * cells; we fill the rest as 0-intensity squares.
 */
export function DowHourHeatmap({
  cells,
}: {
  cells: ReadonlyArray<DowHourCell>;
}) {
  const lookup = new Map<string, DowHourCell>();
  let max = 0;
  for (const c of cells) {
    lookup.set(`${c.dow}:${c.hour}`, c);
    if (c.active_ms > max) max = c.active_ms;
  }

  const CELL = 18;
  const GAP = 3;
  const STRIDE = CELL + GAP;
  const PAD_LEFT = 36;
  const PAD_TOP = 16;
  const width = PAD_LEFT + 24 * STRIDE;
  const height = PAD_TOP + 7 * STRIDE;

  return (
    <figure className="space-y-2">
      <svg
        viewBox={`0 0 ${width} ${height}`}
        width={width}
        height={height}
        style={{ maxWidth: "100%", height: "auto" }}
        className="text-primary block"
        role="img"
        aria-label="Reading time by day of week and hour"
      >
        {[0, 6, 12, 18, 23].map((h) => (
          <text
            key={`hour-${h}`}
            x={PAD_LEFT + h * STRIDE + CELL / 2}
            y={11}
            fontSize={10}
            textAnchor="middle"
            className="fill-muted-foreground"
          >
            {h}
          </text>
        ))}
        {DAYS.map((label, row) => (
          <text
            key={label}
            x={0}
            y={PAD_TOP + row * STRIDE + CELL - 3}
            fontSize={11}
            className="fill-muted-foreground"
          >
            {label}
          </text>
        ))}
        {Array.from({ length: 7 }).map((_, row) =>
          Array.from({ length: 24 }).map((_, col) => {
            const cell = lookup.get(`${row}:${col}`);
            const value = cell?.active_ms ?? 0;
            const intensity = dowHourBucket(value, max);
            const opacity = INTENSITY_OPACITY[intensity];
            const fill =
              intensity === 0 ? "var(--color-muted)" : "currentColor";
            return (
              <rect
                key={`${row}-${col}`}
                x={PAD_LEFT + col * STRIDE}
                y={PAD_TOP + row * STRIDE}
                width={CELL}
                height={CELL}
                rx={2}
                ry={2}
                fill={fill}
                fillOpacity={opacity}
              >
                <title>
                  {DAYS_FULL[row]} {col.toString().padStart(2, "0")}:00 —{" "}
                  {value > 0 ? formatDurationMs(value) : "no activity"}
                </title>
              </rect>
            );
          }),
        )}
      </svg>
      <Legend />
    </figure>
  );
}

const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const DAYS_FULL = [
  "Sunday",
  "Monday",
  "Tuesday",
  "Wednesday",
  "Thursday",
  "Friday",
  "Saturday",
];

const INTENSITY_OPACITY: Record<0 | 1 | 2 | 3 | 4, number> = {
  0: 1,
  1: 0.3,
  2: 0.55,
  3: 0.78,
  4: 1,
};

function Legend() {
  return (
    <figcaption className="text-muted-foreground flex items-center gap-1.5 text-xs">
      <span>Less</span>
      {([0, 1, 2, 3, 4] as const).map((level) => (
        <span
          key={level}
          aria-hidden="true"
          className="text-primary inline-block h-2.5 w-2.5 rounded-sm"
          style={{
            backgroundColor:
              level === 0 ? "var(--color-muted)" : "currentColor",
            opacity: INTENSITY_OPACITY[level],
          }}
        />
      ))}
      <span>More</span>
    </figcaption>
  );
}
