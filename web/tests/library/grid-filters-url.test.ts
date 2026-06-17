/**
 * URL ↔ grid-filter round-trip (audit B2).
 *
 * `serializeLibraryGridFilters` is the inverse of
 * `parseLibraryGridFilters`; the grid's bidirectional URL sync relies on
 * `parse(serialize(x))` recovering the same facet set so its loop guard
 * (compare current state's serialization to the URL's) converges. These
 * tests anchor that invariant plus the "omit defaults" contract that
 * keeps a pristine grid's URL short.
 */
import { describe, expect, it } from "vitest";

import {
  EMPTY_CREDITS,
  parseLibraryGridFilters,
  parseStartsWithParam,
  serializeLibraryGridFilters,
  type LibraryGridUrlState,
} from "@/components/library/library-grid-filters";

function baseState(
  over: Partial<LibraryGridUrlState> = {},
): LibraryGridUrlState {
  return {
    library: "all",
    mode: "series",
    status: undefined,
    readStatus: [],
    startsWith: null,
    yearFrom: undefined,
    yearTo: undefined,
    publishers: [],
    languages: [],
    ageRatings: [],
    genres: [],
    tags: [],
    credits: { ...EMPTY_CREDITS },
    anyCredits: [],
    characters: [],
    teams: [],
    locations: [],
    ratingRange: null,
    ...over,
  };
}

function parseQs(qs: string) {
  const raw: Record<string, string> = {};
  new URLSearchParams(qs).forEach((v, k) => {
    raw[k] = v;
  });
  return raw;
}

describe("serializeLibraryGridFilters", () => {
  it("a pristine grid serializes to just the library param", () => {
    expect(serializeLibraryGridFilters(baseState())).toBe("library=all");
  });

  it("omits the default series mode but emits mode=issues", () => {
    expect(
      serializeLibraryGridFilters(baseState({ mode: "issues" })),
    ).toContain("mode=issues");
    expect(
      serializeLibraryGridFilters(baseState({ mode: "series" })),
    ).not.toContain("mode=");
  });

  it("never serializes in-grid search (q stays local; ?q= now redirects to /search)", () => {
    // q isn't part of LibraryGridUrlState — guard that no stray q leaks.
    // A leaked `?q=` would trip HomePage's redirect to the dedicated
    // /search page (audit E2 / 1.6) and yank the user off the grid.
    const qs = serializeLibraryGridFilters(
      baseState({ genres: ["Horror"], publishers: ["Image"] }),
    );
    expect(qs).not.toContain("q=");
  });

  it("round-trips a fully-populated facet set through parse", () => {
    const state = baseState({
      mode: "issues",
      status: "ended",
      metadataCompleteness: "needs_metadata",
      readStatus: ["unread", "in_progress"],
      startsWith: "s",
      yearFrom: "1990",
      yearTo: "2005",
      publishers: ["Image", "Marvel"],
      languages: ["en"],
      ageRatings: ["Teen"],
      genres: ["Horror", "Sci-Fi"],
      tags: ["one-shot"],
      credits: { ...EMPTY_CREDITS, writers: ["Brian K. Vaughan"] },
      anyCredits: ["Fiona Staples"],
      characters: ["Saga"],
      teams: ["Blackguard"],
      locations: ["Cleave"],
      ratingRange: [2, 4.5],
    });
    const qs = serializeLibraryGridFilters(state);
    const parsed = parseLibraryGridFilters(parseQs(qs));
    expect(parsed).toMatchObject({
      mode: "issues",
      status: "ended",
      metadataCompleteness: "needs_metadata",
      readStatus: ["unread", "in_progress"],
      startsWith: "s",
      yearFrom: "1990",
      yearTo: "2005",
      publishers: ["Image", "Marvel"],
      languages: ["en"],
      ageRatings: ["Teen"],
      genres: ["Horror", "Sci-Fi"],
      tags: ["one-shot"],
      credits: { writers: ["Brian K. Vaughan"] },
      anyCredits: ["Fiona Staples"],
      characters: ["Saga"],
      teams: ["Blackguard"],
      locations: ["Cleave"],
      ratingRange: [2, 4.5],
    });
  });

  it("is order-stable: same state always serializes identically (loop-guard invariant)", () => {
    const a = baseState({ genres: ["a", "b"], tags: ["x"], status: "ended" });
    expect(serializeLibraryGridFilters(a)).toBe(serializeLibraryGridFilters(a));
  });

  it("status=any is treated as no filter", () => {
    expect(serializeLibraryGridFilters(baseState({ status: "any" }))).toBe(
      "library=all",
    );
  });

  it("emits the metadata_completeness worklist param and round-trips it", () => {
    const qs = serializeLibraryGridFilters(
      baseState({ metadataCompleteness: "needs_metadata" }),
    );
    expect(qs).toContain("metadata_completeness=needs_metadata");
    expect(parseLibraryGridFilters(parseQs(qs))).toMatchObject({
      metadataCompleteness: "needs_metadata",
    });
  });

  it("ignores an unknown metadata_completeness value (defends the deep-link)", () => {
    expect(
      parseLibraryGridFilters({ metadata_completeness: "bogus" }),
    ).toBeUndefined();
  });

  it("emits + round-trips the starts_with jump bucket", () => {
    const qs = serializeLibraryGridFilters(baseState({ startsWith: "s" }));
    expect(qs).toContain("starts_with=s");
    expect(parseLibraryGridFilters(parseQs(qs))).toMatchObject({
      startsWith: "s",
    });
    // The "#" bucket survives the URL round-trip too.
    const hashQs = serializeLibraryGridFilters(baseState({ startsWith: "#" }));
    expect(parseLibraryGridFilters(parseQs(hashQs))).toMatchObject({
      startsWith: "#",
    });
  });
});

describe("parseStartsWithParam", () => {
  it("normalizes a letter to lowercase", () => {
    expect(parseStartsWithParam("S")).toBe("s");
    expect(parseStartsWithParam("a")).toBe("a");
  });
  it("passes through the # bucket", () => {
    expect(parseStartsWithParam("#")).toBe("#");
  });
  it("rejects multi-char, digits, and empty (→ undefined)", () => {
    expect(parseStartsWithParam("ab")).toBeUndefined();
    expect(parseStartsWithParam("1")).toBeUndefined();
    expect(parseStartsWithParam("")).toBeUndefined();
    expect(parseStartsWithParam(undefined)).toBeUndefined();
  });
});
