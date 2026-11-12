/**
 * Optimistic mark read / unread (folio-flair-1.0 interaction polish).
 *
 * Marking an issue read or unread writes the new state into every cache
 * that draws it *before* the request goes out, so the finished badge and
 * the `ProgressFill` bar move on the click instead of after a refetch:
 *
 *   - the progress embedded on every cached issue card
 *     ([`card-list-cache.ts`](../card-list-cache.ts));
 *   - the shared `/progress` map (`queryKeys.userProgress`), when loaded;
 *   - the Continue Reading rail (`queryKeys.continueReading`), whose
 *     cards carry their own `progress.percent`;
 *   - the issue page's per-issue read state (`queryKeys.issueProgress`).
 *
 * The query cache is the only copy. On error each cache is rolled back
 * per issue, and only where it still holds this write's own record, so a
 * later overlapping write to the same issue is never undone by an earlier
 * one failing. On success the mutation hooks raise an Undo toast that
 * puts each issue back the way it was ([`restoreProgress`]).
 */
import type { QueryClient, QueryKey } from "@tanstack/react-query";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import type { ReadState } from "@/lib/reading-state";
import { readStateFor } from "@/lib/reading-state";

import {
  cachedCardProgress,
  cancelCardLists,
  fetchingCardLists,
  patchCardProgress,
  restoreCardProgress,
  resumeCardLists,
  type CardPatchToken,
  type CardProgress,
} from "../card-list-cache";
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

/** The fields every progress shape shares. */
type ProgressFields = { finished: boolean; page: number; percent: number };

/** One issue's progress before a mark: the stored record, or `null` when
 *  there was none (never opened). */
export type PriorProgress = Map<string, ProgressFields | null>;

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
  /** Per issue: the map's record before, and the one written. */
  mapIds: Map<
    string,
    { before: ProgressView | undefined; wrote: ProgressView }
  >;
  continueReading: ContinueReadingView | undefined;
  cards: CardPatchToken;
  /** Card-list fetches this write cancelled; re-run when it settles. */
  cancelled: QueryKey[];
  /** Per issue: the issue page's record before, and the one written. */
  issues: Map<
    string,
    { before: ProgressView | null | undefined; wrote: ProgressView }
  >;
};

function nextRecord(
  prev: (ProgressFields & { run?: number }) | null | undefined,
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
    // Display only: nothing orders by it.
    updated_at: new Date().toISOString(),
  };
}

function cardCopy(r: ProgressView): NonNullable<CardProgress> {
  return {
    finished: r.finished,
    page: r.page,
    percent: r.percent,
    updated_at: r.updated_at,
  };
}

/** Write `writes` into every progress cache; returns what to roll back
 *  to. Cancels in-flight fetches first so one that started before the
 *  write can't land on top of the optimistic state. */
export async function applyOptimisticProgress(
  qc: QueryClient,
  writes: readonly ProgressWrite[],
): Promise<Snapshot> {
  const [, , cancelled] = await Promise.all([
    qc.cancelQueries({ queryKey: queryKeys.userProgress }),
    qc.cancelQueries({ queryKey: queryKeys.continueReading }),
    cancelCardLists(qc),
  ]);
  const map = qc.getQueryData<ProgressMap>(queryKeys.userProgress);
  const continueReading = qc.getQueryData<ContinueReadingView>(
    queryKeys.continueReading,
  );
  const records = new Map(
    writes.map((w) => {
      const prev =
        qc.getQueryData<ProgressView | null>(
          queryKeys.issueProgress(w.issue_id),
        ) ?? map?.get(w.issue_id);
      return [w.issue_id, nextRecord(prev, w)] as const;
    }),
  );

  const mapIds: Snapshot["mapIds"] = new Map();
  // Only patch a map that's already loaded: seeding a partial one would
  // read as "everything else unread" to every card for its stale time.
  if (map) {
    const next = new Map(map);
    for (const [id, rec] of records) {
      mapIds.set(id, { before: map.get(id), wrote: rec });
      next.set(id, rec);
    }
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
  const cards = patchCardProgress(
    qc,
    new Map([...records].map(([id, r]) => [id, cardCopy(r)])),
  );
  const issues: Snapshot["issues"] = new Map();
  for (const [id, rec] of records) {
    const key = queryKeys.issueProgress(id);
    const before = qc.getQueryData<ProgressView | null>(key);
    qc.setQueryData(key, rec);
    // Read back: structural sharing may have stored a copy of `rec`.
    issues.set(id, { before, wrote: qc.getQueryData<ProgressView>(key)! });
  }
  return { map, mapIds, continueReading, cards, cancelled, issues };
}

/** Undo [`applyOptimisticProgress`] after a failed write, for the issues
 *  in `ids` (all of them by default), and in each cache only where it
 *  still holds this write's own record. */
export function rollbackProgress(
  qc: QueryClient,
  snap: Snapshot | undefined,
  ids?: ReadonlySet<string>,
) {
  if (!snap) return;
  const mine = (id: string) => !ids || ids.has(id);
  const map = qc.getQueryData<ProgressMap>(queryKeys.userProgress);
  if (map && snap.mapIds.size > 0) {
    const next = new Map(map);
    let changed = false;
    for (const [id, { before, wrote }] of snap.mapIds) {
      if (!mine(id) || next.get(id) !== wrote) continue;
      if (before) next.set(id, before);
      else next.delete(id);
      changed = true;
    }
    if (changed) qc.setQueryData(queryKeys.userProgress, next);
  }
  if (snap.continueReading && !ids) {
    qc.setQueryData(queryKeys.continueReading, snap.continueReading);
  }
  restoreCardProgress(qc, snap.cards, ids);
  if (!ids) resumeCardLists(qc, snap.cancelled);
  for (const [id, { before, wrote }] of snap.issues) {
    const key = queryKeys.issueProgress(id);
    if (!mine(id) || qc.getQueryData(key) !== wrote) continue;
    // `resetQueries` notifies the issue page (a removed query does not).
    if (before === undefined)
      void qc.resetQueries({ queryKey: key, exact: true });
    else qc.setQueryData(key, before);
  }
}

/**
 * A write succeeded. Put its result into every cached card list again —
 * the server's `reply` for a single mark, else the optimistic records —
 * so a list that mounted and fetched while the request was in flight
 * (pre-commit data, not in the patch token) shows it too. The issue
 * page's record takes the reply where the optimistic one still stands.
 * Then re-run the card-list fetches this write cancelled, and any that
 * are in flight now (they may have started before the commit).
 */
export function settleProgress(
  qc: QueryClient,
  snap: Snapshot | undefined,
  reply?: ProgressView | null,
  extraCancelled: readonly QueryKey[] = [],
) {
  if (!snap) return;
  if (reply) {
    patchCardProgress(qc, new Map([[reply.issue_id, cardCopy(reply)]]));
    const own = snap.issues.get(reply.issue_id);
    const key = queryKeys.issueProgress(reply.issue_id);
    if (own && qc.getQueryData(key) === own.wrote) qc.setQueryData(key, reply);
  } else {
    patchCardProgress(
      qc,
      new Map([...snap.issues].map(([id, { wrote }]) => [id, cardCopy(wrote)])),
    );
  }
  resumeCardLists(qc, [
    ...extraCancelled,
    ...snap.cancelled,
    ...fetchingCardLists(qc),
  ]);
}

/** Per-id reads (`?issue_id=`, one row each) cover at most this many
 *  issues no cache knows; past it the full list is one request. */
const PRIOR_PER_ID_MAX = 8;

/** Each issue's record before the mark, for Undo. Taken from what the tab
 *  already has: the issue page's per-issue record, then the progress
 *  embedded on cached cards (the most recently received list; `null` is a
 *  known "no record"), then a loaded `/progress` map. Only issues none of
 *  them know go to the network. The cache reads happen synchronously, so
 *  calling this before the optimistic write sees the pre-mark state. */
export async function capturePriorProgress(
  qc: QueryClient,
  issueIds: readonly string[],
): Promise<PriorProgress> {
  const prior: PriorProgress = new Map();
  const cards = cachedCardProgress(qc, issueIds);
  const map = qc.getQueryData<ProgressMap>(queryKeys.userProgress);
  const unknown: string[] = [];
  for (const id of issueIds) {
    const own = qc.getQueryData<ProgressView | null>(
      queryKeys.issueProgress(id),
    );
    if (own !== undefined) prior.set(id, own);
    else if (cards.has(id)) prior.set(id, cards.get(id) ?? null);
    else if (map) prior.set(id, map.get(id) ?? null);
    else unknown.push(id);
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
function writeFor(id: string, rec: ProgressFields | null): ProgressWrite {
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
 * On failure the caches go back to the marked state they showed before
 * the Undo (the server still has it, unless part of it applied), and the
 * mounted lists refetch.
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
    settleProgress(qc, snap);
  } finally {
    if (!ok) rollbackProgress(qc, snap);
    qc.invalidateQueries({ queryKey: queryKeys.userProgress });
    invalidateRails(qc);
    invalidateSeriesProgress(qc);
    // A failed Undo may have applied part of its writes: refetch the
    // mounted lists so the server's state replaces the rollback.
    invalidateIssueCards(qc, { refetchActive: !ok });
  }
}

/**
 * The issue page's read state, live: the per-issue record once a mark
 * (or its Undo) has touched it in this tab, else the server-rendered
 * `fallback`. Observes the cache only — never fetches. A series-level
 * mark resets it (`invalidateIssueCards`), and an unobserved record is
 * garbage-collected on TanStack's usual schedule.
 */
export function useIssueReadState(
  issue: { id: string; page_count?: number | null },
  fallback: ReadState,
): ReadState {
  const qc = useQueryClient();
  const { data } = useQuery({
    queryKey: queryKeys.issueProgress(issue.id),
    queryFn: () =>
      qc.getQueryData<ProgressView | null>(queryKeys.issueProgress(issue.id)) ??
      null,
    enabled: false,
  });
  return data === undefined ? fallback : readStateFor(issue, data);
}
