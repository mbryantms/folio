"use client";

import * as React from "react";

import { ChronoFeedWidget } from "./ChronoFeedWidget";
import { HeatmapWidget } from "./HeatmapWidget";
import { LogHeader } from "./LogHeader";
import { StatsHeroWidget } from "./StatsHeroWidget";
import { TopCreatorsWidget } from "./TopCreatorsWidget";
import type {
  ReadingLogEventKind,
  ReadingLogFilters,
  ReadingStatsRange,
} from "@/lib/api/types";

const ALL_KINDS: ReadingLogEventKind[] = [
  "issue_finished",
  "series_finished",
  "session_completed",
  "marker_created",
];

/** Convert the page's selected range token into an RFC3339 `from`
 *  bound for the log feed. `all` returns `undefined` (unbounded). */
function rangeToFrom(range: ReadingStatsRange): string | undefined {
  if (range === "all") return undefined;
  const now = new Date();
  const days =
    range === "7d"
      ? 7
      : range === "30d"
        ? 30
        : range === "60d"
          ? 60
          : range === "90d"
            ? 90
            : 365;
  const cutoff = new Date(now);
  cutoff.setDate(now.getDate() - days);
  return cutoff.toISOString();
}

/** Top-level layout for `/log`. Owns the range + kind-filter state
 *  and threads it through the chronological feed and the right-rail
 *  widgets so all four surfaces stay in sync. */
export function ReadingLogPage() {
  const [range, setRange] = React.useState<ReadingStatsRange>("30d");
  const [kinds, setKinds] = React.useState<ReadingLogEventKind[]>(ALL_KINDS);

  const from = React.useMemo(() => rangeToFrom(range), [range]);
  const filters: ReadingLogFilters = React.useMemo(
    () => ({
      kinds: kinds.length === ALL_KINDS.length ? undefined : kinds,
      from,
      limit: 30,
    }),
    [kinds, from],
  );

  return (
    <div className="mx-auto flex w-full max-w-7xl flex-col gap-6 px-4 py-6 lg:px-6">
      <LogHeader
        range={range}
        onRangeChange={setRange}
        kinds={kinds}
        onKindsChange={setKinds}
      />
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
        <ChronoFeedWidget filters={filters} />
        <div className="flex flex-col gap-6">
          <StatsHeroWidget range={range} />
          <HeatmapWidget />
          <TopCreatorsWidget range={range} />
        </div>
      </div>
    </div>
  );
}
