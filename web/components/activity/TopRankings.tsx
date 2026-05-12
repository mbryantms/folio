"use client";

import { useMemo, useState } from "react";
import Link from "next/link";

import { SegmentedControl } from "@/components/settings/SegmentedControl";
import { Skeleton } from "@/components/ui/skeleton";
import { formatDurationMs } from "@/lib/activity";
import { useReadingStats, type ReadingStatsScope } from "@/lib/api/queries";
import type {
  CreatorRole,
  ReadingStatsRange,
  ReadingStatsView,
  TopCreatorEntry,
  TopNameEntry,
  TopSeriesEntry,
} from "@/lib/api/types";

type Dimension =
  | "series"
  | "publishers"
  | "imprints"
  | "genres"
  | "tags"
  | CreatorRole;

const DIMENSION_LABELS: Record<Dimension, string> = {
  series: "Series",
  publishers: "Publishers",
  imprints: "Imprints",
  genres: "Genres",
  tags: "Tags",
  writer: "Writers",
  penciller: "Pencillers",
  inker: "Inkers",
  colorist: "Colorists",
  letterer: "Letterers",
  cover_artist: "Cover artists",
  editor: "Editors",
  translator: "Translators",
};

const DEFAULT_ORDER: ReadonlyArray<Dimension> = [
  "series",
  "writer",
  "penciller",
  "inker",
  "colorist",
  "letterer",
  "cover_artist",
  "publishers",
  "imprints",
  "genres",
  "tags",
];

type Metric = "active_ms" | "sessions";

/**
 * Self-fetching wrapper for the user / settings page. Admin / per-series
 * surfaces pass data straight to `<TopRankingsList />` so they can avoid
 * re-fetching.
 */
export function TopRankings({
  scope,
  range,
}: {
  scope: ReadingStatsScope;
  range: ReadingStatsRange;
}) {
  const stats = useReadingStats(scope, range);
  if (stats.isLoading) return <Skeleton className="h-48 w-full" />;
  if (stats.error || !stats.data) return null;
  return (
    <TopRankingsList data={stats.data} emptyHint={emptyHintForScope(scope)} />
  );
}

export function TopRankingsList({
  data,
  emptyHint,
  /** Optional override for the dimension list. The series-scope page hides
   *  series/publishers/imprints since they'd be tautological. */
  dimensions,
}: {
  data: Pick<
    ReadingStatsView,
    | "top_series"
    | "top_genres"
    | "top_tags"
    | "top_publishers"
    | "top_imprints"
    | "top_creators"
  >;
  emptyHint?: string;
  dimensions?: ReadonlyArray<Dimension>;
}) {
  const available = useMemo<ReadonlyArray<Dimension>>(() => {
    const all = dimensions ?? DEFAULT_ORDER;
    return all.filter((d) => entriesFor(d, data).length > 0);
  }, [data, dimensions]);

  const [active, setActive] = useState<Dimension | null>(null);
  const [metric, setMetric] = useState<Metric>("active_ms");

  const current = active ?? available[0] ?? null;
  if (!current) {
    return (
      <p className="text-muted-foreground text-sm">
        {emptyHint ?? "Read a few issues to populate the rankings."}
      </p>
    );
  }
  const rows = entriesFor(current, data);

  return (
    <div className="space-y-3">
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <SegmentedControl
          value={current}
          onChange={(v) => setActive(v as Dimension)}
          options={available.map((d) => ({
            value: d,
            label: DIMENSION_LABELS[d],
          }))}
          ariaLabel="Ranking dimension"
        />
        <SegmentedControl
          value={metric}
          onChange={(v) => setMetric(v as Metric)}
          options={[
            { value: "active_ms", label: "Time" },
            { value: "sessions", label: "Sessions" },
          ]}
          ariaLabel="Ranking metric"
        />
      </div>
      <ol className="divide-border bg-card border-border divide-y rounded-md border">
        {rows.map((row, i) => (
          <li
            key={`${current}-${row.key}`}
            className="flex items-center gap-3 px-3 py-2 text-sm"
          >
            <span className="text-muted-foreground w-5 shrink-0 text-xs tabular-nums">
              {i + 1}.
            </span>
            <span className="min-w-0 flex-1 truncate">
              {row.href ? (
                <Link
                  href={row.href}
                  className="text-foreground font-medium hover:underline"
                >
                  {row.label}
                </Link>
              ) : (
                <span className="text-foreground font-medium">{row.label}</span>
              )}
            </span>
            <Bar metric={metric} row={row} max={rows[0]} />
            <span className="text-muted-foreground w-24 shrink-0 text-right text-xs tabular-nums">
              {metric === "active_ms"
                ? formatDurationMs(row.active_ms)
                : `${row.sessions} session${row.sessions === 1 ? "" : "s"}`}
            </span>
          </li>
        ))}
      </ol>
    </div>
  );
}

function Bar({
  metric,
  row,
  max,
}: {
  metric: Metric;
  row: Row;
  max: Row | undefined;
}) {
  const denom =
    metric === "active_ms"
      ? Math.max(max?.active_ms ?? 0, 1)
      : Math.max(max?.sessions ?? 0, 1);
  const num = metric === "active_ms" ? row.active_ms : row.sessions;
  const pct = Math.max(0, Math.min(100, (num / denom) * 100));
  return (
    <span
      aria-hidden="true"
      className="bg-muted hidden h-1.5 w-24 overflow-hidden rounded-full sm:block"
    >
      <span
        className="bg-primary block h-full rounded-full"
        style={{ width: `${pct}%` }}
      />
    </span>
  );
}

type Row = {
  key: string;
  label: string;
  href?: string;
  sessions: number;
  active_ms: number;
};

function entriesFor(
  dim: Dimension,
  data: Pick<
    ReadingStatsView,
    | "top_series"
    | "top_genres"
    | "top_tags"
    | "top_publishers"
    | "top_imprints"
    | "top_creators"
  >,
): Row[] {
  switch (dim) {
    case "series":
      return data.top_series.map(seriesRow);
    case "publishers":
      return data.top_publishers.map(nameRow);
    case "imprints":
      return data.top_imprints.map(nameRow);
    case "genres":
      return data.top_genres.map(nameRow);
    case "tags":
      return data.top_tags.map(nameRow);
    default:
      return data.top_creators.filter((c) => c.role === dim).map(creatorRow);
  }
}

function seriesRow(s: TopSeriesEntry): Row {
  return {
    key: s.series_id,
    label: s.name,
    href: `/series/${s.series_id}`,
    sessions: s.sessions,
    active_ms: s.active_ms,
  };
}

function nameRow(s: TopNameEntry): Row {
  return {
    key: s.name,
    label: s.name,
    sessions: s.sessions,
    active_ms: s.active_ms,
  };
}

function creatorRow(c: TopCreatorEntry): Row {
  return {
    key: `${c.role}:${c.person}`,
    label: c.person,
    sessions: c.sessions,
    active_ms: c.active_ms,
  };
}

function emptyHintForScope(scope: ReadingStatsScope): string {
  return scope.type === "series"
    ? "No tag/credit metadata recorded for issues in this series."
    : "Read a few issues to populate the rankings.";
}
