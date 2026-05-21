"use client";

import { BookOpen, Flame, Timer } from "lucide-react";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { useReadingStats } from "@/lib/api/queries";
import { formatTotalHours } from "@/lib/activity";
import type { ReadingStatsRange } from "@/lib/api/types";

/** Compact summary of the user's reading in the selected range —
 *  issues finished, hours read, current streak. Pulls from
 *  `/me/reading-stats`; the same endpoint backs the full activity
 *  dashboard, so the numbers track. */
export function StatsHeroWidget({ range }: { range: ReadingStatsRange }) {
  const stats = useReadingStats({ type: "all" }, range);
  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="text-base">At a glance</CardTitle>
      </CardHeader>
      <CardContent>
        {stats.isLoading ? (
          <div className="grid grid-cols-3 gap-3">
            <Skeleton className="h-14" />
            <Skeleton className="h-14" />
            <Skeleton className="h-14" />
          </div>
        ) : stats.data ? (
          <div className="grid grid-cols-3 gap-3">
            <Tile
              label="Issues"
              value={stats.data.totals.distinct_issues.toLocaleString()}
              Icon={BookOpen}
            />
            <Tile
              label="Time read"
              value={formatTotalHours(stats.data.totals.active_ms / 3_600_000)}
              Icon={Timer}
            />
            <Tile
              label="Streak"
              value={`${stats.data.totals.current_streak}d`}
              Icon={Flame}
            />
          </div>
        ) : (
          <p className="text-destructive text-sm">Failed to load stats.</p>
        )}
      </CardContent>
    </Card>
  );
}

function Tile({
  label,
  value,
  Icon,
}: {
  label: string;
  value: string;
  Icon: typeof BookOpen;
}) {
  return (
    <div className="border-border/60 bg-muted/30 flex flex-col gap-1 rounded-md border p-2.5">
      <Icon aria-hidden="true" className="text-muted-foreground h-3.5 w-3.5" />
      <div className="text-lg leading-tight font-semibold tabular-nums">
        {value}
      </div>
      <div className="text-muted-foreground text-[10px] tracking-wider uppercase">
        {label}
      </div>
    </div>
  );
}
