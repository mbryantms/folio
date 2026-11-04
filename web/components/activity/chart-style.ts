/**
 * Shared Recharts styling for the activity charts (reading log visual
 * language, folio-flair phase 4): no library chrome — no axis lines or tick
 * marks, dashed faint horizontal gridlines, `text-xs` muted tick labels and
 * a popover-toned tooltip. Spread these into the Recharts elements.
 */
import type { CSSProperties } from "react";

/** Axis tick text: `text-xs` (12px) in the muted foreground. */
export const AXIS_TICK = {
  fill: "var(--muted-foreground)",
  fontSize: 12,
} as const;

/** XAxis / YAxis props that drop the axis line and tick marks. */
export const AXIS_BARE = {
  axisLine: false,
  tickLine: false,
  tick: AXIS_TICK,
} as const;

/** CartesianGrid props: horizontal only, dashed, faint. */
export const GRID = {
  stroke: "var(--border)",
  strokeDasharray: "3 4",
  strokeOpacity: 0.7,
  vertical: false,
} as const;

export const TOOLTIP_CONTENT_STYLE: CSSProperties = {
  background: "var(--popover)",
  color: "var(--popover-foreground)",
  border: "1px solid var(--border)",
  borderRadius: 8,
  fontSize: 12,
  padding: "6px 10px",
  boxShadow: "0 4px 16px rgb(0 0 0 / 0.12)",
};

export const TOOLTIP_LABEL_STYLE: CSSProperties = {
  color: "var(--foreground)",
  fontWeight: 600,
  marginBottom: 2,
};

export const TOOLTIP_ITEM_STYLE: CSSProperties = {
  color: "var(--muted-foreground)",
  padding: 0,
};
