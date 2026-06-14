"use client";

import Link from "next/link";

import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import {
  useHealthIssues,
  useLibrary,
  useQueueDepth,
  useScanPreview,
  useScanRuns,
} from "@/lib/api/queries";
import { useTriggerScan } from "@/lib/api/mutations";
import { validateCron } from "@/lib/api/cron";
import { cn } from "@/lib/utils";
import { ScanModeMenu } from "./ScanModeMenu";

/** Render the cron schedule as the actual next run time — the raw
 *  cron string ("0 3 * * *") made admins parse cron in their heads on
 *  the at-a-glance status card. Falls back to the raw string when the
 *  stored value doesn't parse. */
function nextScheduledScan(cron: string | null | undefined): string {
  if (!cron) return "not scheduled";
  const parsed = validateCron(cron);
  if (!parsed.ok || parsed.nextRuns.length === 0) return cron;
  return parsed.nextRuns[0]!.toLocaleString();
}

export function LibraryOverview({ id }: { id: string }) {
  const lib = useLibrary(id);
  const runs = useScanRuns(id);
  const issues = useHealthIssues(id);
  const preview = useScanPreview(id);
  const queue = useQueueDepth({ intervalMs: 10_000 });
  const trigger = useTriggerScan(id);

  if (lib.isLoading) {
    return <Skeleton className="h-40 w-full" />;
  }
  if (lib.error || !lib.data) {
    return <p className="text-destructive text-sm">Library not found.</p>;
  }
  const last = runs.data?.[0];
  const scanPreview = preview.data;
  // Open count from the server-computed summary — the items array is a
  // paginated sample, so its length would undercount.
  const openIssueCount = issues.data?.counts?.open ?? 0;

  const stats = [
    {
      label: "Last scan",
      value: last?.started_at
        ? new Date(last.started_at).toLocaleString()
        : "Never",
    },
    { label: "Last state", value: last?.state ?? "—" },
    { label: "Open health issues", value: String(openIssueCount) },
    {
      label: "Thumbnail backlog",
      value:
        typeof scanPreview?.thumbnail_backlog === "number"
          ? String(scanPreview.thumbnail_backlog)
          : "—",
    },
  ];

  return (
    <div className="space-y-6">
      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        {stats.map((s) => (
          <Card key={s.label}>
            <CardContent className="space-y-1 p-4">
              <p className="text-muted-foreground text-xs font-medium tracking-widest uppercase">
                {s.label}
              </p>
              <p className="text-foreground text-lg font-medium">{s.value}</p>
            </CardContent>
          </Card>
        ))}
      </div>
      <Card>
        <CardContent className="grid gap-4 p-4 md:grid-cols-[1.4fr_1fr_1fr_1fr]">
          <div className="space-y-1">
            <p className="text-muted-foreground text-xs font-medium tracking-widest uppercase">
              Scanner status
            </p>
            <p className="text-sm">
              {scanPreview?.reason ?? "Ready for a normal incremental scan."}
            </p>
            <p className="text-muted-foreground text-xs">
              Last duration:{" "}
              <span className="text-foreground">
                {formatDuration(scanPreview?.last_scan_duration_ms)}
              </span>
            </p>
          </div>
          <StatusMetric
            label="Watcher"
            value={scanPreview?.watcher_status ?? "unknown"}
          />
          <StatusMetric
            label="Dirty folders"
            value={String(scanPreview?.dirty_folders ?? 0)}
          />
          <StatusMetric
            label="Server queue"
            value={queue.data?.total ? `${queue.data.total} pending` : "idle"}
          />
          <StatusMetric
            label="Known issues"
            value={
              typeof scanPreview?.known_issue_count === "number"
                ? String(scanPreview.known_issue_count)
                : "—"
            }
          />
          <StatusMetric
            label="Next scheduled scan"
            value={nextScheduledScan(lib.data.scan_schedule_cron)}
          />
          <StatusMetric
            label="Soft-delete window"
            value={`${lib.data.soft_delete_days}d`}
          />
          <StatusMetric
            label="Last recorded state"
            value={scanPreview?.last_scan_state ?? last?.state ?? "—"}
          />
        </CardContent>
      </Card>
      <div className="flex flex-wrap items-center gap-3">
        <ScanModeMenu
          isPending={trigger.isPending}
          isRunning={last?.state === "running"}
          onScan={(mode) => trigger.mutate({ mode })}
        />
        <Link
          href={`/admin/libraries/${id}/scan`}
          className={cn(
            "border-border hover:bg-secondary rounded-md border px-3 py-1.5 text-xs font-medium",
            last?.state === "running"
              ? "text-primary"
              : "text-muted-foreground",
          )}
        >
          Open live details
        </Link>
      </div>
    </div>
  );
}

function StatusMetric({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 space-y-1">
      <p className="text-muted-foreground text-xs font-medium tracking-widest uppercase">
        {label}
      </p>
      <p className="truncate text-sm font-medium">{value}</p>
    </div>
  );
}

function formatDuration(ms: number | null | undefined): string {
  if (typeof ms !== "number") return "—";
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${(ms / 60_000).toFixed(1)}m`;
}
