"use client";

import * as React from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Loader2, CheckCircle2, XCircle, Clock } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { BatchListSkeleton } from "@/components/skeletons/AdminShapes";
import { Skeleton } from "@/components/ui/skeleton";
import { BumpNumber } from "@/components/admin/BumpNumber";
import { InfiniteSentinel } from "@/components/folio/infinite-sentinel";
import { LibraryEventsList } from "@/components/admin/library/LibraryEventsList";
import { LiveScanPanel } from "@/components/admin/library/LiveScanPanel";
import { ScanRunsHistory } from "@/components/admin/library/ScanRunsHistory";
import { UrlViewTabs } from "@/components/admin/UrlViewTabs";
import { WatchersCard } from "@/components/admin/library/WatchersCard";
import { useScanBatch, useScanBatches } from "@/lib/api/queries";
import { useScanEvents } from "@/lib/api/scan-events";
import { BACKGROUND_WORK_HREF } from "@/lib/admin/background-work";
import { libraryScanHref, scanBatchHref, SCANS_HREF } from "@/lib/admin/routes";
import { statusTone, statusToneText } from "@/lib/ui/status-tone";
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
  /** Library slug, for the link to its Scan tab ("" if deleted). */
  slug: string;
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
      slug: r.library_slug,
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

/**
 * Admin → Scans (`/admin/scans`): the one place to watch scans.
 *
 *  - No `?batch=`: live cards for every running library (start a scan or
 *    "Scan all" from here), then history — every "Scan all" batch and every
 *    single scan run — and the file-watcher card.
 *  - `?batch=<id>`: one "Scan all" run — where the Scan all button lands.
 *    Overall progress, a status row per library, the rich live card for
 *    each library still scanning, and, once the batch closes, the results
 *    summary + itemized changes.
 *
 * A single library's scan has its own scoped view on that library's Scan
 * tab (`/admin/libraries/<slug>/scan`).
 */
export function ScanDashboardClient() {
  const sp = useSearchParams();
  const batchId = sp.get("batch");

  if (!batchId) {
    return (
      <div className="space-y-8">
        <LiveScanPanel />
        <section className="space-y-3" aria-labelledby="scan-history-heading">
          <h2 id="scan-history-heading" className="text-lg font-semibold">
            History
          </h2>
          <UrlViewTabs
            param="history"
            label="Scan history"
            views={[
              {
                value: "batches",
                label: "Scan all runs",
                content: <BatchPicker />,
              },
              {
                value: "runs",
                label: "Every scan",
                content: <ScanRunsHistory />,
              },
            ]}
          />
        </section>
        <WatchersCard />
      </div>
    );
  }
  return <BatchView batchId={batchId} />;
}

/** No batch selected — list every "Scan all" batch, newest first, to drill
 *  into. The list is an infinite query: an IntersectionObserver sentinel
 *  walks `next_cursor`, so older batches never fall off a first-page cap.
 *  Exported for the jsdom paging test. */
export function BatchPicker() {
  const query = useScanBatches();
  const { data, isLoading } = query;
  if (isLoading) return <BatchListSkeleton />;
  const batches = data?.pages.flatMap((p) => p.items) ?? [];
  if (batches.length === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        No “Scan all” runs yet. Start one above (or from the Libraries header)
        and it opens here with live progress.
      </p>
    );
  }
  return (
    <>
      <ul className="space-y-2">
        {batches.map((b) => (
          <li key={b.id}>
            <Link
              href={scanBatchHref(b.id)}
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
      <InfiniteSentinel query={query} label="Loading older batches…" />
    </>
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

  if (isLoading) return <BatchSkeleton />;
  if (!batch) {
    return <p className="text-destructive text-sm">Scan batch not found.</p>;
  }

  const done = doneCount(rows);
  const overall = pct(done, rows.length);
  const terminal = batch.state !== "running";

  return (
    <div className="space-y-4">
      <Link
        href={SCANS_HREF}
        className="text-muted-foreground hover:text-foreground text-sm"
      >
        ← All scans
      </Link>

      {/* Overall progress */}
      <Card>
        <CardHeader className="flex flex-row items-center justify-between pb-2">
          <CardTitle className="flex items-center gap-2 text-sm font-medium">
            <BatchStateBadge state={batch.state} />
            <span>
              <BumpNumber value={done} on="up" />/{rows.length} libraries done
            </span>
          </CardTitle>
          <span className="text-muted-foreground text-xs">
            {status === "open" ? "live" : status}
          </span>
        </CardHeader>
        <CardContent>
          <Progress
            value={overall}
            className="live-progress h-2"
            aria-label="Overall scan progress"
          />
        </CardContent>
      </Card>

      {/* The batch closes when its last scan finishes — which is when the
          thumbnail jobs those scans queued start. Say so, and point at the
          page that tracks them. */}
      {terminal && batch.post_scan.thumb_jobs > 0 && (
        <Link
          href={BACKGROUND_WORK_HREF}
          className={cn(
            "flex items-center justify-between gap-3 rounded-md border px-4 py-3 text-sm",
            statusTone("warning"),
          )}
        >
          <span className="flex min-w-0 items-center gap-2">
            <Loader2
              className="size-4 shrink-0 animate-spin"
              aria-hidden="true"
            />
            <span>
              Scans finished — still generating thumbnails:{" "}
              {batch.post_scan.thumb_jobs.toLocaleString("en-US")}{" "}
              {batch.post_scan.thumb_jobs === 1 ? "job" : "jobs"} across{" "}
              {batch.post_scan.libraries_pending}{" "}
              {batch.post_scan.libraries_pending === 1
                ? "library"
                : "libraries"}
              .
            </span>
          </span>
          <span className="shrink-0 text-xs font-medium">
            Background work →
          </span>
        </Link>
      )}

      {/* Post-run summary (aggregated totals) once the batch is terminal. */}
      {terminal && (
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium">
              Summary — what changed
            </CardTitle>
          </CardHeader>
          <CardContent className="grid grid-cols-2 gap-3 sm:grid-cols-3">
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
            <Stat
              label="Password-protected"
              value={batch.totals.files_encrypted}
              warn={batch.totals.files_encrypted > 0}
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
                {r.slug ? (
                  <Link
                    href={libraryScanHref(r.slug)}
                    className="truncate text-sm hover:underline"
                  >
                    {r.name}
                  </Link>
                ) : (
                  <span className="truncate text-sm">{r.name}</span>
                )}
              </span>
              {r.state === "running" && r.total > 0 && (
                <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
                  {r.completed}/{r.total}
                </span>
              )}
            </div>
            {r.state === "running" && (
              <div className="mt-1.5 flex items-center gap-2">
                <Progress
                  value={pct(r.completed, r.total)}
                  className="live-progress h-1"
                />
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

      {/* The full live card (ring, throughput, discoveries, log) for each
          member library scanning — or finished — while this page is open:
          the same card the Scans page shows, scoped to this batch. A batch
          opened after it ended has no live state, and the panel renders
          nothing. */}
      <LiveScanPanel libraryIds={batch.member_runs.map((r) => r.library_id)} />

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
        <BumpNumber value={value} format={String} />
      </div>
      <div className="text-muted-foreground text-xs">{label}</div>
    </div>
  );
}

function RowIcon({ state }: { state: string }) {
  if (state === "complete")
    return (
      <CheckCircle2
        className={cn("size-4 shrink-0", statusToneText("success"))}
      />
    );
  if (state === "failed" || state === "cancelled")
    return <XCircle className="text-destructive size-4 shrink-0" />;
  if (state === "running")
    return <Loader2 className="text-primary size-4 shrink-0 animate-spin" />;
  return <Clock className="text-muted-foreground size-4 shrink-0" />;
}

// State tones mirror the LogsClient level-chip convention
// (`border-{c}-500/40 text-{c}-400`) and the green completion checks in the
// per-library rows — so a finished batch reads as "done", not as the amber
// brand-accent (which signals attention).
const BATCH_TONE: Record<string, string> = {
  complete: statusTone("success"),
  running: statusTone("info"),
  partial_failed: statusTone("warning"),
  failed: statusTone("error"),
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

/** One batch's shape (back link, overall progress, library rows) while it
 *  loads. */
function BatchSkeleton() {
  return (
    <div className="space-y-4" role="status" aria-label="Loading batch">
      <Skeleton className="h-4 w-24" />
      <div className="border-border space-y-3 rounded-xl border p-6">
        <Skeleton className="h-5 w-48" />
        <Skeleton className="h-2 w-full rounded-full" />
      </div>
      <ul className="divide-border border-border bg-card divide-y rounded-md border">
        {Array.from({ length: 3 }, (_, i) => (
          <li key={i} className="flex items-center gap-2 px-4 py-3">
            <Skeleton className="size-4 rounded-full" />
            <Skeleton className="h-4 w-40" />
          </li>
        ))}
      </ul>
    </div>
  );
}
