/**
 * This tab's progress writes, per issue, for the surfaces that do not
 * load every progress record of the user: issue cards (whose lists embed
 * the viewer's progress, `IssueSummaryView.progress`) and the issue page.
 *
 * An entry is what this tab last knew about one issue's record, and it is
 * shown in place of the card's list copy. Entries are ordered against list
 * copies by WHEN THIS TAB SAW THEM — a counter, never a clock, so a client
 * clock that is ahead of or behind the server can't change the answer:
 *
 *   - A write (optimistic mark, bulk mark, Undo, the reader's per-page
 *     reply) sets the entry. An optimistic one is *pending* until the
 *     server acknowledges it ([`settleOverlay`]); a reply is settled at once.
 *   - A failed write puts the previous entry back ([`restoreOverlay`]),
 *     or removes it, and the cards re-render (listeners are notified).
 *   - When a card list's fetch that STARTED after an entry settled lands,
 *     that list's copy of the issue replaces the entry (it already
 *     reflects the write, and anything newer). A fetch started earlier, or
 *     while the write was pending, or a snapshot hydrated from the server
 *     render, never does.
 *   - With no entry, a card shows its own list copy.
 *
 * One store per QueryClient; listeners are per issue, so a write
 * re-renders only that issue's cards. A mounted card costs one Set entry
 * and no query in the cache.
 *
 * Kept free of `queries.ts` so the reader can write entries without
 * pulling list code into its first load.
 */
import type { Query, QueryClient } from "@tanstack/react-query";
import { useCallback, useSyncExternalStore } from "react";

/** The fields every progress shape shares (a `ProgressView`, a card's
 *  `IssueCardProgress`, an optimistic record). */
export type OverlayRecord = {
  finished: boolean;
  percent: number;
  page: number;
  updated_at?: string;
};

export type OverlayEntry = {
  /** `null` = "no record" (an unread restore). */
  record: OverlayRecord | null;
  /** Counter value when the tab learned it; `Infinity` while pending. */
  seq: number;
};

/** Query-key heads whose data can carry issue cards with embedded
 *  progress — the same families `invalidateIssueCards` and
 *  `invalidateRails` cover. */
const CARD_LIST_HEADS = new Set([
  "series",
  "issues",
  "entities",
  "rails",
  "saved-views",
  "cbl-lists",
  "collections",
]);

type Listener = () => void;

export class ProgressOverlay {
  private entries = new Map<string, OverlayEntry>();
  private listeners = new Map<string, Set<Listener>>();
  private fetchStarted = new Map<string, number>();
  private seq = 0;

  constructor(qc: QueryClient) {
    qc.getQueryCache().subscribe((event) => {
      const query = event.query as Query;
      if (!CARD_LIST_HEADS.has(String(query.queryKey[0]))) return;
      if (event.type === "removed") {
        this.fetchStarted.delete(query.queryHash);
        return;
      }
      if (event.type !== "updated") return;
      if (event.action.type === "fetch") {
        this.fetchStarted.set(query.queryHash, ++this.seq);
      } else if (event.action.type === "success") {
        const started = this.fetchStarted.get(query.queryHash);
        if (started !== undefined && this.entries.size > 0) {
          this.adoptListCopies(event.action.data, started);
        }
      }
    });
  }

  get(id: string): OverlayEntry | undefined {
    return this.entries.get(id);
  }

  /** Write `record`; returns the entry it replaced (for rollback). */
  write(
    id: string,
    record: OverlayRecord | null,
    pending: boolean,
  ): OverlayEntry | undefined {
    const prev = this.entries.get(id);
    this.entries.set(id, { record, seq: pending ? Infinity : ++this.seq });
    this.notify(id);
    return prev;
  }

  /** The server acknowledged the pending writes for `ids`. */
  settle(ids: Iterable<string>, records?: Map<string, OverlayRecord>) {
    for (const id of ids) {
      const cur = this.entries.get(id);
      if (!cur) continue;
      this.entries.set(id, {
        record: records?.get(id) ?? cur.record,
        seq: ++this.seq,
      });
      this.notify(id);
    }
  }

  /** Put back what [`write`] replaced (`undefined` = no entry). */
  restore(id: string, prev: OverlayEntry | undefined) {
    if (prev) this.entries.set(id, prev);
    else this.entries.delete(id);
    this.notify(id);
  }

  subscribe(id: string, cb: Listener): () => void {
    let set = this.listeners.get(id);
    if (!set) this.listeners.set(id, (set = new Set()));
    set.add(cb);
    return () => {
      set.delete(cb);
      if (set.size === 0) this.listeners.delete(id);
    };
  }

  private notify(id: string) {
    this.listeners.get(id)?.forEach((cb) => cb());
  }

  /** A list fetch that started at `started` landed: its copy of each
   *  issue supersedes an entry learned before that fetch began. */
  private adoptListCopies(data: unknown, started: number) {
    for (const card of embeddedCards(data)) {
      const cur = this.entries.get(card.id);
      if (!cur || cur.seq >= started) continue;
      this.entries.set(card.id, { record: card.progress, seq: started });
      this.notify(card.id);
    }
  }
}

const overlays = new WeakMap<QueryClient, ProgressOverlay>();

export function progressOverlay(qc: QueryClient): ProgressOverlay {
  let o = overlays.get(qc);
  if (!o) overlays.set(qc, (o = new ProgressOverlay(qc)));
  return o;
}

/** This tab's entry for `issueId` (re-renders when it changes). */
export function useOverlayEntry(
  qc: QueryClient,
  issueId: string,
): OverlayEntry | undefined {
  const overlay = progressOverlay(qc);
  const subscribe = useCallback(
    (cb: Listener) => overlay.subscribe(issueId, cb),
    [overlay, issueId],
  );
  const get = () => overlay.get(issueId);
  return useSyncExternalStore(subscribe, get, get);
}

/** Every card in a list response that carries embedded progress
 *  (`progress` present, object or `null`). Walks the response shapes the
 *  card lists use: `items`, `pages[].items`, `items[].issue`,
 *  `earliest`, … — bounded depth, so non-card data costs little. */
export function embeddedCards(
  data: unknown,
): Array<{ id: string; progress: OverlayRecord | null }> {
  const out: Array<{ id: string; progress: OverlayRecord | null }> = [];
  const visit = (v: unknown, depth: number) => {
    if (depth > 5 || v === null || typeof v !== "object") return;
    if (Array.isArray(v)) {
      for (const x of v) visit(x, depth + 1);
      return;
    }
    const o = v as Record<string, unknown>;
    if (
      typeof o.id === "string" &&
      typeof o.series_slug === "string" &&
      "progress" in o
    ) {
      out.push({
        id: o.id,
        progress: (o.progress as OverlayRecord | null) ?? null,
      });
      return;
    }
    for (const key in o) {
      const x = o[key];
      if (x !== null && typeof x === "object") visit(x, depth + 1);
    }
  };
  visit(data, 0);
  return out;
}

/** For each of `ids` that some cached card list carries with embedded
 *  progress: that copy (`null` = a known "no record"). Reads only what
 *  the tab already has — no request. */
export function cachedCardProgress(
  qc: QueryClient,
  ids: readonly string[],
): Map<string, OverlayRecord | null> {
  const wanted = new Set(ids);
  const found = new Map<string, OverlayRecord | null>();
  for (const query of qc.getQueryCache().getAll()) {
    if (found.size === wanted.size) break;
    if (!CARD_LIST_HEADS.has(String(query.queryKey[0]))) continue;
    if (query.state.data === undefined) continue;
    for (const card of embeddedCards(query.state.data)) {
      if (wanted.has(card.id) && !found.has(card.id))
        found.set(card.id, card.progress);
    }
  }
  return found;
}
