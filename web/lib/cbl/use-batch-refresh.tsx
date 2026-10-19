"use client";

/**
 * Client-driven batch refresh for CBL reading lists.
 *
 * The server's `POST /me/cbl-lists/refresh-all` is one opaque call — the
 * UI can't show which list is being fetched or let the user stop halfway
 * through forty GitHub round-trips. This runner instead walks the chosen
 * lists **sequentially** through the single-list refresh endpoint, so the
 * dashboard can render a real progress bar, per-row outcomes, and a
 * Cancel that takes effect after the in-flight list. Sequential on
 * purpose: parallel fan-out would hammer the same upstream host and
 * trip its rate limits.
 *
 * Provided via context so the refresh dashboard (which renders the
 * progress) and the grouped board (whose group menus can start an
 * "Update this group" batch) share one runner.
 */

import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { apiMutate, ApiMutationError } from "@/lib/api/mutations/_core";
import { queryKeys } from "@/lib/api/queries";
import type { ImportSummary } from "@/lib/api/types";

export type BatchTarget = { id: string; name: string };

export type BatchRowStatus =
  "queued" | "running" | "done" | "failed" | "skipped";

export type BatchRow = BatchTarget & {
  status: BatchRowStatus;
  summary?: ImportSummary;
  error?: string;
};

export type BatchState = {
  rows: BatchRow[];
  running: boolean;
  startedAt: number | null;
  finishedAt: number | null;
};

const IDLE: BatchState = {
  rows: [],
  running: false,
  startedAt: null,
  finishedAt: null,
};

export type BatchRefresh = {
  state: BatchState;
  /** Start a batch over `targets`. Ignored while one is already running. */
  start: (targets: BatchTarget[]) => void;
  /** Stop after the in-flight list; remaining rows become `skipped`. */
  cancel: () => void;
  /** Drop the finished batch's rows from the panel. */
  clear: () => void;
  /** Number of rows that have finished (any outcome) — the progress numerator. */
  completed: number;
  /** Be told when a batch finishes (with its final rows), however it was
   *  started. Returns the unsubscribe. Subscribe from an effect. */
  subscribe: (listener: (rows: BatchRow[]) => void) => () => void;
};

const BatchRefreshContext = React.createContext<BatchRefresh | null>(null);

export function useBatchRefresh(): BatchRefresh {
  const ctx = React.useContext(BatchRefreshContext);
  if (!ctx) {
    throw new Error(
      "useBatchRefresh must be used inside <BatchRefreshProvider>",
    );
  }
  return ctx;
}

/** Nullable variant for components that may render outside the provider
 *  (the shared ViewCard). */
export function useOptionalBatchRefresh(): BatchRefresh | null {
  return React.useContext(BatchRefreshContext);
}

export function summarizeBatch(rows: readonly BatchRow[]): {
  updated: number;
  unchanged: number;
  failed: number;
  skipped: number;
} {
  let updated = 0;
  let unchanged = 0;
  let failed = 0;
  let skipped = 0;
  for (const r of rows) {
    if (r.status === "failed") failed++;
    else if (r.status === "skipped") skipped++;
    else if (r.status === "done") {
      if (summaryChanged(r.summary)) updated++;
      else unchanged++;
    }
  }
  return { updated, unchanged, failed, skipped };
}

export function summaryChanged(s: ImportSummary | undefined): boolean {
  if (!s) return false;
  return (
    s.upstream_changed ||
    s.added > 0 ||
    s.removed > 0 ||
    s.reordered > 0 ||
    s.rematched > 0
  );
}

export function BatchRefreshProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const qc = useQueryClient();
  const [state, setState] = React.useState<BatchState>(IDLE);
  const cancelRef = React.useRef(false);
  const runningRef = React.useRef(false);
  const listeners = React.useRef(new Set<(rows: BatchRow[]) => void>());

  const subscribe = React.useCallback(
    (listener: (rows: BatchRow[]) => void) => {
      listeners.current.add(listener);
      return () => {
        listeners.current.delete(listener);
      };
    },
    [],
  );

  const setRow = React.useCallback((id: string, patch: Partial<BatchRow>) => {
    setState((s) => ({
      ...s,
      rows: s.rows.map((r) => (r.id === id ? { ...r, ...patch } : r)),
    }));
  }, []);

  const start = React.useCallback(
    (targets: BatchTarget[]) => {
      if (runningRef.current || targets.length === 0) return;
      runningRef.current = true;
      cancelRef.current = false;
      setState({
        rows: targets.map((t) => ({ ...t, status: "queued" })),
        running: true,
        startedAt: Date.now(),
        finishedAt: null,
      });

      void (async () => {
        const outcomes: BatchRow[] = [];
        for (const t of targets) {
          if (cancelRef.current) {
            setRow(t.id, { status: "skipped" });
            outcomes.push({ ...t, status: "skipped" });
            continue;
          }
          setRow(t.id, { status: "running" });
          try {
            const summary = await apiMutate<ImportSummary>({
              path: `/me/cbl-lists/${t.id}/refresh`,
              method: "POST",
            });
            const row: BatchRow = {
              ...t,
              status: "done",
              summary: summary ?? undefined,
            };
            outcomes.push(row);
            setRow(t.id, row);
          } catch (e) {
            const message =
              e instanceof ApiMutationError || e instanceof Error
                ? e.message
                : "Refresh failed";
            const row: BatchRow = { ...t, status: "failed", error: message };
            outcomes.push(row);
            setRow(t.id, row);
          }
          // Each list's own caches move as soon as it lands so an open
          // detail page behind the settings tab doesn't go stale.
          qc.invalidateQueries({ queryKey: queryKeys.cblList(t.id) });
          qc.invalidateQueries({ queryKey: queryKeys.cblRefreshLog(t.id) });
          qc.invalidateQueries({
            queryKey: ["cbl-lists", "entries", t.id],
            exact: false,
          });
        }
        runningRef.current = false;
        setState((s) => ({ ...s, running: false, finishedAt: Date.now() }));
        // The list index carries per-list stats + last_refreshed_at.
        qc.invalidateQueries({ queryKey: queryKeys.cblLists });

        const { updated, unchanged, failed, skipped } =
          summarizeBatch(outcomes);
        const parts: string[] = [];
        if (updated) parts.push(`${updated} updated`);
        if (unchanged) parts.push(`${unchanged} already current`);
        if (failed) parts.push(`${failed} failed`);
        if (skipped) parts.push(`${skipped} skipped`);
        const text = parts.join(" · ") || "Nothing to update";
        if (failed > 0) toast.warning(text);
        else toast.success(text);
        for (const l of listeners.current) l(outcomes);
      })();
    },
    [qc, setRow],
  );

  const cancel = React.useCallback(() => {
    cancelRef.current = true;
  }, []);

  const clear = React.useCallback(() => {
    if (runningRef.current) return;
    setState(IDLE);
  }, []);

  const completed = state.rows.filter(
    (r) =>
      r.status === "done" || r.status === "failed" || r.status === "skipped",
  ).length;

  const value = React.useMemo<BatchRefresh>(
    () => ({ state, start, cancel, clear, completed, subscribe }),
    [state, start, cancel, clear, completed, subscribe],
  );
  return (
    <BatchRefreshContext.Provider value={value}>
      {children}
    </BatchRefreshContext.Provider>
  );
}
