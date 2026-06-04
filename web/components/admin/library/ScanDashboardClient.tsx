"use client";

import * as React from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Loader2, CheckCircle2, XCircle, Clock } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { LibraryEventsList } from "@/components/admin/library/LibraryEventsList";
import { useScanBatch, useScanBatches } from "@/lib/api/queries";
import { useScanEvents } from "@/lib/api/scan-events";
import { cn } from "@/lib/utils";
import type {
  ScanBatchDetailView,
  ScanBatchView,
  ScanEvent,
} from "@/lib/api/types";

/** Live per-library progress, reduced from the WS stream + seeded from the
 *  batch detail's member runs. */
export type LibRow = {
  libraryId: string;
  name: string;
  state: string; // queued | running | complete | failed | cancelled
  completed: number;
  total: number;
  label: string | null;
};

export function pct(completed: number, total: number): number {
  if (total <= 0) return 0;
  return Math.min(100, Math.round((completed / total) * 100));
}

/**
 * Pure reducer: seed one row per member run, then overlay the live WS events
 * for this batch. `scan.started`/`completed`/`failed` are matched by
 * `batch_id`; the chatty `scan.progress` (untagged by design — M8) is matched
 * by `library_id` against the fixed member set. Exported for unit testing.
 */
export function buildLibRows(
  memberRuns: ScanBatchDetailView["member_runs"],
  events: ScanEvent[],
  batchId: string,
): LibRow[] {
  const map = new Map<string, LibRow>();
  for (const r of memberRuns) {
    map.set(r.library_id, {
      libraryId: r.library_id,
      name: r.library_name,
      state: r.state,
      completed: 0,
      total: 0,
      label: null,
    });
  }
  const members = new Set(map.keys());
  for (const ev of events) {
    if (ev.type === "scan.started" && ev.batch_id === batchId) {
      const row = map.get(ev.library_id);
      if (row && row.state === "queued") row.state = "running";
    } else if (ev.type === "scan.progress" && members.has(ev.library_id)) {
      const row = map.get(ev.library_id);
      if (row) {
        row.state = row.state === "queued" ? "running" : row.state;
        row.completed = ev.completed;
        row.total = ev.total;
        row.label = ev.current_label;
      }
    } else if (ev.type === "scan.completed" && ev.batch_id === batchId) {
      const row = map.get(ev.library_id);
      if (row) {
        row.state = "complete";
        row.completed = row.total || row.completed;
      }
    } else if (ev.type === "scan.failed" && ev.batch_id === batchId) {
      const row = map.get(ev.library_id);
      if (row) row.state = "failed";
    }
  }
  return [...map.values()].sort((a, b) => a.name.localeCompare(b.name));
}

/** Count of member rows in a terminal state. */
export function doneCount(rows: LibRow[]): number {
  return rows.filter(
    (r) =>
      r.state === "complete" || r.state === "failed" || r.state === "cancelled",
  ).length;
}

export function ScanDashboardClient() {
  const sp = useSearchParams();
  const batchId = sp.get("batch");

  if (!batchId) return <BatchPicker />;
  return <BatchView batchId={batchId} />;
}

/** No batch selected — show recent batches to drill into. */
function BatchPicker() {
  const { data, isLoading } = useScanBatches();
  if (isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-8 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading scan batches…
      </div>
    );
  }
  const batches = data?.items ?? [];
  if (batches.length === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        No “Scan all” runs yet. Trigger one from the Libraries header and it
        will appear here with live progress.
      </p>
    );
  }
  return (
    <ul className="space-y-2">
      {batches.map((b) => (
        <li key={b.id}>
          <Link
            href={`?batch=${b.id}`}
            className="border-border bg-card hover:bg-muted/40 flex items-center justify-between rounded-md border px-4 py-3 transition-colors"
          >
            <span className="flex items-center gap-3">
              <BatchStateBadge state={b.state} />
              <span className="text-sm">
                {b.library_count}{" "}
                {b.library_count === 1 ? "library" : "libraries"}
                {b.force ? " · content-verify" : ""}
              </span>
            </span>
            <span className="text-muted-foreground text-xs">
              {new Date(b.started_at).toLocaleString()}
            </span>
          </Link>
        </li>
      ))}
    </ul>
  );
}

function BatchView({ batchId }: { batchId: string }) {
  const { data: batch, isLoading } = useScanBatch(batchId);
  // Subscribe to the global scan stream; we filter to this batch's libraries
  // ourselves. No completion toasts here — the dashboard *is* the surface.
  const { events, status } = useScanEvents({ toastCompletions: false });

  // Seed per-library rows from the batch's member runs, then overlay live
  // events. Member set is fixed once the batch is created (M6).
  const rows = React.useMemo<LibRow[]>(
    () => (batch ? buildLibRows(batch.member_runs, events, batchId) : []),
    [batch, events, batchId],
  );

  if (isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-8 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading batch…
      </div>
    );
  }
  if (!batch) {
    return <p className="text-destructive text-sm">Scan batch not found.</p>;
  }

  const done = doneCount(rows);
  const overall = pct(done, rows.length);
  const terminal = batch.state !== "running";

  return (
    <div className="space-y-4">
      <Link
        href="?"
        className="text-muted-foreground hover:text-foreground text-sm"
      >
        ← All batches
      </Link>

      {/* Overall progress */}
      <Card>
        <CardHeader className="flex flex-row items-center justify-between pb-2">
          <CardTitle className="flex items-center gap-2 text-sm font-medium">
            <BatchStateBadge state={batch.state} />
            {done}/{rows.length} libraries done
          </CardTitle>
          <span className="text-muted-foreground text-xs">
            {status === "open" ? "live" : status}
          </span>
        </CardHeader>
        <CardContent>
          <Progress value={overall} className="h-2" />
        </CardContent>
      </Card>

      {/* Post-run summary (aggregated totals) once the batch is terminal. */}
      {terminal && (
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium">
              Summary — what changed
            </CardTitle>
          </CardHeader>
          <CardContent className="grid grid-cols-2 gap-3 sm:grid-cols-4">
            <Stat label="Issues added" value={batch.totals.files_added} />
            <Stat label="Issues updated" value={batch.totals.files_updated} />
            <Stat label="Series created" value={batch.totals.series_created} />
            <Stat label="Issues removed" value={batch.totals.issues_removed} />
            <Stat label="Restored" value={batch.totals.issues_restored} />
            <Stat label="Duplicates" value={batch.totals.files_duplicate} />
            <Stat
              label="Malformed"
              value={batch.totals.files_malformed}
              warn={batch.totals.files_malformed > 0}
            />
            <Stat label="Events recorded" value={batch.event_count} />
          </CardContent>
        </Card>
      )}

      {/* Per-library rows */}
      <ul className="divide-border border-border bg-card divide-y rounded-md border">
        {rows.map((r) => (
          <li key={r.libraryId} className="px-4 py-2.5">
            <div className="flex items-center justify-between gap-3">
              <span className="flex min-w-0 items-center gap-2">
                <RowIcon state={r.state} />
                <span className="truncate text-sm">{r.name}</span>
              </span>
              {r.state === "running" && r.total > 0 && (
                <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
                  {r.completed}/{r.total}
                </span>
              )}
            </div>
            {r.state === "running" && (
              <div className="mt-1.5 flex items-center gap-2">
                <Progress value={pct(r.completed, r.total)} className="h-1" />
              </div>
            )}
            {r.state === "running" && r.label && (
              <p className="text-muted-foreground mt-1 truncate text-xs">
                {r.label}
              </p>
            )}
          </li>
        ))}
      </ul>

      {/* Itemized manifest — every change recorded under this batch (M10). */}
      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">
            Changes ({batch.event_count})
          </CardTitle>
        </CardHeader>
        <CardContent>
          <LibraryEventsList batchId={batchId} showLibrary />
        </CardContent>
      </Card>
    </div>
  );
}

function Stat({
  label,
  value,
  warn,
}: {
  label: string;
  value: number;
  warn?: boolean;
}) {
  return (
    <div>
      <div
        className={`text-2xl font-semibold tabular-nums ${warn ? "text-destructive" : ""}`}
      >
        {value}
      </div>
      <div className="text-muted-foreground text-xs">{label}</div>
    </div>
  );
}

function RowIcon({ state }: { state: string }) {
  if (state === "complete")
    return <CheckCircle2 className="h-4 w-4 shrink-0 text-emerald-400" />;
  if (state === "failed" || state === "cancelled")
    return <XCircle className="text-destructive h-4 w-4 shrink-0" />;
  if (state === "running")
    return <Loader2 className="text-primary h-4 w-4 shrink-0 animate-spin" />;
  return <Clock className="text-muted-foreground h-4 w-4 shrink-0" />;
}

// State tones mirror the LogsClient level-chip convention
// (`border-{c}-500/40 text-{c}-400`) and the green completion checks in the
// per-library rows — so a finished batch reads as "done", not as the amber
// brand-accent (which signals attention).
const BATCH_TONE: Record<string, string> = {
  complete: "border-emerald-500/40 text-emerald-400",
  running: "border-sky-500/40 text-sky-400",
  partial_failed: "border-amber-500/40 text-amber-400",
  failed: "border-red-500/40 text-red-400",
};

function BatchStateBadge({ state }: { state: ScanBatchView["state"] }) {
  const tone = BATCH_TONE[state] ?? "border-border text-muted-foreground";
  const label = state === "partial_failed" ? "partial" : state;
  return (
    <Badge variant="outline" className={cn("capitalize", tone)}>
      {label}
    </Badge>
  );
}
