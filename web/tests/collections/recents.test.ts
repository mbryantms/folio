import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  getRecentCollectionIds,
  partitionByRecents,
  recordCollectionUse,
} from "@/lib/collections/recents";

describe("partitionByRecents", () => {
  const list = [
    { id: "a", name: "A" },
    { id: "b", name: "B" },
    { id: "c", name: "C" },
  ];

  it("promotes recent ids in MRU order and keeps the rest in input order", () => {
    const { recent, rest } = partitionByRecents(list, ["c", "a"]);
    expect(recent.map((c) => c.id)).toEqual(["c", "a"]);
    expect(rest.map((c) => c.id)).toEqual(["b"]);
  });

  it("ignores recent ids no longer present and de-dupes", () => {
    const { recent, rest } = partitionByRecents(list, ["gone", "b", "b"]);
    expect(recent.map((c) => c.id)).toEqual(["b"]);
    expect(rest.map((c) => c.id)).toEqual(["a", "c"]);
  });

  it("returns everything as rest when there are no recents", () => {
    const { recent, rest } = partitionByRecents(list, []);
    expect(recent).toEqual([]);
    expect(rest.map((c) => c.id)).toEqual(["a", "b", "c"]);
  });
});

describe("recordCollectionUse / getRecentCollectionIds", () => {
  let store: Map<string, string>;
  beforeEach(() => {
    store = new Map();
    vi.stubGlobal("window", {
      localStorage: {
        getItem: (k: string) => store.get(k) ?? null,
        setItem: (k: string, v: string) => void store.set(k, v),
        removeItem: (k: string) => void store.delete(k),
      },
    });
  });
  afterEach(() => vi.unstubAllGlobals());

  it("records most-recent-first and de-dupes on re-use", () => {
    recordCollectionUse("a");
    recordCollectionUse("b");
    recordCollectionUse("a"); // re-use moves a back to front
    expect(getRecentCollectionIds()).toEqual(["a", "b"]);
  });

  it("caps the list at 6 entries", () => {
    for (const id of ["1", "2", "3", "4", "5", "6", "7", "8"]) {
      recordCollectionUse(id);
    }
    const ids = getRecentCollectionIds();
    expect(ids).toHaveLength(6);
    // Most recent first; the two oldest (1, 2) are dropped.
    expect(ids).toEqual(["8", "7", "6", "5", "4", "3"]);
  });

  it("returns [] when nothing is stored", () => {
    expect(getRecentCollectionIds()).toEqual([]);
  });

  it("ignores empty ids", () => {
    recordCollectionUse("");
    expect(getRecentCollectionIds()).toEqual([]);
  });
});
