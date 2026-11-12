/**
 * The card detector finds embedded progress in every response shape that
 * carries it, and the patch / restore / read helpers work over the cached
 * lists by key (`isCardListKey`).
 */
import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";

import {
  cachedCardProgress,
  embeddedCards,
  isEmbeddedCard,
  patchCardProgress,
  restoreCardProgress,
} from "@/lib/api/card-list-cache";
import { isCardListKey } from "@/lib/api/mutations/rails";
import { queryKeys } from "@/lib/api/query-keys";

const T0 = "2026-11-01T10:00:00+00:00";
const READ = { finished: true, page: 19, percent: 1, updated_at: T0 };

function card(id: string, progress: unknown = null) {
  return { id, series_slug: "saga", title: id, progress };
}

const ids = (data: unknown) => embeddedCards(data).map((c) => c.id);

describe("embeddedCards", () => {
  it.each([
    // series issues, recent / on-this-day / new-in-series rails, entity
    // issues, saved-view issue results: `{ items: [card] }`
    ["plain list", { items: [card("a"), card("b")], next_cursor: null }],
    // the same, as an infinite query
    [
      "infinite list",
      {
        pages: [{ items: [card("a")] }, { items: [card("b")] }],
        pageParams: [undefined, "p2"],
      },
    ],
    // issue search hits: `{ items: [{ issue: card, ... }] }`
    [
      "search hits",
      { items: [{ issue: card("a"), rank: 1 }, { issue: card("b") }] },
    ],
    // CBL entries: `{ items: [{ issue?: card }] }` (unmatched entries
    // carry no issue)
    [
      "CBL entries",
      { items: [{ issue: card("a") }, { issue: null }, { issue: card("b") }] },
    ],
    // collection entries, infinite: `pages[].items[].issue` (depth 5)
    [
      "infinite collection entries",
      {
        pages: [
          { items: [{ entry_kind: "issue", issue: card("a"), series: null }] },
          { items: [{ entry_kind: "issue", issue: card("b") }] },
        ],
        pageParams: [undefined, "x"],
      },
    ],
    // entity insights: `{ earliest: [card], ... }`
    ["insights earliest", { earliest: [card("a"), card("b")], totals: {} }],
  ])("finds the cards in %s", (_name, data) => {
    expect(ids(data)).toEqual(["a", "b"]);
  });

  it("skips cards without the field, series views, and anything deeper than five", () => {
    expect(isEmbeddedCard({ id: "a", series_slug: "s" })).toBe(false);
    expect(isEmbeddedCard({ id: "s", slug: "s", progress: null })).toBe(false);
    expect(ids({ a: { b: { c: { d: { e: { f: card("x") } } } } } })).toEqual(
      [],
    );
  });

  it("does not descend into a card", () => {
    const nested = { ...card("a"), related: [card("b")] };
    expect(ids({ items: [nested] })).toEqual(["a"]);
  });
});

describe("isCardListKey", () => {
  it("covers the card lists and nothing else", () => {
    expect(isCardListKey(queryKeys.seriesIssues("s", {}))).toBe(true);
    expect(isCardListKey(["rails", "recent-issues"])).toBe(true);
    expect(isCardListKey(["collections", "entries-infinite", "c"])).toBe(true);
    expect(isCardListKey(["cbl-lists", "entries", "l"])).toBe(true);
    expect(isCardListKey(queryKeys.userProgress)).toBe(false);
    expect(isCardListKey(queryKeys.issueProgress("a"))).toBe(false);
    expect(isCardListKey(["series", "s"])).toBe(false);
  });
});

describe("patch / restore / read", () => {
  function seeded() {
    const qc = new QueryClient();
    qc.setQueryData(queryKeys.seriesIssues("s", {}), {
      pages: [{ items: [card("a"), card("b", READ)] }],
      pageParams: [undefined],
    });
    qc.setQueryData(["rails", "recent-issues"], { items: [card("a")] });
    // Not a card list: never touched, never read.
    qc.setQueryData(["series", "s"], { items: [card("a")] });
    return qc;
  }

  it("patches every card list immutably and restores per id", () => {
    const qc = seeded();
    const before = qc.getQueryData(queryKeys.seriesIssues("s", {}));
    const other = qc.getQueryData(["series", "s"]);
    const token = patchCardProgress(qc, new Map([["a", READ]]));
    expect(token).toHaveLength(2);
    const after = qc.getQueryData(queryKeys.seriesIssues("s", {}));
    expect(after).not.toBe(before);
    expect(ids(after)).toEqual(["a", "b"]);
    expect(embeddedCards(after)[0]!.progress).toEqual(READ);
    expect(qc.getQueryData(["series", "s"])).toBe(other);

    restoreCardProgress(qc, token);
    expect(
      embeddedCards(qc.getQueryData(["rails", "recent-issues"]))[0]!.progress,
    ).toBeNull();
    expect(
      embeddedCards(qc.getQueryData(queryKeys.seriesIssues("s", {})))[0]!
        .progress,
    ).toBeNull();
  });

  it("leaves a card a later write replaced", () => {
    const qc = seeded();
    const first = patchCardProgress(qc, new Map([["a", READ]]));
    const mid = { finished: false, page: 4, percent: 0.2, updated_at: T0 };
    patchCardProgress(qc, new Map([["a", mid]]));
    restoreCardProgress(qc, first);
    expect(
      embeddedCards(qc.getQueryData(["rails", "recent-issues"]))[0]!.progress,
    ).toEqual(mid);
  });

  it("reads the copy from the most recently received list", () => {
    const qc = new QueryClient();
    qc.setQueryData(
      ["rails", "recent-issues"],
      { items: [card("a", READ)] },
      {
        updatedAt: 2000,
      },
    );
    qc.setQueryData(
      queryKeys.seriesIssues("s", {}),
      { items: [card("a")] },
      {
        updatedAt: 1000,
      },
    );
    const got = cachedCardProgress(qc, ["a", "z"]);
    expect(got.get("a")).toEqual(READ);
    expect(got.has("z")).toBe(false);
  });
});

describe("a patch does not change a list's age", () => {
  it("keeps dataUpdatedAt and a stale list's invalidated flag, through patch and restore", async () => {
    const qc = new QueryClient();
    const key = ["rails", "recent-issues"];
    qc.setQueryData(
      key,
      { items: [card("a"), card("b", READ)] },
      {
        updatedAt: 1000,
      },
    );
    await qc.invalidateQueries({ queryKey: key, refetchType: "none" });
    const state = () => qc.getQueryCache().find({ queryKey: key })!.state;
    expect(state().isInvalidated).toBe(true);

    const token = patchCardProgress(qc, new Map([["a", READ]]));
    expect(state().dataUpdatedAt).toBe(1000);
    expect(state().isInvalidated).toBe(true);
    expect(embeddedCards(state().data)[0]!.progress).toEqual(READ);

    restoreCardProgress(qc, token);
    expect(state().dataUpdatedAt).toBe(1000);
    expect(state().isInvalidated).toBe(true);
    expect(embeddedCards(state().data)[0]!.progress).toBeNull();
  });

  it("leaves a fresh list fresh", () => {
    const qc = new QueryClient();
    const key = ["rails", "recent-issues"];
    qc.setQueryData(key, { items: [card("a")] }, { updatedAt: 1000 });
    patchCardProgress(qc, new Map([["a", READ]]));
    const state = qc.getQueryCache().find({ queryKey: key })!.state;
    expect(state.dataUpdatedAt).toBe(1000);
    expect(state.isInvalidated).toBe(false);
  });
});
