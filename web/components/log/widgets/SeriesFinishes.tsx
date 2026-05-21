"use client";

import * as React from "react";
import Link from "next/link";
import { ListChecks } from "lucide-react";

import { Skeleton } from "@/components/ui/skeleton";
import { useReadingLogInfinite } from "@/lib/api/queries";
import { formatRelativeDate } from "@/lib/format";
import { seriesUrl } from "@/lib/urls";
import type { ReadingLogFilters, ReadingStatsRange } from "@/lib/api/types";

import { WidgetCard } from "../WidgetCard";
import type { LogWidgetProps, RankingConfig } from "./types";

function rangeToFrom(range: ReadingStatsRange): string | undefined {
  if (range === "all") return undefined;
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
  const cutoff = new Date();
  cutoff.setDate(cutoff.getDate() - days);
  return cutoff.toISOString();
}

/** Recent series finishes — pulls `kind=series_finished` from the
 *  reading-log feed and shows the latest N. The same event powers
 *  the rows that appear in `ChronoFeed`, but here the user gets a
 *  focused list when "I just finished Sandman; what else is done?"
 *  is the question. */
export function SeriesFinishes({
  widget,
  scope,
}: LogWidgetProps<RankingConfig>) {
  const range = widget.config.range ?? scope.range;
  const limit = widget.config.limit ?? 5;
  const filters: ReadingLogFilters = React.useMemo(
    () => ({
      kinds: ["series_finished"],
      from: rangeToFrom(range),
      limit,
    }),
    [range, limit],
  );
  const query = useReadingLogInfinite(filters);
  const events = (query.data?.pages.flatMap((p) => p.events) ?? []).slice(
    0,
    limit,
  );

  return (
    <WidgetCard
      widgetId={widget.id}
      title="Series finished"
      subtitle={`Last ${range}`}
      Icon={ListChecks}
    >
      {query.isLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-4 w-3/4" />
          <Skeleton className="h-4 w-2/3" />
          <Skeleton className="h-4 w-1/2" />
        </div>
      ) : events.length === 0 ? (
        <p className="text-muted-foreground text-xs">
          No series wrapped up in this window.
        </p>
      ) : (
        <ol className="flex flex-col gap-2">
          {events.map((e) => {
            const slug = e.series?.slug;
            const name = e.series?.name ?? "Series";
            const meta =
              e.payload.kind === "series_finished" && e.payload.total_issues > 0
                ? `${e.payload.total_issues} issue${e.payload.total_issues === 1 ? "" : "s"}`
                : null;
            const inner = (
              <div className="hover:bg-muted/50 flex items-center gap-2 rounded-md p-1 transition-colors">
                <div className="flex min-w-0 flex-1 flex-col">
                  <span className="truncate text-sm font-medium" title={name}>
                    {name}
                  </span>
                  <span className="text-muted-foreground text-xs">
                    {formatRelativeDate(e.occurred_at)}
                    {meta ? <span> · {meta}</span> : null}
                  </span>
                </div>
              </div>
            );
            return (
              <li key={e.id}>
                {slug ? <Link href={seriesUrl(slug)}>{inner}</Link> : inner}
              </li>
            );
          })}
        </ol>
      )}
    </WidgetCard>
  );
}
