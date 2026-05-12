"use client";

import { useMemo, useState } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";

import { Activity, ListOrdered, Trash2 } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { useClearQueue } from "@/lib/api/mutations";
import { useScanEvents } from "@/lib/api/scan-events";
import { useLibraryList, useQueueDepth } from "@/lib/api/queries";
import { cn } from "@/lib/utils";

/**
 * A single subscriber that lives at the admin layout level. It opens *one*
 * WebSocket for the whole admin tree, polls the apalis queue depth on a
 * steady cadence, and renders two small pills in the topbar:
 *   - WS status (connecting / scans live / closed)
 *   - Queue depth (only when total > 0, so the topbar stays quiet at idle)
 *
 * The queue pill matters operationally: it makes "draining N stale jobs"
 * legible at a glance, instead of an invisible reason for sluggishness.
 */
export function ScanEventBeacon() {
  const pathname = usePathname() ?? "";
  const { status, events } = useScanEvents({ toastErrors: true });
  const queue = useQueueDepth();
  const libraryList = useLibraryList();
  const clearQueue = useClearQueue();
  const [confirmClear, setConfirmClear] = useState(false);
  const tone =
    status === "open"
      ? "border-emerald-800/50 bg-emerald-950/40 text-emerald-300"
      : status === "connecting"
        ? "border-amber-800/40 bg-amber-950/30 text-amber-300"
        : "border-border text-muted-foreground";

  const total = queue.data?.total ?? 0;
  const hasActiveScan = useMemo(() => activeScanCount(events) > 0, [events]);
  const showStreamPill = status !== "open" || hasActiveScan;
  // Prefer the live-scan page of whichever library is currently being
  // scanned (derived from in-flight scan events). Falls back to the
  // library-context in the URL path, then to the admin library list.
  // Surfaces the "go straight to the active scan" affordance the user
  // wants without forcing them to remember which library is scanning.
  const liveScanHref = useMemo(
    () => liveScanHrefForEvents(events, libraryList.data, pathname),
    [events, libraryList.data, pathname],
  );
  const queueTone =
    total === 0
      ? "border-border text-muted-foreground"
      : total < 25
        ? "border-amber-800/40 bg-amber-950/30 text-amber-300"
        : "border-orange-800/50 bg-orange-950/40 text-orange-300";

  return (
    <div className="flex items-center gap-2">
      {total > 0 ? (
        <div className="inline-flex overflow-hidden rounded-full border border-amber-800/40">
          <Link
            href={liveScanHref}
            className={cn(
              "inline-flex items-center gap-1.5 px-2 py-0.5 text-[10px] font-semibold tracking-wider uppercase transition-colors hover:bg-amber-900/30 hover:text-amber-100",
              queueTone,
            )}
            aria-label={`Job queue: ${total} pending. Open live scan.`}
            title={
              queue.data
                ? `Open live scan · scan ${queue.data.scan} · scan_series ${queue.data.scan_series} · thumbs ${queue.data.post_scan_thumbs} · search ${queue.data.post_scan_search} · dictionary ${queue.data.post_scan_dictionary}`
                : `${total} pending jobs`
            }
          >
            <ListOrdered className="h-3 w-3" />
            queue: {total}
          </Link>
          <button
            type="button"
            className={cn(
              "inline-flex items-center border-l border-amber-800/40 px-1.5 py-0.5 text-amber-300 transition-colors hover:bg-amber-900/30 hover:text-amber-100 disabled:opacity-50",
              clearQueue.isPending && "cursor-wait",
            )}
            aria-label="Clear all pending background queues"
            title="Clear all pending background queues"
            disabled={clearQueue.isPending}
            onClick={() => setConfirmClear(true)}
          >
            <Trash2 className="h-3 w-3" />
          </button>
        </div>
      ) : null}
      {showStreamPill ? (
        <span
          className={cn(
            "inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-[10px] font-semibold tracking-wider uppercase",
            tone,
          )}
          aria-label={`Scan event stream ${status}`}
        >
          <Activity
            className={cn(
              "h-3 w-3",
              status === "open" ? "animate-pulse" : "opacity-60",
            )}
          />
          {status === "open" ? "scan active" : status}
        </span>
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
          <AlertDialogFooter>
            <AlertDialogCancel disabled={clearQueue.isPending}>
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
              Clear scans
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
              Clear thumbnails
            </Button>
            <AlertDialogAction
              disabled={clearQueue.isPending}
              onClick={() =>
                clearQueue.mutate(
                  { target: "all" },
                  { onSettled: () => setConfirmClear(false) },
                )
              }
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              {clearQueue.isPending ? "Clearing..." : "Clear queues"}
            </AlertDialogAction>
          </AlertDialogFooter>
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

function liveScanHrefForPath(pathname: string): string {
  const match = pathname.match(/\/admin\/libraries\/([^/]+)/);
  return match?.[1] ? `/admin/libraries/${match[1]}/scan` : "/admin/libraries";
}

/**
 * Derive the best live-scan URL for the current state. Priority:
 *   1. An active scan in the event stream → that library's scan page
 *      (so the queue pill takes the user straight to the scan that's
 *      generating the queue depth).
 *   2. A library context already in the URL → that library's scan page
 *      (covers the case where the user is already on the relevant
 *      library tree but no events have arrived yet).
 *   3. Fallback to the admin library list.
 */
function liveScanHrefForEvents(
  events: ReturnType<typeof useScanEvents>["events"],
  libraries: { id: string; slug: string }[] | undefined,
  pathname: string,
): string {
  const activeLibraryId = mostRecentActiveLibraryId(events);
  if (activeLibraryId && libraries) {
    const lib = libraries.find((l) => l.id === activeLibraryId);
    if (lib) return `/admin/libraries/${lib.slug}/scan`;
  }
  return liveScanHrefForPath(pathname);
}

/** Walk the events stream newest-first; return the library_id of the
 *  most recent active scan (one we've seen `started` or `progress` for
 *  but no terminal `completed` / `failed`). */
function mostRecentActiveLibraryId(
  events: ReturnType<typeof useScanEvents>["events"],
): string | null {
  const terminated = new Set<string>();
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i];
    if (event.type === "scan.completed" || event.type === "scan.failed") {
      terminated.add(event.scan_id);
      continue;
    }
    if (event.type === "scan.started" || event.type === "scan.progress") {
      if (event.type === "scan.progress" && event.phase === "complete") {
        terminated.add(event.scan_id);
        continue;
      }
      if (!terminated.has(event.scan_id)) {
        return event.library_id;
      }
    }
  }
  return null;
}
