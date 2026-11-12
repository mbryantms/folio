/**
 * The issue-card lists in the query cache, as the one copy of each card's
 * embedded progress (`IssueSummaryView.progress`).
 *
 * A mark patches the cards in place (the standard TanStack optimistic
 * update, over every cached card list); a failure restores, per issue,
 * only what is still the mark's own write. Infinite lists keep the patch
 * in their cached pages, so a next-page fetch carries it along. Which
 * queries are card lists is defined once, by `isCardListKey` in
 * [`mutations/rails.ts`](./mutations/rails.ts).
 *
 * Not reachable from the reader's first load: the reader only marks the
 * lists stale (through `mutations/rails.ts`).
 */
import type { QueryClient, QueryKey } from "@tanstack/react-query";

import { isCardListKey } from "./mutations/rails";
import type { IssueCardProgress } from "./types";

/** A card's embedded progress: the record, or `null` for "no record". */
export type CardProgress = IssueCardProgress | null;

const cardLists = {
  predicate: (q: { queryKey: QueryKey }) => isCardListKey(q.queryKey),
};

/** Cards sit at most this deep in a card-list response (an infinite
 *  list's `pages[].items[].issue` is depth 5). */
const MAX_DEPTH = 5;

type Card = { id: string; series_slug: string; progress: CardProgress };

/** An issue card with embedded progress: `id` + `series_slug` + a
 *  `progress` key (an object or `null`; absent = not loaded). */
export function isEmbeddedCard(v: unknown): v is Card {
  if (v === null || typeof v !== "object" || Array.isArray(v)) return false;
  const o = v as Record<string, unknown>;
  return (
    typeof o.id === "string" &&
    typeof o.series_slug === "string" &&
    "progress" in o
  );
}

/** Every card with embedded progress in a card-list response. */
export function embeddedCards(data: unknown): Card[] {
  const out: Card[] = [];
  const visit = (v: unknown, depth: number) => {
    if (depth > MAX_DEPTH || v === null || typeof v !== "object") return;
    if (isEmbeddedCard(v)) {
      out.push(v);
      return;
    }
    const children = Array.isArray(v) ? v : Object.values(v);
    for (const x of children) visit(x, depth + 1);
  };
  visit(data, 0);
  return out;
}

/** `data` with `fn` applied to each card's progress; untouched subtrees
 *  (and the whole value, when nothing changed) keep their identity. */
function mapCards(
  data: unknown,
  fn: (card: Card) => CardProgress | undefined,
): unknown {
  const visit = (v: unknown, depth: number): unknown => {
    if (depth > MAX_DEPTH || v === null || typeof v !== "object") return v;
    if (isEmbeddedCard(v)) {
      const next = fn(v);
      return next === undefined || next === v.progress
        ? v
        : { ...v, progress: next };
    }
    if (Array.isArray(v)) {
      let changed = false;
      const arr = v.map((x) => {
        const y = visit(x, depth + 1);
        if (y !== x) changed = true;
        return y;
      });
      return changed ? arr : v;
    }
    let copy: Record<string, unknown> | null = null;
    for (const [k, x] of Object.entries(v)) {
      const y = visit(x, depth + 1);
      if (y !== x) (copy ??= { ...(v as Record<string, unknown>) })[k] = y;
    }
    return copy ?? v;
  };
  return visit(data, 0);
}

/** What a patch replaced: per query, per issue, the copy it found and the
 *  copies it left in the cache. `wrote` holds the objects as stored —
 *  TanStack's structural sharing copies changed objects on `setQueryData`,
 *  so the patch reads them back rather than keeping the ones it passed. */
export type CardPatchToken = Array<{
  queryKey: QueryKey;
  ids: Map<string, { before: CardProgress; wrote: Set<CardProgress> }>;
}>;

/** Cancel in-flight card-list fetches, so one that started before a
 *  write cannot land after it and overwrite the patch. Returns the keys it
 *  cancelled: a cancelled fetch goes back to its earlier state and nothing
 *  re-runs it, so the mutation hands them to [`resumeCardLists`] when it
 *  settles (TanStack's cancel-in-onMutate, refetch-in-onSettled recipe). */
export async function cancelCardLists(qc: QueryClient): Promise<QueryKey[]> {
  const keys = fetchingCardLists(qc);
  await Promise.all(
    keys.map((queryKey) => qc.cancelQueries({ queryKey, exact: true })),
  );
  return keys;
}

/** Card lists with a fetch in flight right now. */
export function fetchingCardLists(qc: QueryClient): QueryKey[] {
  return qc
    .getQueryCache()
    .findAll({ ...cardLists, fetchStatus: "fetching" })
    .map((q) => q.queryKey);
}

/** Refetch `keys` (once each) that are still mounted. Run when a mark
 *  settles, for the fetches it cancelled and, on success, for any that
 *  started before the server committed. */
export function resumeCardLists(qc: QueryClient, keys: readonly QueryKey[]) {
  const seen = new Set<string>();
  for (const queryKey of keys) {
    const hash = JSON.stringify(queryKey);
    if (seen.has(hash)) continue;
    seen.add(hash);
    void qc.refetchQueries({ queryKey, exact: true, type: "active" });
  }
}

/** Write a patched copy of a list without changing its age: the patch
 *  keeps `dataUpdatedAt` (so it doesn't pass for the newest list in
 *  [`cachedCardProgress`]) and a list marked stale stays stale (a write
 *  dispatches "success", which would clear the flag). */
function writeList(qc: QueryClient, queryKey: QueryKey, data: unknown) {
  const query = qc.getQueryCache().find({ queryKey, exact: true });
  const wasInvalidated = query?.state.isInvalidated ?? false;
  qc.setQueryData(queryKey, data, {
    updatedAt: query?.state.dataUpdatedAt,
  });
  if (wasInvalidated) {
    void qc.invalidateQueries({ queryKey, exact: true, refetchType: "none" });
  }
}

/** Replace the embedded progress of every cached card whose id is in
 *  `changes`. Returns what it replaced, for [`restoreCardProgress`]. */
export function patchCardProgress(
  qc: QueryClient,
  changes: ReadonlyMap<string, CardProgress>,
): CardPatchToken {
  const token: CardPatchToken = [];
  for (const [queryKey, data] of qc.getQueriesData(cardLists)) {
    if (data === undefined) continue;
    const ids = new Map<
      string,
      { before: CardProgress; wrote: Set<CardProgress> }
    >();
    const next = mapCards(data, (card) => {
      if (!changes.has(card.id)) return undefined;
      if (!ids.has(card.id)) {
        ids.set(card.id, { before: card.progress, wrote: new Set() });
      }
      return changes.get(card.id) ?? null;
    });
    if (next !== data) {
      writeList(qc, queryKey, next);
      for (const card of embeddedCards(qc.getQueryData(queryKey))) {
        ids.get(card.id)?.wrote.add(card.progress);
      }
      token.push({ queryKey, ids });
    }
  }
  return token;
}

/** For each card in `token` whose cached progress is still exactly the
 *  copy the patch wrote, `next(id, before)`'s value. A card a later write
 *  or a refetch has replaced since is left alone. */
function replaceOwn(
  qc: QueryClient,
  token: CardPatchToken,
  next: (id: string, before: CardProgress) => CardProgress | undefined,
) {
  for (const { queryKey, ids } of token) {
    const data = qc.getQueryData(queryKey);
    if (data === undefined) continue;
    const out = mapCards(data, (card) => {
      const own = ids.get(card.id);
      if (!own || !own.wrote.has(card.progress)) return undefined;
      return next(card.id, own.before);
    });
    if (out !== data) writeList(qc, queryKey, out);
  }
}

/** Undo a patch after a failed write, per issue (see [`replaceOwn`]). */
export function restoreCardProgress(
  qc: QueryClient,
  token: CardPatchToken,
  ids?: ReadonlySet<string>,
) {
  replaceOwn(qc, token, (id, before) =>
    ids && !ids.has(id) ? undefined : before,
  );
}

/** For each of `ids` some cached card list carries: its embedded copy,
 *  from the list this tab received most recently. No request. */
export function cachedCardProgress(
  qc: QueryClient,
  ids: readonly string[],
): Map<string, CardProgress> {
  const wanted = new Set(ids);
  const found = new Map<string, { p: CardProgress; at: number }>();
  for (const query of qc.getQueryCache().findAll(cardLists)) {
    const at = query.state.dataUpdatedAt;
    for (const card of embeddedCards(query.state.data)) {
      if (!wanted.has(card.id)) continue;
      const cur = found.get(card.id);
      if (!cur || at > cur.at) found.set(card.id, { p: card.progress, at });
    }
  }
  return new Map([...found].map(([id, v]) => [id, v.p]));
}
