"use client";

import { Skeleton } from "@/components/ui/skeleton";
import { useReadingStats } from "@/lib/api/queries";
import { formatDurationMs } from "@/lib/activity";

import { WidgetCard } from "../WidgetCard";
import type { LogWidgetProps, RankingConfig } from "./types";

/** Top publishers by `active_ms` in the page's selected range —
 *  surfaces which catalogs the user actually spends time inside.
 *  Backed by `/me/reading-stats.top_publishers`. */
export function TopPublishers({
  widget,
  scope,
}: LogWidgetProps<RankingConfig>) {
  const range = widget.config.range ?? scope.range;
  const limit = widget.config.limit ?? 5;
  const stats = useReadingStats({ type: "all" }, range);
  const rows = (stats.data?.top_publishers ?? []).slice(0, limit);

  return (
    <WidgetCard
      widget={widget}
      title="Top publishers"
      subtitle={`Last ${range}`}
    >
      <RankingBody
        loading={stats.isLoading}
        rows={rows}
        emptyHint="No publisher activity yet."
      />
    </WidgetCard>
  );
}

function RankingBody({
  loading,
  rows,
  emptyHint,
}: {
  loading: boolean;
  rows: { name: string; active_ms: number }[];
  emptyHint: string;
}) {
  if (loading) {
    return (
      <div className="space-y-2">
        <Skeleton className="h-3 w-3/4" />
        <Skeleton className="h-3 w-2/3" />
        <Skeleton className="h-3 w-1/2" />
      </div>
    );
  }
  if (rows.length === 0) {
    return <p className="text-muted-foreground text-xs">{emptyHint}</p>;
  }
  const max = rows.reduce((m, r) => Math.max(m, r.active_ms), 0) || 1;
  return (
    <ol className="flex flex-col gap-2">
      {rows.map((r, i) => {
        const pct = Math.max(2, Math.round((r.active_ms / max) * 100));
        return (
          <li key={`${r.name}-${i}`} className="flex flex-col gap-0.5">
            <div className="flex items-center justify-between text-sm">
              <span className="truncate" title={r.name}>
                {r.name}
              </span>
              <span className="text-muted-foreground/80 ml-2 text-xs tabular-nums">
                {formatDurationMs(r.active_ms)}
              </span>
            </div>
            <div
              aria-hidden
              className="bg-muted/40 h-1.5 overflow-hidden rounded-full"
            >
              <div
                className="bg-primary/70 h-full rounded-full"
                style={{ width: `${pct}%` }}
              />
            </div>
          </li>
        );
      })}
    </ol>
  );
}

export { RankingBody };
