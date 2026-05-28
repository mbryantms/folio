"use client";

/**
 * Auto-synced tab — the opt-in set the weekly refresh cron will touch.
 *
 * Auto-sync defaults OFF per series (metadata for the bulk of a library
 * rarely changes); operators turn it on for the few new/popular series
 * worth refreshing. This lists those series (auto-sync ON) with a
 * one-click "Stop" to opt a series back out. Auto-sync is series-level,
 * so issues inherit their series' setting.
 */

import { useQueryClient } from "@tanstack/react-query";
import { Loader2, PauseCircle } from "lucide-react";
import Link from "next/link";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { usePauseMetadataSync } from "@/lib/api/mutations";
import { queryKeys, useAdminMetadataAutoSynced } from "@/lib/api/queries";
import { formatRelativeDate } from "@/lib/format";
import type { AutoSyncedSeriesRow } from "@/lib/api/types";

export function AutoSyncedTab() {
  const q = useAdminMetadataAutoSynced();

  if (q.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-6 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading…
      </div>
    );
  }

  const rows = q.data?.series ?? [];

  if (rows.length === 0) {
    return (
      <Card>
        <CardContent className="text-muted-foreground py-8 text-center text-sm">
          No series have auto-sync enabled. Auto-sync is opt-in — turn it on
          from a series&rsquo; <span className="font-medium">Details</span> tab.
          The weekly refresh cron only refreshes series listed here.
        </CardContent>
      </Card>
    );
  }

  return (
    <div className="space-y-3">
      <p className="text-muted-foreground text-sm">
        {rows.length} series set to auto-sync — the weekly refresh cron
        refreshes these; everything else is left alone. (Issues inherit their
        series&rsquo; setting.)
      </p>
      <ul className="border-border divide-border divide-y rounded-md border">
        {rows.map((r) => (
          <AutoSyncedRow key={r.id} row={r} />
        ))}
      </ul>
    </div>
  );
}

function AutoSyncedRow({ row }: { row: AutoSyncedSeriesRow }) {
  const qc = useQueryClient();
  const pause = usePauseMetadataSync(row.slug);

  return (
    <li className="bg-background flex items-center justify-between gap-3 px-3 py-2">
      <div className="min-w-0">
        <Link
          href={`/series/${row.slug}`}
          className="hover:text-foreground block truncate text-sm font-medium"
          title={row.name}
        >
          {row.name}
          {row.year ? ` (${row.year})` : ""}
        </Link>
        <p className="text-muted-foreground truncate text-xs">
          {row.library_name} · last synced{" "}
          {row.last_metadata_sync_at
            ? formatRelativeDate(row.last_metadata_sync_at)
            : "never"}
        </p>
      </div>
      <Button
        size="sm"
        variant="ghost"
        className="text-muted-foreground hover:text-foreground shrink-0"
        onClick={() =>
          pause.mutate(undefined, {
            onSuccess: () =>
              qc.invalidateQueries({
                queryKey: queryKeys.adminMetadataAutoSynced,
              }),
          })
        }
        disabled={pause.isPending}
        aria-label={`Stop auto-syncing ${row.name}`}
      >
        {pause.isPending ? (
          <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
        ) : (
          <PauseCircle className="mr-1 h-3.5 w-3.5" />
        )}
        Stop
      </Button>
    </li>
  );
}
