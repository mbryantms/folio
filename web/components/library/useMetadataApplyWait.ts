"use client";

import * as React from "react";
import { toast } from "sonner";

import { useScanEvents } from "@/lib/api/scan-events";

/**
 * Post-apply waiting machinery for the metadata match dialog (H4 extract).
 *
 * The apply API returns 202 — the rows aren't current until the job runs.
 * Rather than detect the library's writeback mode client-side (it decides
 * *which* completion event fires, not *whether* to wait), the dialog always
 * enters a waiting state on apply success and resolves once it sees a
 * completion event for this library:
 *   - writeback path → the scoped rescan's `scan.completed`
 *   - DB-direct path → `metadata.applied` (emitted by the apply job)
 * Either way it re-hydrates first, so an open Covers/Notes tab updates
 * without a page refresh. A 30s timeout is the fallback for a missed event.
 *
 * Takes primitives (not the dialog's `MetadataMatchScope`) so it carries no
 * dependency back on the dialog module.
 */
export function useMetadataApplyWait({
  applyDidSucceed,
  libraryId,
  isSeriesScope,
  rehydrate,
  onApplied,
  onClose,
}: {
  /** `apply.isSuccess || compositeApply.isSuccess` from the caller. */
  applyDidSucceed: boolean;
  libraryId: string;
  /** Series-scope rescans get a progress chip; issue rescans are too short. */
  isSeriesScope: boolean;
  /** Pull the freshly-written data into the page before resolving. */
  rehydrate: () => void;
  /** Worklist controller advances to the next item; standalone closes. */
  onApplied?: () => void;
  onClose: () => void;
}): {
  waitingForRescan: boolean;
  seriesProgress: { done: number; total: number } | null;
} {
  const [applyAt, setApplyAt] = React.useState<number | null>(null);
  React.useEffect(() => {
    if (!applyDidSucceed) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- legitimate transition: apply success triggers the wait-for-completion state.
    setApplyAt(Date.now());
  }, [applyDidSucceed]);

  // Subscribe to the library's scan events only when waiting. The existing
  // `useScanEvents` hook auto-reconnects + filters by libraryId server-side,
  // and tolerates re-subscribes (module-level ticket dedupe).
  const waitingForRescan = applyAt !== null;
  const scanEvents = useScanEvents({
    libraryId: waitingForRescan ? libraryId : undefined,
    // The dialog already owns "Apply succeeded" feedback; don't toast.
    toastCompletions: false,
    toastErrors: false,
  });
  // Watch the events buffer for a completion event for this library. The
  // subscription only starts when `applyAt` is set, so any completed event
  // in the buffer is by definition post-apply — no timestamp filtering needed.
  React.useEffect(() => {
    if (!waitingForRescan) return;
    const completed = scanEvents.events.find(
      (e) => e.type === "scan.completed" || e.type === "metadata.applied",
    );
    if (completed) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- WS event arrival is the trigger; resetting `applyAt` to null also tears down the subscription on the next render.
      setApplyAt(null);
      rehydrate();
      if (onApplied) onApplied();
      else onClose();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scanEvents.events, waitingForRescan]);

  // 30s timeout fallback — resolve anyway with an info toast so the user
  // isn't stuck if the WS missed the event (rare; broadcast lag, reconnect).
  // The rewrite has already landed; the data refreshes on next navigation.
  React.useEffect(() => {
    if (!waitingForRescan) return;
    const t = setTimeout(() => {
      setApplyAt(null);
      rehydrate();
      toast.info(
        "Refreshing — reopen the page if the latest data isn't shown yet.",
      );
      if (onApplied) onApplied();
      else onClose();
    }, 30_000);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [waitingForRescan]);

  // Series-scope progress chip — derived from the latest `scan.progress`.
  // Computed inline (no useMemo) so the react-compiler's preservation
  // analysis doesn't have to bend around a `for`-loop with a return inside.
  let seriesProgress: { done: number; total: number } | null = null;
  if (waitingForRescan && isSeriesScope) {
    for (let i = scanEvents.events.length - 1; i >= 0; i--) {
      const e = scanEvents.events[i];
      if (e === undefined) continue;
      if (e.type === "scan.progress" && e.series_total > 0) {
        seriesProgress = { done: e.series_scanned, total: e.series_total };
        break;
      }
    }
  }

  return { waitingForRescan, seriesProgress };
}
