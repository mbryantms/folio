"use client";

import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";

import { CountUp } from "@/components/CountUp";
import { FillProgress } from "@/components/FillProgress";
import { queryKeys, useSeries } from "@/lib/api/queries";
import type { SeriesView } from "@/lib/api/types";
import { formatReadingTimeCompact } from "@/lib/format";

/**
 * The series page's two per-user progress tiles (reading load remaining,
 * read progress). The page is server-rendered, so these read the series
 * through `useSeries` seeded with the server's copy: every progress write
 * (mark read / unread, bulk marks, the reader) invalidates the series
 * detail query (`invalidateSeriesProgress`), and the tiles refetch
 * instead of staying stale until a reload.
 */
export function SeriesProgressStats({ series }: { series: SeriesView }) {
  const qc = useQueryClient();
  // The server copy is the newest on arrival: a cache entry left by an
  // earlier visit may predate progress made on another device, and
  // `initialData` is ignored once an entry exists.
  useEffect(() => {
    qc.setQueryData(queryKeys.series(series.slug), series);
  }, [qc, series]);
  const live = useSeries(series.slug, { initialData: series }).data ?? series;
  const summary = live.progress_summary;
  const total = summary?.total ?? live.issue_count ?? live.total_issues ?? 0;
  return (
    <>
      <ReadingLoadStat
        totalPages={live.total_page_count ?? null}
        finishedPages={summary?.finished_pages ?? 0}
      />
      <ReadProgressStat read={summary?.finished ?? 0} total={total} />
    </>
  );
}

/**
 * Read-progress card for the stats grid. Replaces the duplicative
 * "Total pages" / "Reading time" / "Last updated" tiles with a meaningful
 * per-user metric: how many of this series's active issues the user has
 * finished. Empty bar at 0/N, full + accent label at N/N. Total pages and
 * reading time still live in the Details tab for editors who want them.
 */
function ReadProgressStat({ read, total }: { read: number; total: number }) {
  const pct =
    total > 0
      ? Math.max(0, Math.min(100, Math.round((read / total) * 100)))
      : 0;
  const complete = total > 0 && read === total;
  return (
    <div className="border-border bg-card flex flex-col gap-2 rounded-md border px-4 py-3">
      <div className="flex items-baseline justify-between">
        <span className="text-muted-foreground text-xs font-medium tracking-wider uppercase">
          Read progress
        </span>
        <span className="text-muted-foreground text-xs">
          {total > 0 ? <CountUp value={pct} format="percent" /> : "—"}
        </span>
      </div>
      <span className="text-lg leading-tight font-semibold">
        {total > 0 ? (
          <>
            <CountUp value={read} /> / {total}
          </>
        ) : (
          "—"
        )}
      </span>
      <FillProgress value={pct} label={`Read ${read} of ${total} issues`} />
      <span
        className={
          complete
            ? "text-primary text-xs font-medium"
            : "text-muted-foreground text-xs"
        }
      >
        {total === 0
          ? "No active issues"
          : complete
            ? "Series complete"
            : `${total - read} left`}
      </span>
    </div>
  );
}

/**
 * Reading-load card for the stats grid. Surfaces estimated time + page
 * count remaining to finish the series. Once the user has finished every
 * active issue, the card flips to a "re-read" estimate over the full page
 * count so it stays useful for completionists deciding whether to start
 * over. Mirrors the `<Stat>` shape so it sits flush next to the other
 * tiles. `total_page_count` is the server-aggregated sum across active
 * issues; we subtract `progress_summary.finished_pages` to get the
 * remainder without paginating the issue list client-side.
 */
function ReadingLoadStat({
  totalPages,
  finishedPages,
}: {
  totalPages: number | null;
  finishedPages: number;
}) {
  if (totalPages == null || totalPages <= 0) {
    return (
      <div className="border-border bg-card flex flex-col gap-1 rounded-md border px-4 py-3">
        <span className="text-muted-foreground text-xs font-medium tracking-wider uppercase">
          Reading load remaining
        </span>
        <span className="text-lg leading-tight font-semibold">—</span>
      </div>
    );
  }
  const remainingPages = Math.max(0, totalPages - finishedPages);
  const isReread = remainingPages === 0;
  // When the user has finished the whole series, fall back to the full
  // page count — the card now estimates a re-read instead of a finish.
  const pagesShown = isReread ? totalPages : remainingPages;
  const time = formatReadingTimeCompact(pagesShown);
  return (
    <div className="border-border bg-card flex flex-col gap-1 rounded-md border px-4 py-3">
      <div className="flex items-baseline justify-between">
        <span className="text-muted-foreground text-xs font-medium tracking-wider uppercase">
          Reading load remaining
        </span>
        {isReread ? (
          <span className="text-primary text-xs font-medium">re-read</span>
        ) : null}
      </div>
      <span className="text-lg leading-tight font-semibold tabular-nums">
        {time ? <CountUp value={pagesShown} format="readingTime" /> : "—"}
      </span>
      <span className="text-muted-foreground text-xs">
        <CountUp value={pagesShown} format="compactPages" /> pages
      </span>
    </div>
  );
}
