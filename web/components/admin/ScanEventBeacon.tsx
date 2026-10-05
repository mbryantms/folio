"use client";

import { useMemo, useState } from "react";
import Link from "next/link";

import { Activity, ListOrdered, Trash2 } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { useClearQueue } from "@/lib/api/mutations";
import { useScanEvents } from "@/lib/api/scan-events";
import { useQueueDepth } from "@/lib/api/queries";
import {
  BACKGROUND_WORK_HREF,
  compactCount,
  pillBreakdown,
} from "@/lib/admin/background-work";
import { statusTone } from "@/lib/ui/status-tone";
import { cn } from "@/lib/utils";

/**
 * A single subscriber that lives at the admin layout level. It opens *one*
 * WebSocket for the whole admin tree, polls the apalis queue depth on a
 * steady cadence, and renders two small pills in the topbar:
 *   - WS status (connecting / scan active / closed)
 *   - Queue depth (only when total > 0, so the topbar stays quiet at idle)
 *
 * Both link to the Background work page — the one surface that shows what
 * the number is made of, across every library and job type.
 *
 * The queue pill matters operationally: it makes "draining N stale jobs"
 * legible at a glance, instead of an invisible reason for sluggishness.
 */
export function ScanEventBeacon() {
  const { status, events } = useScanEvents({ toastErrors: true });
  const queue = useQueueDepth();
  const clearQueue = useClearQueue();
  const [confirmClear, setConfirmClear] = useState(false);
  const tone =
    status === "open"
      ? statusTone("success")
      : status === "connecting"
        ? statusTone("warning")
        : "border-border text-muted-foreground";

  const total = queue.data?.total ?? 0;
  const hasActiveScan = useMemo(() => activeScanCount(events) > 0, [events]);
  const showStreamPill = status !== "open" || hasActiveScan;
  const queueTone =
    total === 0 ? "border-border text-muted-foreground" : statusTone("warning");
  const breakdown = queue.data ? pillBreakdown(queue.data) : "";

  return (
    <div className="flex items-center gap-2">
      {total > 0 ? (
        <div className="border-warning/30 inline-flex overflow-hidden rounded-full border">
          <Link
            href={BACKGROUND_WORK_HREF}
            className={cn(
              "hover:bg-warning/10 hover:text-warning inline-flex items-center gap-1.5 px-2 py-0.5 text-[10px] font-semibold tracking-wider uppercase transition-colors",
              queueTone,
            )}
            aria-label={`Background work: ${total} jobs pending${breakdown ? ` (${breakdown})` : ""}. Open background work.`}
            title={breakdown || `${total} pending jobs`}
          >
            <ListOrdered className="h-3 w-3" />
            queue: {compactCount(total)}
          </Link>
          <button
            type="button"
            // Wider tap area than the original 12px icon box, kept in
            // proportion to the compact topbar pill (a literal 44px target
            // would break the pill — see the search-pill / kebab feedback).
            className={cn(
              "border-warning/30 text-warning hover:bg-warning/10 hover:text-warning inline-flex items-center justify-center border-l px-2.5 py-1 transition-colors disabled:opacity-50",
              clearQueue.isPending && "cursor-wait",
            )}
            aria-label="Clear all pending background queues"
            title="Clear all pending background queues"
            disabled={clearQueue.isPending}
            onClick={() => setConfirmClear(true)}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </button>
        </div>
      ) : null}
      {showStreamPill ? (
        <Link
          href={BACKGROUND_WORK_HREF}
          className={cn(
            "inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-[10px] font-semibold tracking-wider uppercase",
            tone,
          )}
          aria-label={`Scan event stream ${status}. Open background work.`}
        >
          <Activity
            className={cn(
              "h-3 w-3",
              status === "open" ? "animate-pulse" : "opacity-60",
            )}
          />
          {status === "open" ? "scan active" : status}
        </Link>
      ) : null}

      <AlertDialog open={confirmClear} onOpenChange={setConfirmClear}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Clear pending queues?</AlertDialogTitle>
            <AlertDialogDescription>
              Remove pending work by queue type. A job that is already executing
              may still finish and report its normal events.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {/* Four buttons don't fit cleanly on one row at typical dialog
              widths — "Clear thumbnails" wraps mid-label. Force a
              two-column grid so each button gets equal width and labels
              don't break. The destructive "Clear all" remains visually
              distinct via the bg-destructive override. */}
          <div className="grid grid-cols-2 gap-2 pt-2 sm:grid-cols-4">
            <AlertDialogCancel disabled={clearQueue.isPending} className="m-0">
              Cancel
            </AlertDialogCancel>
            <Button
              type="button"
              variant="outline"
              disabled={clearQueue.isPending}
              onClick={() =>
                clearQueue.mutate(
                  { target: "scans" },
                  { onSettled: () => setConfirmClear(false) },
                )
              }
            >
              Scans only
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={clearQueue.isPending}
              onClick={() =>
                clearQueue.mutate(
                  { target: "thumbnails" },
                  { onSettled: () => setConfirmClear(false) },
                )
              }
            >
              Thumbnails only
            </Button>
            <AlertDialogAction
              disabled={clearQueue.isPending}
              onClick={() =>
                clearQueue.mutate(
                  { target: "all" },
                  { onSettled: () => setConfirmClear(false) },
                )
              }
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90 m-0"
            >
              {clearQueue.isPending ? "Clearing…" : "Clear all"}
            </AlertDialogAction>
          </div>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function activeScanCount(events: ReturnType<typeof useScanEvents>["events"]) {
  const active = new Set<string>();
  for (const event of events) {
    if (event.type === "scan.started") active.add(event.scan_id);
    if (event.type === "scan.progress") {
      if (event.phase === "complete") active.delete(event.scan_id);
      else active.add(event.scan_id);
    }
    if (event.type === "scan.completed" || event.type === "scan.failed") {
      active.delete(event.scan_id);
    }
  }
  return active.size;
}
