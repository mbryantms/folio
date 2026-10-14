"use client";

import * as React from "react";

import { useScanEvents } from "@/lib/api/scan-events";
import type { ScanEvent } from "@/lib/api/types";

/** What the apply response told us about the work it started. */
export type ApplyTarget = {
  runId: string;
  seriesId: string;
  issueId: string | null;
  /** The apply already landed synchronously (DB-direct composite apply):
   *  nothing to wait for. */
  settled: boolean;
};

/** How long we wait before admitting the completion is overdue. */
export const APPLY_WAIT_OVERDUE_MS = 30_000;

/**
 * Does this event finish `target`? Completion arrives one of two ways:
 *  - DB-direct apply → `metadata.applied` carrying the run id (older
 *    servers omit it; then the entity ids must match);
 *  - writeback apply → the scoped rescan's `scan.completed`, matched on
 *    its `series_id` / `issue_id` scope (a library-wide scan or another
 *    series' rescan in the same library is *not* ours).
 * Exported for unit tests.
 */
export function eventCompletesApply(e: ScanEvent, t: ApplyTarget): boolean {
  if (e.type === "metadata.applied") {
    if (e.run_id) return e.run_id === t.runId;
    if (e.series_id !== t.seriesId) return false;
    return t.issueId ? e.issue_id === t.issueId : !e.issue_id;
  }
  if (e.type === "scan.completed") {
    if (!e.series_id || e.series_id !== t.seriesId) return false;
    // An issue apply waits for its own issue rescan; a series apply for a
    // series-scoped rescan (an issue rescan inside it is not the whole job).
    return t.issueId ? e.issue_id === t.issueId : !e.issue_id;
  }
  return false;
}

/** Does this event report `target`'s apply as failed? */
export function eventFailsApply(e: ScanEvent, t: ApplyTarget): boolean {
  return e.type === "metadata.failed" && e.run_id === t.runId;
}

/**
 * Post-apply waiting machinery for the metadata match dialog (H4 extract,
 * reworked in post-audit T1.6 so the wait is correlated to *this* apply).
 *
 * The apply API returns 202 — the rows aren't current until the job runs.
 * The dialog enters a waiting state on apply success and resolves once it
 * sees the completion event for its run / scoped rescan (see
 * `eventCompletesApply`); it used to resolve on *any* completion in the
 * library, so a neighbour's rescan (or an unrelated scan) ended the wait
 * early and the worklist counted it as applied.
 *
 * Outcomes:
 *  - matching completion → `rehydrate`, then `onApplied` (worklist
 *    advances) or `onClose`;
 *  - `metadata.failed` for the run → `failure` is set, nothing advances;
 *  - no event within `APPLY_WAIT_OVERDUE_MS` → `overdue` is set and the
 *    dialog stays open, still listening (decision D5a). The worklist does
 *    not advance; the user can keep waiting, open Background work, or
 *    close — the dialog then reports the item as pending, not applied.
 *  - `target.settled` (synchronous composite apply) → resolves at once.
 *
 * Takes primitives (not the dialog's `MetadataMatchScope`) so it carries no
 * dependency back on the dialog module.
 */
export function useMetadataApplyWait({
  target,
  libraryId,
  isSeriesScope,
  rehydrate,
  onApplied,
  onClose,
}: {
  /** From the apply / composite-apply response; `null` until one succeeds. */
  target: ApplyTarget | null;
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
  /** The completion is overdue; still listening. */
  overdue: boolean;
  /** The apply job reported failure (the message). */
  failure: string | null;
  seriesProgress: { done: number; total: number } | null;
} {
  // One state object so each effect below makes a single transition.
  const [wait, setWait] = React.useState<{
    target: ApplyTarget | null;
    overdue: boolean;
    failure: string | null;
  }>({ target: null, overdue: false, failure: null });
  const waitingFor = wait.target;
  const resolvedRef = React.useRef<string | null>(null);

  const resolve = React.useCallback(() => {
    setWait((s) => ({ ...s, target: null, overdue: false }));
    rehydrate();
    if (onApplied) onApplied();
    else onClose();
  }, [rehydrate, onApplied, onClose]);

  // A new apply target starts a wait (or resolves immediately when the
  // server says the work already landed).
  React.useEffect(() => {
    if (!target || resolvedRef.current === target.runId) return;
    resolvedRef.current = target.runId;
    if (target.settled) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- the server said the apply already landed; resolving is the transition.
      resolve();
      return;
    }
    setWait({ target, overdue: false, failure: null });
  }, [target, resolve]);

  // Subscribe to the library's scan events only while waiting. The hook
  // auto-reconnects + filters by libraryId server-side, and tolerates
  // re-subscribes (module-level ticket dedupe).
  const waitingForRescan = waitingFor !== null;
  const scanEvents = useScanEvents({
    libraryId: waitingForRescan ? libraryId : undefined,
    // The dialog already owns "Apply succeeded" feedback; don't toast.
    toastCompletions: false,
    toastErrors: false,
  });

  React.useEffect(() => {
    if (!waitingFor) return;
    const failed = scanEvents.events.find((e) =>
      eventFailsApply(e, waitingFor),
    );
    if (failed && failed.type === "metadata.failed") {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- WS event arrival is the trigger.
      setWait({ target: null, overdue: false, failure: failed.error });
      return;
    }
    if (scanEvents.events.some((e) => eventCompletesApply(e, waitingFor))) {
      resolve();
    }
  }, [scanEvents.events, waitingFor, resolve]);

  // Overdue marker (D5a): say so, keep listening, never claim success.
  React.useEffect(() => {
    if (!waitingForRescan) return;
    const t = setTimeout(
      () => setWait((s) => (s.target ? { ...s, overdue: true } : s)),
      APPLY_WAIT_OVERDUE_MS,
    );
    return () => clearTimeout(t);
  }, [waitingForRescan]);

  // Series-scope progress chip — derived from the latest `scan.progress`
  // for our series. Computed inline (no useMemo) so the react-compiler's
  // preservation analysis doesn't have to bend around a `for`-loop with a
  // return inside.
  let seriesProgress: { done: number; total: number } | null = null;
  if (waitingFor && isSeriesScope) {
    for (let i = scanEvents.events.length - 1; i >= 0; i--) {
      const e = scanEvents.events[i];
      if (e === undefined) continue;
      if (e.type === "scan.progress" && e.series_total > 0) {
        seriesProgress = { done: e.series_scanned, total: e.series_total };
        break;
      }
    }
  }

  return {
    waitingForRescan,
    overdue: wait.overdue,
    failure: wait.failure,
    seriesProgress,
  };
}
