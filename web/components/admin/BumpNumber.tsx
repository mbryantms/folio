"use client";

import * as React from "react";

import { cn } from "@/lib/utils";

/**
 * A number that bumps (`count-bump`, globals.css) when it changes — the
 * live scan counters, the background-work tiles, the dashboard Scans card
 * (folio-flair). `up` bumps only on increases (a running tally); `quiet`
 * never bumps (a value that changes every tick and would never stop
 * bouncing). Reduced motion: the global reset makes the bump instant.
 */
export function BumpNumber({
  value,
  format = (v) => v.toLocaleString(),
  on = "change",
  className,
}: {
  value: number;
  format?: (v: number) => string;
  on?: "change" | "up" | "never";
  className?: string;
}) {
  const [bump, setBump] = React.useState(0);
  const [prev, setPrev] = React.useState(value);
  if (prev !== value) {
    setPrev(value);
    if (on === "change" || (on === "up" && value > prev)) setBump((b) => b + 1);
  }
  return (
    <span
      key={bump}
      className={cn(
        "inline-block tabular-nums",
        bump > 0 && "count-bump",
        className,
      )}
    >
      {format(value)}
    </span>
  );
}
