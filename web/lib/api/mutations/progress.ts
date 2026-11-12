/**
 * Optimistic mark read / unread (folio-flair-1.0 interaction polish).
 *
 * Marking an issue read or unread writes the new state into every cache
 * that draws it *before* the request goes out, so the finished badge and
 * the `ProgressFill` bar move on the click instead of after a refetch:
 *
 *   - the shared `/progress` map (`queryKeys.userProgress`) behind every
 *     `IssueCard`, `CblIssueCard` and series card;
 *   - the Continue Reading rail (`queryKeys.continueReading`), whose
 *     cards carry their own `progress.percent`;
 *   - a per-issue record (`issueProgressKey`) the server-rendered issue
 *     page reads its live read state from (it never loads the whole map).
 *
 * The caches roll back on error. On success the mutation hooks raise an
 * Undo toast that puts each issue back the way it was
 * ([`restoreProgress`]).
 */
import type { QueryClient, QueryKey } from "@tanstack/react-query";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import type { ReadState } from "@/lib/reading-state";
import { readStateFor } from "@/lib/reading-state";

import { jsonFetch, queryKeys } from "../queries";
import type { ContinueReadingView, ProgressView } from "../types";
import { apiMutate } from "./_core";
import {
  invalidateIssueCards,
  invalidateRails,
  invalidateSeriesProgress,
} from "./rails";
import { apiPath } from "../path";

export type ProgressMap = Map<string, ProgressView>;

/** One issue's progress before a mark: the stored record, or `null` when
 *  there was none (never opened). */
export type PriorProgress = Map<string, ProgressView | null>;

/** Live per-issue progress record (`queryKeys.issueProgress`) for
 *  surfaces that don't load the whole `/progress` map: the issue page and
 *  every issue card. Written by the optimistic mark, the mark's response
 *  and the reader's write replies; empty otherwise. */
export function issueProgressKey(issueId: string): QueryKey {
  return queryKeys.issueProgress(issueId);
}

export type ProgressWrite = {
  issue_id: string;
  finished: boolean;
  page: number;
};

type Snapshot = {
  map: ProgressMap | undefined;
  continueReading: ContinueReadingView | undefined;
  issues: Array<[string, ProgressView | null | undefined]>;
};

function nextRecord(
  prev: ProgressView | null | undefined,
  w: ProgressWrite,
): ProgressView {
  return {
    run: prev?.run ?? 0,
    issue_id: w.issue_id,
    page: w.page,
    finished: w.finished,
    // A mark is all-or-nothing: read = full bar, unread = empty. A
    // restore to a mid-issue page keeps the bar it had (if known).
    percent: w.finished ? 1 : w.page <= 0 ? 0 : (prev?.percent ?? 0),
    updated_at: new Date().toISOString(),
  };
}

/** Write `writes` into every progress cache; returns what to roll back
 *  to. Cancels in-flight fetches first so a slow refetch can't land on
 *  top of the optimistic state. */
export async function applyOptimisticProgress(
  qc: QueryClient,
  writes: readonly ProgressWrite[],
): Promise<Snapshot> {
  await Promise.all([
    qc.cancelQueries({ queryKey: queryKeys.userProgress }),
    qc.cancelQueries({ queryKey: queryKeys.continueReading }),
  ]);
  const map = qc.getQueryData<ProgressMap>(queryKeys.userProgress);
  const continueReading = qc.getQueryData<ContinueReadingView>(
    queryKeys.continueReading,
  );
  const issues: Snapshot["issues"] = writes.map((w) => [
    w.issue_id,
    qc.getQueryData<ProgressView | null>(issueProgressKey(w.issue_id)),
  ]);

  // Only patch a map that's already loaded: seeding a partial one would
  // read as "everything else unread" to every card for its stale time.
  if (map) {
    const next = new Map(map);
    for (const w of writes)
      next.set(w.issue_id, nextRecord(map.get(w.issue_id), w));
    qc.setQueryData<ProgressMap>(queryKeys.userProgress, next);
  }
  if (continueReading) {
    const byId = new Map(writes.map((w) => [w.issue_id, w]));
    qc.setQueryData<ContinueReadingView>(queryKeys.continueReading, {
      ...continueReading,
      items: continueReading.items.map((card) => {
        const w = byId.get(card.issue.id);
        if (!w) return card;
        return {
          ...card,
          progress: {
            ...card.progress,
            last_page: w.page,
            percent: w.finished ? 1 : w.page <= 0 ? 0 : card.progress.percent,
          },
        };
      }),
    });
  }
  for (const w of writes) {
    const prev =
      qc.getQueryData<ProgressView | null>(issueProgressKey(w.issue_id)) ??
      map?.get(w.issue_id);
    qc.setQueryData(issueProgressKey(w.issue_id), nextRecord(prev, w));
  }
  return { map, continueReading, issues };
}

/** Undo [`applyOptimisticProgress`] after a failed write. */
export function rollbackProgress(qc: QueryClient, snap: Snapshot | undefined) {
  if (!snap) return;
  if (snap.map) qc.setQueryData(queryKeys.userProgress, snap.map);
  if (snap.continueReading) {
    qc.setQueryData(queryKeys.continueReading, snap.continueReading);
  }
  for (const [id, rec] of snap.issues) {
    if (rec === undefined) qc.removeQueries({ queryKey: issueProgressKey(id) });
    else qc.setQueryData(issueProgressKey(id), rec);
  }
}

/** Each issue's record before the mark, for Undo. Read from the caches
 *  when they know the issue; a loaded `/progress` map that lacks it means
 *  "no record". The issue page has neither, so it asks the server for
 *  that one record (`?issue_id=`, one row). Must run *before* the
 *  optimistic write. */
export async function capturePriorProgress(
  qc: QueryClient,
  issueIds: readonly string[],
): Promise<PriorProgress> {
  const map = qc.getQueryData<ProgressMap>(queryKeys.userProgress);
  const prior: PriorProgress = new Map();
  const missing: string[] = [];
  for (const id of issueIds) {
    const own = qc.getQueryData<ProgressView | null>(issueProgressKey(id));
    if (own !== undefined) prior.set(id, own);
    else if (map) prior.set(id, map.get(id) ?? null);
    else missing.push(id);
  }
  if (missing.length === 1) {
    const id = missing[0]!;
    const resp = await jsonFetch<{ records: ProgressView[] }>(
      apiPath`/progress?issue_id=${id}`,
    );
    prior.set(id, resp.records.find((r) => r.issue_id === id) ?? null);
  } else if (missing.length > 1) {
    const resp = await jsonFetch<{ records: ProgressView[] }>("/progress");
    const all = new Map(resp.records.map((r) => [r.issue_id, r]));
    for (const id of missing) prior.set(id, all.get(id) ?? null);
  }
  return prior;
}

/** The write that puts `rec` back, as an optimistic cache write. */
function writeFor(id: string, rec: ProgressView | null): ProgressWrite {
  if (!rec || (!rec.finished && rec.page <= 0)) {
    return { issue_id: id, page: 0, finished: false };
  }
  return { issue_id: id, page: rec.page, finished: rec.finished };
}

/**
 * Put every issue in `prior` back to its recorded state:
 *
 *   - never opened / unread → one bulk "unread" write;
 *   - mid-issue → its page, one write each (rare: a mark from the middle);
 *   - finished → one bulk "read" write with `backfill: true`. The server
 *     re-stamps `finished_at` on a false → true flip and has no way to
 *     set it back, so the restore is flagged as catalog maintenance
 *     rather than counted as a finish today.
 */
export async function restoreProgress(
  qc: QueryClient,
  prior: PriorProgress,
): Promise<void> {
  const unread: string[] = [];
  const finished: string[] = [];
  const midway: ProgressWrite[] = [];
  for (const [id, rec] of prior) {
    const w = writeFor(id, rec);
    if (w.finished) finished.push(id);
    else if (w.page <= 0) unread.push(id);
    else midway.push(w);
  }
  await applyOptimisticProgress(
    qc,
    [...prior].map(([id, rec]) => writeFor(id, rec)),
  );
  let ok = false;
  try {
    if (unread.length > 0) {
      await apiMutate({
        path: "/me/progress/bulk",
        method: "POST",
        body: { issue_ids: unread, finished: false },
      });
    }
    if (finished.length > 0) {
      await apiMutate({
        path: "/me/progress/bulk",
        method: "POST",
        body: { issue_ids: finished, finished: true, backfill: true },
      });
    }
    for (const w of midway) {
      await apiMutate({ path: "/progress", method: "POST", body: w });
    }
    ok = true;
  } finally {
    // On success the optimistic restore already matches the server; the
    // per-issue records stay so the issue page keeps showing it. On
    // failure they go, and the refetch brings back the server's truth.
    if (!ok) {
      for (const id of prior.keys()) {
        qc.removeQueries({ queryKey: issueProgressKey(id) });
      }
    }
    qc.invalidateQueries({ queryKey: queryKeys.userProgress });
    invalidateRails(qc);
    invalidateSeriesProgress(qc);
    invalidateIssueCards(qc);
  }
}

/**
 * The issue page's read state, live: the per-issue record once a mark
 * (or its Undo) has touched it this session, else the server-rendered
 * `fallback`. Observes the cache only — never fetches.
 */
export function useIssueReadState(
  issue: { id: string; page_count?: number | null },
  fallback: ReadState,
): ReadState {
  const qc = useQueryClient();
  const key = issueProgressKey(issue.id);
  const { data } = useQuery({
    queryKey: key,
    queryFn: () => qc.getQueryData<ProgressView | null>(key) ?? null,
    enabled: false,
  });
  return data === undefined ? fallback : readStateFor(issue, data);
}
