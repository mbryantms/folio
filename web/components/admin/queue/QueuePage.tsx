"use client";

import * as React from "react";
import {
  ChevronLeft,
  ChevronRight,
  Loader2,
  RotateCcw,
  Trash2,
} from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  useAuditLog,
  useDeadJobs,
  useDeadLetters,
  useQueueDepth,
} from "@/lib/api/queries";
import { usePurgeDeadJobs, useRetryDeadJob } from "@/lib/api/mutations";
import type { AuditEntryView, DeadJob } from "@/lib/api/types";
import { cn } from "@/lib/utils";

/**
 * `/admin/queue` — background-job queue surface (archive-rewrite-1.0 M7).
 *
 *   - **Overview**: live pending-job depth per queue (polled), incl. the
 *     archive page-edit queue.
 *   - **Archive operations**: recent archive edits from the audit log
 *     (`admin.issue.archive_edit{,.bulk}`) with per-row drill-down. Apalis
 *     can't enumerate in-flight jobs individually, so "recent" = the audit
 *     trail the worker writes on completion (success or failure).
 */
export function QueuePage() {
  return (
    <Tabs defaultValue="overview" className="mt-2">
      <TabsList>
        <TabsTrigger value="overview">Overview</TabsTrigger>
        <TabsTrigger value="failed">Failed jobs</TabsTrigger>
        <TabsTrigger value="archive">Archive operations</TabsTrigger>
      </TabsList>
      <TabsContent value="overview" className="mt-4">
        <QueueOverview />
      </TabsContent>
      <TabsContent value="failed" className="mt-4">
        <FailedJobs />
      </TabsContent>
      <TabsContent value="archive" className="mt-4">
        <ArchiveOperations />
      </TabsContent>
    </Tabs>
  );
}

const QUEUE_LABELS: { key: string; label: string }[] = [
  { key: "scan", label: "Library scans" },
  { key: "scan_series", label: "Series scans" },
  { key: "post_scan_thumbs", label: "Thumbnails" },
  { key: "post_scan_search", label: "Search index" },
  { key: "post_scan_dictionary", label: "Dictionary" },
  { key: "archive_edit", label: "Archive edits" },
];

/** Friendly labels for every apalis queue (incl. the metadata + sidecar
 *  queues that don't surface in the pending-depth overview). */
const QUEUE_LABEL_MAP: Record<string, string> = {
  scan: "Library scans",
  scan_series: "Series scans",
  post_scan_thumbs: "Thumbnails",
  post_scan_search: "Search index",
  post_scan_dictionary: "Dictionary",
  metadata_search_series: "Metadata search (series)",
  metadata_search_issue: "Metadata search (issue)",
  metadata_apply_series: "Metadata apply (series)",
  metadata_apply_issue: "Metadata apply (issue)",
  rewrite_issue_sidecars: "Sidecar rewrite",
  archive_edit: "Archive edits",
};

function queueLabel(key: string): string {
  return QUEUE_LABEL_MAP[key] ?? key;
}

function LoaderRow({ label }: { label: string }) {
  return (
    <div className="text-muted-foreground flex items-center gap-2 py-8 text-sm">
      <Loader2 className="h-4 w-4 animate-spin" /> {label}
    </div>
  );
}

function QueueOverview() {
  const q = useQueueDepth();
  if (q.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-8 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading queue depth…
      </div>
    );
  }
  if (!q.data) {
    return (
      <p className="text-destructive text-sm">Failed to load queue depth.</p>
    );
  }
  const view = q.data as unknown as Record<string, number>;
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
      {QUEUE_LABELS.map(({ key, label }) => (
        <Card key={key}>
          <CardHeader className="pb-1">
            <CardTitle className="text-muted-foreground text-xs font-medium">
              {label}
            </CardTitle>
          </CardHeader>
          <CardContent>
            <span className="text-2xl font-semibold tabular-nums">
              {view[key] ?? 0}
            </span>
          </CardContent>
        </Card>
      ))}
      <Card className="border-primary/40">
        <CardHeader className="pb-1">
          <CardTitle className="text-muted-foreground text-xs font-medium">
            Total pending
          </CardTitle>
        </CardHeader>
        <CardContent>
          <span className="text-2xl font-semibold tabular-nums">
            {q.data.total}
          </span>
        </CardContent>
      </Card>
    </div>
  );
}

function ArchiveOperations() {
  // `prefix.*` matches both the per-issue rows (admin.issue.archive_edit) and
  // the bulk summary rows (…​.bulk).
  const q = useAuditLog({ action: "admin.issue.archive_edit.*", limit: 50 });
  if (q.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-8 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading…
      </div>
    );
  }
  const items = q.data?.items ?? [];
  if (items.length === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        No archive operations yet. Run an edit from a series, collection, or
        reading-list multi-selection (&ldquo;Edit archives…&rdquo;).
      </p>
    );
  }
  return (
    <ul className="space-y-2">
      {items.map((row) => (
        <ArchiveOpRow key={row.id} row={row} />
      ))}
    </ul>
  );
}

function ArchiveOpRow({ row }: { row: AuditEntryView }) {
  const isBulk = row.action.endsWith(".bulk");
  const payload = (row.payload ?? {}) as Record<string, unknown>;
  const error = typeof payload.error === "string" ? payload.error : null;
  const when = new Date(row.created_at).toLocaleString();

  return (
    <li className="border-border bg-card rounded-md border">
      <details className="group">
        <summary className="flex cursor-pointer items-center justify-between gap-3 px-3 py-2 text-sm">
          <span className="flex min-w-0 items-center gap-2">
            <span
              className={`inline-flex shrink-0 items-center rounded-full px-2 py-0.5 text-xs ${
                error
                  ? "bg-destructive/15 text-destructive"
                  : "bg-muted text-muted-foreground"
              }`}
            >
              {isBulk ? "bulk" : "edit"}
            </span>
            <span className="truncate">
              {summarize(payload, isBulk, error)}
            </span>
          </span>
          <span className="text-muted-foreground shrink-0 text-xs">{when}</span>
        </summary>
        <div className="border-border border-t px-3 py-2">
          <div className="text-muted-foreground mb-1 text-xs">
            {row.actor_label ?? row.actor_id}
          </div>
          <pre className="bg-muted/40 overflow-x-auto rounded p-2 text-xs">
            {JSON.stringify(payload, null, 2)}
          </pre>
        </div>
      </details>
    </li>
  );
}

function summarize(
  payload: Record<string, unknown>,
  isBulk: boolean,
  error: string | null,
): string {
  if (error) return `Failed: ${error}`;
  if (isBulk) {
    const queued = numberOr(payload.queued, 0);
    const skipped = numberOr(payload.skipped, 0);
    return `Bulk edit — ${queued} queued${skipped ? `, ${skipped} skipped` : ""}`;
  }
  const before = numberOr(payload.page_count_before, null);
  const after = numberOr(payload.page_count_after, null);
  const issue = typeof payload.issue_id === "string" ? payload.issue_id : "";
  const pages =
    before != null && after != null ? ` — ${before} → ${after} pages` : "";
  return `Issue ${issue}${pages}`;
}

function numberOr<T>(v: unknown, fallback: T): number | T {
  return typeof v === "number" ? v : fallback;
}

/**
 * Failed-jobs tab (D8b): apalis moves a job to its dead set after it exhausts
 * its retries. This surfaces the per-queue counts as chips; drilling into a
 * queue lists its dead jobs (newest-first) with the stored error + payload, a
 * per-row Retry (re-enqueue), and a Clear-all purge.
 */
function FailedJobs() {
  const dl = useDeadLetters();
  const [selected, setSelected] = React.useState<string | null>(null);

  if (dl.isLoading) {
    return <LoaderRow label="Loading failed jobs…" />;
  }
  if (!dl.data) {
    return (
      <p className="text-destructive text-sm">Failed to load failed jobs.</p>
    );
  }

  const queues = dl.data.queues.filter((q) => q.count > 0);
  if (dl.data.total === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        No failed jobs. When a background job exhausts its retries it lands here
        so you can inspect the error and re-run it.
      </p>
    );
  }

  const active = selected ?? queues[0]?.queue ?? null;

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap gap-2">
        {queues.map((q) => (
          <button
            key={q.queue}
            type="button"
            onClick={() => setSelected(q.queue)}
            className={cn(
              "inline-flex items-center gap-2 rounded-md border px-3 py-1.5 text-sm transition-colors",
              active === q.queue
                ? "border-primary/50 bg-primary/10 text-foreground"
                : "border-border text-muted-foreground hover:bg-muted/50",
            )}
          >
            <span>{queueLabel(q.queue)}</span>
            <Badge variant="destructive">{q.count}</Badge>
          </button>
        ))}
      </div>
      {/* key on the queue so switching queues remounts with page reset to 1 */}
      {active ? <DeadJobList key={active} queue={active} /> : null}
    </div>
  );
}

const DEAD_PAGE_SIZE = 20;

function DeadJobList({ queue }: { queue: string }) {
  // Mounted fresh per queue (keyed by the parent), so page starts at 1.
  const [page, setPage] = React.useState(1);

  const q = useDeadJobs(queue, page, { pageSize: DEAD_PAGE_SIZE });
  const retry = useRetryDeadJob();
  const purge = usePurgeDeadJobs();
  const [confirmPurge, setConfirmPurge] = React.useState(false);

  const jobs = q.data?.jobs ?? [];
  const total = q.data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / DEAD_PAGE_SIZE));
  const retryingId = retry.isPending ? retry.variables?.task_id : null;

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-3">
        <span className="text-muted-foreground text-xs">
          {total} failed job{total === 1 ? "" : "s"} in {queueLabel(queue)}
        </span>
        <AlertDialog open={confirmPurge} onOpenChange={setConfirmPurge}>
          <AlertDialogTrigger asChild>
            <Button variant="outline" size="sm" disabled={purge.isPending}>
              <Trash2 className="mr-1.5 h-3.5 w-3.5" /> Clear all
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Clear all failed jobs?</AlertDialogTitle>
              <AlertDialogDescription>
                Permanently discards the {total} failed{" "}
                {queueLabel(queue).toLowerCase()} job{total === 1 ? "" : "s"}.
                This can&rsquo;t be undone — retry instead if you want them to
                run again.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel disabled={purge.isPending}>
                Cancel
              </AlertDialogCancel>
              <AlertDialogAction
                onClick={(e) => {
                  e.preventDefault();
                  purge.mutate(
                    { queue },
                    { onSuccess: () => setConfirmPurge(false) },
                  );
                }}
                disabled={purge.isPending}
              >
                {purge.isPending ? "Clearing…" : "Clear all"}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </div>

      {q.isLoading ? (
        <LoaderRow label="Loading…" />
      ) : jobs.length === 0 ? (
        <p className="text-muted-foreground text-sm">
          No failed jobs on this page.
        </p>
      ) : (
        <ul className="space-y-2">
          {jobs.map((job) => (
            <DeadJobRow
              key={job.task_id}
              job={job}
              onRetry={() => retry.mutate({ queue, task_id: job.task_id })}
              retrying={retryingId === job.task_id}
            />
          ))}
        </ul>
      )}

      {totalPages > 1 ? (
        <div className="flex items-center justify-end gap-2">
          <Button
            variant="outline"
            size="sm"
            disabled={page <= 1}
            onClick={() => setPage((p) => Math.max(1, p - 1))}
          >
            <ChevronLeft className="h-3.5 w-3.5" />
          </Button>
          <span className="text-muted-foreground text-xs tabular-nums">
            {page} / {totalPages}
          </span>
          <Button
            variant="outline"
            size="sm"
            disabled={page >= totalPages}
            onClick={() => setPage((p) => p + 1)}
          >
            <ChevronRight className="h-3.5 w-3.5" />
          </Button>
        </div>
      ) : null}
    </div>
  );
}

function DeadJobRow({
  job,
  onRetry,
  retrying,
}: {
  job: DeadJob;
  onRetry: () => void;
  retrying: boolean;
}) {
  const when = job.failed_at
    ? new Date(job.failed_at * 1000).toLocaleString()
    : "—";
  const payload = (job.payload ?? null) as Record<string, unknown> | null;

  return (
    <li className="border-border bg-card rounded-md border">
      <details className="group">
        <summary className="flex cursor-pointer items-center justify-between gap-3 px-3 py-2 text-sm">
          <span className="flex min-w-0 flex-col gap-0.5">
            <span className="text-destructive truncate">
              {job.error ?? "Unknown error"}
            </span>
            <span className="text-muted-foreground truncate text-xs">
              {when} · {job.task_id}
            </span>
          </span>
          <Button
            variant="outline"
            size="sm"
            className="shrink-0"
            disabled={retrying}
            onClick={(e) => {
              // Don't let the click toggle the <details> disclosure.
              e.preventDefault();
              onRetry();
            }}
          >
            <RotateCcw className="mr-1.5 h-3.5 w-3.5" />
            {retrying ? "Retrying…" : "Retry"}
          </Button>
        </summary>
        {payload ? (
          <div className="border-border border-t px-3 py-2">
            <pre className="bg-muted/40 overflow-x-auto rounded p-2 text-xs">
              {JSON.stringify(payload, null, 2)}
            </pre>
          </div>
        ) : null}
      </details>
    </li>
  );
}
