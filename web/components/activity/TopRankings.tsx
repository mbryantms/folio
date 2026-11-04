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
  "series" | "publishers" | "imprints" | "genres" | "tags" | CreatorRole;

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
  // Rank by the chosen metric so the leader (and the bar scale) follow
  // the Time / Sessions toggle; the server orders by time.
  const rows = [...entriesFor(current, data)].sort(
    (a, b) => b[metric] - a[metric],
  );

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
      <ol className="space-y-1">
        {rows.slice(0, 10).map((row, i) => (
          <li
            key={`${current}-${row.key}`}
            className="hover:bg-muted/50 -mx-2 flex items-start gap-3 rounded-md px-2 py-1.5"
          >
            <span className="text-muted-foreground w-5 shrink-0 pt-0.5 text-right text-xs tabular-nums">
              {i + 1}
            </span>
            <span className="min-w-0 flex-1 space-y-1.5">
              <span className="flex items-baseline justify-between gap-3 text-sm">
                {row.href ? (
                  <Link
                    href={row.href}
                    className="text-foreground min-w-0 truncate font-medium hover:underline"
                  >
                    {row.label}
                  </Link>
                ) : (
                  <span className="text-foreground min-w-0 truncate font-medium">
                    {row.label}
                  </span>
                )}
                <span className="text-muted-foreground shrink-0 text-xs whitespace-nowrap tabular-nums">
                  {metric === "active_ms"
                    ? formatDurationMs(row.active_ms)
                    : `${row.sessions} session${row.sessions === 1 ? "" : "s"}`}
                </span>
              </span>
              <Bar metric={metric} row={row} max={rows[0]} lead={i === 0} />
            </span>
          </li>
        ))}
      </ol>
    </div>
  );
}

/** Inline share bar under each row, relative to the leader. The leader is
 *  the full accent; the rest are a softer accent so the order reads at a
 *  glance. */
function Bar({
  metric,
  row,
  max,
  lead,
}: {
  metric: Metric;
  row: Row;
  max: Row | undefined;
  lead: boolean;
}) {
  const denom =
    metric === "active_ms"
      ? Math.max(max?.active_ms ?? 0, 1)
      : Math.max(max?.sessions ?? 0, 1);
  const num = metric === "active_ms" ? row.active_ms : row.sessions;
  const pct = Math.max(2, Math.min(100, (num / denom) * 100));
  return (
    <span
      aria-hidden="true"
      className="bg-muted block h-1.5 w-full overflow-hidden rounded-full"
    >
      <span
        className="block h-full rounded-full transition-[width] duration-500 ease-out motion-reduce:transition-none"
        style={{
          width: `${pct}%`,
          backgroundColor: lead
            ? "var(--primary)"
            : "color-mix(in oklab, var(--primary) 62%, var(--muted))",
        }}
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
      return (data.top_series ?? []).map(seriesRow);
    case "publishers":
      return (data.top_publishers ?? []).map(nameRow);
    case "imprints":
      return (data.top_imprints ?? []).map(nameRow);
    case "genres":
      return (data.top_genres ?? []).map(nameRow);
    case "tags":
      return (data.top_tags ?? []).map(nameRow);
    default:
      return (data.top_creators ?? [])
        .filter((c) => c.role === dim)
        .map(creatorRow);
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
