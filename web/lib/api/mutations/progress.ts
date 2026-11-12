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
 *   - the progress overlay (`../progress-overlay.ts`) every issue card and
 *     the server-rendered issue page read their live state from (neither
 *     loads the whole map). An optimistic entry stays pending until the
 *     server acknowledges it ([`settleProgress`]).
 *
 * The caches roll back on error. On success the mutation hooks raise an
 * Undo toast that puts each issue back the way it was
 * ([`restoreProgress`]).
 */
import type { QueryClient } from "@tanstack/react-query";
import { useQueryClient } from "@tanstack/react-query";

import type { ReadState } from "@/lib/reading-state";
import { readStateFor } from "@/lib/reading-state";

import { jsonFetch, queryKeys } from "../queries";
import type { ContinueReadingView, ProgressView } from "../types";
import {
  cachedCardProgress,
  progressOverlay,
  useOverlayEntry,
  type OverlayEntry,
  type OverlayRecord,
} from "../progress-overlay";
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
export type PriorProgress = Map<string, OverlayRecord | null>;

export type ProgressWrite = {
  issue_id: string;
  finished: boolean;
  page: number;
  /** Known fraction read (an Undo putting a mid-issue record back);
   *  cache-only, never sent. */
  percent?: number;
};

type Snapshot = {
  map: ProgressMap | undefined;
  continueReading: ContinueReadingView | undefined;
  /** Each issue's overlay entry before the write (`undefined` = none). */
  issues: Array<[string, OverlayEntry | undefined]>;
};

function nextRecord(
  prev: (OverlayRecord & { run?: number }) | null | undefined,
  w: ProgressWrite,
): ProgressView {
  return {
    run: prev?.run ?? 0,
    issue_id: w.issue_id,
    page: w.page,
    finished: w.finished,
    // A mark is all-or-nothing: read = full bar, unread = empty. A
    // restore to a mid-issue page keeps the bar it had (if known).
    percent: w.finished
      ? 1
      : w.page <= 0
        ? 0
        : (w.percent ?? prev?.percent ?? 0),
    // Display only; nothing orders by it (the overlay orders by when
    // the tab saw a copy, never by clock).
    updated_at: new Date().toISOString(),
  };
}

/** Write `writes` into every progress cache; returns what to roll back
 *  to. The overlay entries are pending until [`settleProgress`]. Cancels
 *  in-flight fetches first so a slow refetch can't land on top of the
 *  optimistic state. */
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
  const overlay = progressOverlay(qc);

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
  const issues: Snapshot["issues"] = writes.map((w) => {
    const prev = overlay.get(w.issue_id)?.record ?? map?.get(w.issue_id);
    return [w.issue_id, overlay.write(w.issue_id, nextRecord(prev, w), true)];
  });
  return { map, continueReading, issues };
}

/** The server acknowledged the writes for `ids`; `records` are its
 *  replies where it sent them (a single mark's `ProgressView`). */
export function settleProgress(
  qc: QueryClient,
  ids: Iterable<string>,
  records?: Map<string, OverlayRecord>,
) {
  progressOverlay(qc).settle(ids, records);
}

/** Undo [`applyOptimisticProgress`] after a failed write. Every mounted
 *  card of those issues re-renders with what it showed before. */
export function rollbackProgress(qc: QueryClient, snap: Snapshot | undefined) {
  if (!snap) return;
  if (snap.map) qc.setQueryData(queryKeys.userProgress, snap.map);
  if (snap.continueReading) {
    qc.setQueryData(queryKeys.continueReading, snap.continueReading);
  }
  const overlay = progressOverlay(qc);
  for (const [id, prev] of snap.issues) overlay.restore(id, prev);
}

/** Per-id reads (`?issue_id=`, one row each) cover at most this many
 *  issues no cache knows; past it the full list is one request. */
const PRIOR_PER_ID_MAX = 8;

/** Each issue's record before the mark, for Undo. Taken from what the tab
 *  already knows: its own overlay entries, then the progress embedded on
 *  the cards in the card-list caches (`null` there is a known "no
 *  record"), then a loaded `/progress` map. Only issues none of them know
 *  go to the network. Must run *before* the optimistic write. */
export async function capturePriorProgress(
  qc: QueryClient,
  issueIds: readonly string[],
): Promise<PriorProgress> {
  const overlay = progressOverlay(qc);
  const prior: PriorProgress = new Map();
  let unknown: string[] = [];
  for (const id of issueIds) {
    const own = overlay.get(id);
    if (own) prior.set(id, own.record);
    else unknown.push(id);
  }
  if (unknown.length > 0) {
    const cards = cachedCardProgress(qc, unknown);
    const map = qc.getQueryData<ProgressMap>(queryKeys.userProgress);
    unknown = unknown.filter((id) => {
      if (cards.has(id)) prior.set(id, cards.get(id) ?? null);
      else if (map) prior.set(id, map.get(id) ?? null);
      else return true;
      return false;
    });
  }
  if (unknown.length === 0) return prior;
  if (unknown.length <= PRIOR_PER_ID_MAX) {
    const rows = await Promise.all(
      unknown.map((id) =>
        jsonFetch<{ records: ProgressView[] }>(
          apiPath`/progress?issue_id=${id}`,
        ).then((r) => r.records.find((x) => x.issue_id === id) ?? null),
      ),
    );
    unknown.forEach((id, i) => prior.set(id, rows[i] ?? null));
  } else {
    const resp = await jsonFetch<{ records: ProgressView[] }>("/progress");
    const all = new Map(resp.records.map((r) => [r.issue_id, r]));
    for (const id of unknown) prior.set(id, all.get(id) ?? null);
  }
  return prior;
}

/** The write that puts `rec` back, as an optimistic cache write. */
function writeFor(id: string, rec: OverlayRecord | null): ProgressWrite {
  if (!rec || (!rec.finished && rec.page <= 0)) {
    return { issue_id: id, page: 0, finished: false };
  }
  return {
    issue_id: id,
    page: rec.page,
    finished: rec.finished,
    percent: rec.percent,
  };
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
 *
 * On failure the cards go back to the marked state they showed before
 * the Undo (the server still has it), and the lists refetch.
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
  const snap = await applyOptimisticProgress(
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
      await apiMutate({
        path: "/progress",
        method: "POST",
        body: { issue_id: w.issue_id, page: w.page, finished: w.finished },
      });
    }
    ok = true;
  } finally {
    if (ok) settleProgress(qc, prior.keys());
    else rollbackProgress(qc, snap);
    qc.invalidateQueries({ queryKey: queryKeys.userProgress });
    invalidateRails(qc);
    invalidateSeriesProgress(qc);
    // A failed Undo may have applied part of its writes: refetch the
    // mounted lists so their copies (fetched after this) take over.
    invalidateIssueCards(qc, { refetchActive: !ok });
  }
}

/**
 * The issue page's read state, live: this tab's overlay entry once a mark
 * (or its Undo) has touched the issue, else the server-rendered
 * `fallback`. Observes only — never fetches.
 */
export function useIssueReadState(
  issue: { id: string; page_count?: number | null },
  fallback: ReadState,
): ReadState {
  const entry = useOverlayEntry(useQueryClient(), issue.id);
  if (entry === undefined) return fallback;
  return readStateFor(
    issue,
    entry.record && { ...entry.record, issue_id: issue.id },
  );
}
