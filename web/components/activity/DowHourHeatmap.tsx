"use client";

import { dowHourBucket, formatDurationMs } from "@/lib/activity";
import type { DowHourCell } from "@/lib/api/types";

/** The five heat steps of the reading log's visual language: the accent
 *  mixed into `--muted` (folio-flair phase 4). Level 0 is plain muted. */
export const HEAT_STEPS = [0, 30, 55, 78, 100] as const;

export function heatColor(level: 0 | 1 | 2 | 3 | 4): string {
  const pct = HEAT_STEPS[level];
  return pct === 0
    ? "var(--muted)"
    : `color-mix(in oklab, var(--primary) ${pct}%, var(--muted))`;
}

// Monday-first rows (matching the year heatmap); `dow` is the server's
// day number (0 = Sunday).
const ROWS: ReadonlyArray<{ dow: number; short: string; full: string }> = [
  { dow: 1, short: "Mon", full: "Monday" },
  { dow: 2, short: "Tue", full: "Tuesday" },
  { dow: 3, short: "Wed", full: "Wednesday" },
  { dow: 4, short: "Thu", full: "Thursday" },
  { dow: 5, short: "Fri", full: "Friday" },
  { dow: 6, short: "Sat", full: "Saturday" },
  { dow: 0, short: "Sun", full: "Sunday" },
];

const HOUR_LABELS: Readonly<Record<number, string>> = {
  0: "12a",
  6: "6a",
  12: "12p",
  18: "6p",
};

/**
 * 7×24 day-of-week × hour heatmap (user's timezone, server-bucketed). A
 * CSS grid rather than a scaled SVG, so the cells stretch to the card
 * while the labels stay at `text-xs` on a phone. Each cell's step scales
 * against the busiest cell so one outlier doesn't wash the rest out. The
 * server returns only non-zero cells; the rest render as empty squares.
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

  return (
    <figure className="flex flex-1 flex-col gap-3">
      <div
        role="img"
        aria-label="Reading time by day of week and hour"
        className="grid grid-cols-[auto_repeat(24,minmax(0,1fr))] gap-[2px] sm:gap-[3px]"
      >
        <span aria-hidden="true" />
        {Array.from({ length: 24 }, (_, h) => (
          <span
            key={`h-${h}`}
            aria-hidden="true"
            className="text-muted-foreground pb-1 text-xs leading-none whitespace-nowrap tabular-nums"
          >
            {HOUR_LABELS[h] ?? ""}
          </span>
        ))}
        {ROWS.map((row) => (
          <DayRow key={row.dow} row={row} lookup={lookup} max={max} />
        ))}
      </div>
      <HeatLegend />
      <RhythmSummary cells={cells} />
    </figure>
  );
}

const HOUR_12 = (h: number) =>
  `${h % 12 === 0 ? 12 : h % 12}${h < 12 ? "am" : "pm"}`;

/**
 * Three takeaways under the grid: the busiest hour slot, the busiest
 * weekday and how many of the week's 168 hours you've read in. Pinned to
 * the bottom of the card (`mt-auto`), so beside a taller Time of day card
 * the gap sits above it instead of under the legend.
 */
function RhythmSummary({ cells }: { cells: ReadonlyArray<DowHourCell> }) {
  if (cells.length === 0) return null;
  const peak = cells.reduce((a, b) => (b.active_ms > a.active_ms ? b : a));
  const byDay = new Map<number, number>();
  for (const c of cells)
    byDay.set(c.dow, (byDay.get(c.dow) ?? 0) + c.active_ms);
  const [busyDow, busyMs] = [...byDay.entries()].reduce((a, b) =>
    b[1] > a[1] ? b : a,
  );
  const dayName = (dow: number) => ROWS.find((r) => r.dow === dow)?.full ?? "";
  const items = [
    {
      label: "Peak hour",
      value: `${ROWS.find((r) => r.dow === peak.dow)?.short ?? ""} ${HOUR_12(peak.hour)}`,
      sub: formatDurationMs(peak.active_ms),
    },
    {
      label: "Busiest day",
      value: dayName(busyDow),
      sub: formatDurationMs(busyMs),
    },
    {
      label: "Hours of the week",
      value: `${cells.filter((c) => c.active_ms > 0).length} of 168`,
      sub: "with some reading",
    },
  ];
  return (
    <dl className="border-border mt-auto grid grid-cols-3 gap-3 border-t pt-4">
      {items.map((it) => (
        <div key={it.label} className="min-w-0">
          <dt className="text-muted-foreground truncate text-xs tracking-wide uppercase">
            {it.label}
          </dt>
          <dd className="text-foreground mt-1 truncate text-base font-semibold tabular-nums">
            {it.value}
          </dd>
          <dd className="text-muted-foreground truncate text-xs">{it.sub}</dd>
        </div>
      ))}
    </dl>
  );
}

function DayRow({
  row,
  lookup,
  max,
}: {
  row: (typeof ROWS)[number];
  lookup: Map<string, DowHourCell>;
  max: number;
}) {
  return (
    <>
      <span
        aria-hidden="true"
        className="text-muted-foreground flex items-center pr-2 text-xs leading-none"
      >
        {row.short}
      </span>
      {Array.from({ length: 24 }, (_, hour) => {
        const value = lookup.get(`${row.dow}:${hour}`)?.active_ms ?? 0;
        const level = dowHourBucket(value, max);
        return (
          <span
            key={hour}
            className="aspect-square rounded-[3px]"
            style={{ backgroundColor: heatColor(level) }}
            title={`${row.full} ${hour.toString().padStart(2, "0")}:00 — ${
              value > 0 ? formatDurationMs(value) : "no reading"
            }`}
          />
        );
      })}
    </>
  );
}

/** "Less ▢▢▢▢▢ More" key for the five heat steps. */
export function HeatLegend() {
  return (
    <figcaption className="text-muted-foreground flex items-center justify-end gap-1.5 text-xs">
      <span>Less</span>
      {([0, 1, 2, 3, 4] as const).map((level) => (
        <span
          key={level}
          aria-hidden="true"
          className="inline-block size-2.5 rounded-[3px]"
          style={{ backgroundColor: heatColor(level) }}
        />
      ))}
      <span>More</span>
    </figcaption>
  );
}
