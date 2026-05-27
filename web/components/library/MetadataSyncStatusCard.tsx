"use client";

/**
 * Per-series metadata sync status card (metadata-providers-1.0 M5).
 *
 * Surfaces:
 * - `last_metadata_sync_at` ("Last synced 3 days ago" or "Never")
 * - `linked_source_count` (number of `external_ids` rows)
 * - Pause / Resume toggle (writes to `series.metadata_sync_paused`;
 *   future bulk-refresh / weekly cron skips paused series)
 *
 * Mounted on the series page settings sidebar alongside the
 * `<ExternalIdsCard>`.
 */

import { Loader2, PauseCircle, PlayCircle } from "lucide-react";
import * as React from "react";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import {
  usePauseMetadataSync,
  useResumeMetadataSync,
} from "@/lib/api/mutations";
import { useMetadataSyncStatus } from "@/lib/api/queries";

export function MetadataSyncStatusCard({
  seriesSlug,
  chrome = "card",
}: {
  seriesSlug: string;
  /**
   * `"card"` (default): legacy `<Card>` chrome — kept for the admin /
   * settings surfaces that still render this in a sidebar.
   *
   * `"bare"`: drops the Card chrome. Used by the series-page "Metadata
   * sync" tab where the tab label IS the title and a nested Card would
   * clash with the other tabs' content (MetadataGrid, ChipList, etc. —
   * none of which use Cards).
   */
  chrome?: "card" | "bare";
}) {
  const status = useMetadataSyncStatus(seriesSlug);
  const pause = usePauseMetadataSync(seriesSlug);
  const resume = useResumeMetadataSync(seriesSlug);

  const paused = status.data?.paused ?? false;
  const lastSync = status.data?.last_metadata_sync_at ?? null;
  const linked = status.data?.linked_source_count ?? 0;
  const pending = pause.isPending || resume.isPending;

  const onToggle = (next: boolean) => {
    if (next === paused || pending) return;
    if (next) pause.mutate(undefined);
    else resume.mutate(undefined);
  };

  const body = (
    <>
      <div className="flex items-center justify-between">
        <span className="text-muted-foreground">Last synced</span>
        <span>{formatLastSync(lastSync)}</span>
      </div>
      <div className="flex items-center justify-between">
        <span className="text-muted-foreground">Linked sources</span>
        <span>{linked}</span>
      </div>
      <div className="flex items-center justify-between pt-1">
        <span className="text-muted-foreground flex items-center gap-1.5">
          {paused ? (
            <PauseCircle className="h-4 w-4" />
          ) : (
            <PlayCircle className="h-4 w-4" />
          )}
          Auto-sync
        </span>
        <div className="flex items-center gap-2">
          {pending && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
          <Switch
            aria-label={paused ? "Resume auto-sync" : "Pause auto-sync"}
            checked={!paused}
            onCheckedChange={(next) => onToggle(!next)}
            disabled={pending || status.isLoading}
          />
        </div>
      </div>
    </>
  );

  if (chrome === "bare") {
    // Bare mode for the series-page Metadata sync tab. Constrained to
    // a single-column max-width so the labels and values don't fly out
    // across the full content width — keeps the row pairings readable
    // at the eye-tracking distance of a tab body.
    return <div className="max-w-md space-y-3 text-sm">{body}</div>;
  }
  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">Metadata sync</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3 text-sm">{body}</CardContent>
    </Card>
  );
}

function formatLastSync(iso: string | null): string {
  if (!iso) return "Never";
  const d = new Date(iso);
  const now = Date.now();
  const dt = now - d.getTime();
  const sec = Math.floor(dt / 1000);
  if (sec < 60) return "Just now";
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  if (day < 7) return `${day}d ago`;
  return d.toLocaleDateString();
}
