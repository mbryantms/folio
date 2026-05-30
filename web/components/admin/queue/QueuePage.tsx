"use client";

import * as React from "react";
import { Loader2 } from "lucide-react";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs";
import { useAuditLog, useQueueDepth } from "@/lib/api/queries";
import type { AuditEntryView } from "@/lib/api/types";

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
        <TabsTrigger value="archive">Archive operations</TabsTrigger>
      </TabsList>
      <TabsContent value="overview" className="mt-4">
        <QueueOverview />
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
    <li className="rounded-md border">
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
        <div className="border-t px-3 py-2">
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
