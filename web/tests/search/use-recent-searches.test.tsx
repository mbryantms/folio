/** Recent-searches dedupe + cap semantics.
 *
 *  Exercises the pure helpers behind `useRecentSearches`. The hook
 *  itself wires these into `useState` + `localStorage`; we trust the
 *  React shell separately via Playwright in the modal verification
 *  step. Pure functions can run in vitest's default node environment
 *  with no DOM polyfill required. */
import { describe, expect, it } from "vitest";

import {
  RECENT_SEARCHES_MAX,
  appendRecent,
  removeRecent,
} from "@/lib/search/use-recent-searches";

describe("appendRecent", () => {
  it("appends a new query at the front", () => {
    expect(appendRecent([], "Saga")).toEqual(["Saga"]);
    expect(appendRecent(["Watchmen"], "Saga")).toEqual(["Saga", "Watchmen"]);
  });

  it("ignores queries shorter than 2 chars (intermediate keystrokes)", () => {
    expect(appendRecent(["Saga"], "s")).toEqual(["Saga"]);
    expect(appendRecent(["Saga"], "")).toEqual(["Saga"]);
    expect(appendRecent(["Saga"], "  ")).toEqual(["Saga"]);
  });

  it("dedupes case-insensitively and moves the entry to the front", () => {
    const before = ["Watchmen", "Saga"];
    const after = appendRecent(before, "saga");
    expect(after).toEqual(["saga", "Watchmen"]);
    // Original array isn't mutated.
    expect(before).toEqual(["Watchmen", "Saga"]);
  });

  it("caps at RECENT_SEARCHES_MAX entries (oldest evicted)", () => {
    let acc: string[] = [];
    for (let i = 0; i < RECENT_SEARCHES_MAX + 4; i++) {
      acc = appendRecent(acc, `query-${i}`);
    }
    expect(acc.length).toBe(RECENT_SEARCHES_MAX);
    // Most-recent first.
    expect(acc[0]).toBe(`query-${RECENT_SEARCHES_MAX + 3}`);
    // Oldest kept is the (RECENT_SEARCHES_MAX-1)th-most-recent — the
    // ones before it have fallen off the back of the buffer.
    expect(acc[acc.length - 1]).toBe(`query-4`);
  });

  it("trims surrounding whitespace before storing", () => {
    expect(appendRecent([], "  Saga  ")).toEqual(["Saga"]);
  });
});

describe("removeRecent", () => {
  it("removes a matching entry (case-insensitive)", () => {
    expect(removeRecent(["Saga", "Watchmen"], "saga")).toEqual(["Watchmen"]);
  });

  it("no-op when the entry is absent", () => {
    expect(removeRecent(["Saga"], "ghost")).toEqual(["Saga"]);
  });

  it("preserves the order of the remaining entries", () => {
    expect(removeRecent(["A", "B", "C", "D"], "B")).toEqual(["A", "C", "D"]);
  });
});
