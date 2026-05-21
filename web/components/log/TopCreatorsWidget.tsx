"use client";

import * as React from "react";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { useReadingStats } from "@/lib/api/queries";
import { formatDurationMs } from "@/lib/activity";
import type { ReadingStatsRange } from "@/lib/api/types";

const LIMIT = 5;

/** Top creators — writers + pencillers — ranked by time read in the
 *  selected range. Pulls from `/me/reading-stats.top_creators` (a
 *  flat list across all roles); we filter + cap to the two roles
 *  most likely to be interesting per page footprint. */
export function TopCreatorsWidget({ range }: { range: ReadingStatsRange }) {
  const stats = useReadingStats({ type: "all" }, range);

  const groups = React.useMemo(() => {
    const all = stats.data?.top_creators ?? [];
    const writer = all.filter((c) => c.role === "writer").slice(0, LIMIT);
    const penciller = all.filter((c) => c.role === "penciller").slice(0, LIMIT);
    return [
      { label: "Writers", rows: writer },
      { label: "Pencillers", rows: penciller },
    ].filter((g) => g.rows.length > 0);
  }, [stats.data]);

  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="text-base">Top creators</CardTitle>
      </CardHeader>
      <CardContent>
        {stats.isLoading ? (
          <div className="space-y-3">
            <Skeleton className="h-3 w-1/3" />
            <Skeleton className="h-3 w-3/4" />
            <Skeleton className="h-3 w-2/3" />
          </div>
        ) : groups.length === 0 ? (
          <p className="text-muted-foreground text-xs">
            No creator credits yet in this range.
          </p>
        ) : (
          <div className="flex flex-col gap-4">
            {groups.map((g) => (
              <section key={g.label}>
                <h3 className="text-muted-foreground/80 mb-1.5 text-[10px] font-medium tracking-widest uppercase">
                  {g.label}
                </h3>
                <ol className="flex flex-col gap-1.5">
                  {g.rows.map((c, i) => (
                    <li
                      key={`${g.label}-${c.person}`}
                      className="flex items-center gap-2 text-sm"
                    >
                      <span className="text-muted-foreground/70 w-4 text-xs tabular-nums">
                        {i + 1}
                      </span>
                      <span className="truncate" title={c.person}>
                        {c.person}
                      </span>
                      <span className="text-muted-foreground/80 ml-auto text-xs tabular-nums">
                        {formatDurationMs(c.active_ms)}
                      </span>
                    </li>
                  ))}
                </ol>
              </section>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
}
