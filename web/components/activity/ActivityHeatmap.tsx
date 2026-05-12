"use client";

import {
  buildHeatmapGrid,
  formatDurationMs,
  type HeatmapCell,
} from "@/lib/activity";
import type { ReadingDayBucket } from "@/lib/api/types";

/**
 * Year-back (53-week) GitHub-style heatmap. Hand-rolled SVG — no chart
 * lib needed for what's essentially a colored-grid render. Cells with
 * activity get a `<title>` tooltip so hovering shows the date + duration.
 *
 * Color scale uses the `--color-primary`-derived steps already wired in
 * Tailwind v4's design tokens; intensities 0..4 map to opacity steps so
 * the same component reads correctly under any accent palette.
 */
export function ActivityHeatmap({
  perDay,
  today = new Date(),
}: {
  perDay: ReadonlyArray<ReadingDayBucket>;
  /** Override `today` so a series-scoped heatmap can be anchored to the
   *  user's tz-local "now"; the settings page leaves this default. */
  today?: Date;
}) {
  const grid = buildHeatmapGrid(perDay, today);
  const CELL = 12;
  const GAP = 3;
  const STRIDE = CELL + GAP;
  const COLS = grid.cells.length;
  const ROWS = 7;
  const LABEL_FONT = 11;
  // Reserve space on the left for day-of-week labels and on top for month
  // labels — both proportional to the label font size so the layout stays
  // balanced if either is tweaked.
  const PAD_TOP = LABEL_FONT + 6;
  const PAD_LEFT = 28;
  const width = PAD_LEFT + COLS * STRIDE;
  const height = PAD_TOP + ROWS * STRIDE;

  return (
    <figure className="space-y-2">
      {/* Render at natural pixel size so the in-SVG month/day labels and
          cells don't scale up with the container width. `maxWidth: 100%`
          plus `height: auto` shrinks the whole grid proportionally on
          narrow viewports while leaving wider pages alone. */}
      <svg
        viewBox={`0 0 ${width} ${height}`}
        width={width}
        height={height}
        style={{ maxWidth: "100%", height: "auto" }}
        className="text-primary block"
        role="img"
        aria-label="Reading activity, last 53 weeks"
      >
        {/* Month labels along the top edge. */}
        {grid.monthLabels.map((m) => (
          <text
            key={`${m.col}-${m.label}`}
            x={PAD_LEFT + m.col * STRIDE}
            y={LABEL_FONT}
            fontSize={LABEL_FONT}
            className="fill-muted-foreground"
          >
            {m.label}
          </text>
        ))}
        {/* Day-of-week labels — only Mon/Wed/Fri to keep things readable. */}
        {[
          { row: 1, label: "Mon" },
          { row: 3, label: "Wed" },
          { row: 5, label: "Fri" },
        ].map(({ row, label }) => (
          <text
            key={label}
            x={0}
            y={PAD_TOP + row * STRIDE + CELL - 1}
            fontSize={LABEL_FONT}
            className="fill-muted-foreground"
          >
            {label}
          </text>
        ))}
        {grid.cells.map((col, ci) =>
          col.map((cell, ri) => (
            <Cell
              key={`${ci}-${ri}`}
              x={PAD_LEFT + ci * STRIDE}
              y={PAD_TOP + ri * STRIDE}
              size={CELL}
              cell={cell}
              max={grid.max}
            />
          )),
        )}
      </svg>
      <Legend />
    </figure>
  );
}

function Cell({
  x,
  y,
  size,
  cell,
  max,
}: {
  x: number;
  y: number;
  size: number;
  cell: HeatmapCell;
  max: number;
}) {
  const opacity = INTENSITY_OPACITY[cell.intensity];
  const fill = cell.intensity === 0 ? "var(--color-muted)" : "currentColor";
  return (
    <rect
      x={x}
      y={y}
      width={size}
      height={size}
      rx={2}
      ry={2}
      fill={fill}
      fillOpacity={cell.inRange ? opacity : opacity * 0.4}
      stroke={cell.intensity === 0 ? "transparent" : "transparent"}
    >
      <title>
        {cell.date}:{" "}
        {cell.value > 0 ? formatDurationMs(cell.value) : "no activity"}
        {!cell.inRange ? " (future)" : ""}
        {max > 0 && cell.value > 0
          ? ` · ${Math.round((cell.value / max) * 100)}% of peak`
          : ""}
      </title>
    </rect>
  );
}

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
