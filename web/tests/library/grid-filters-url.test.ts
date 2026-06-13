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

  it("never serializes in-grid search (q stays local, collides with SearchView)", () => {
    // q isn't part of LibraryGridUrlState — guard that no stray q leaks.
    const qs = serializeLibraryGridFilters(
      baseState({ genres: ["Horror"], publishers: ["Image"] }),
    );
    expect(qs).not.toContain("q=");
  });

  it("round-trips a fully-populated facet set through parse", () => {
    const state = baseState({
      mode: "issues",
      status: "ended",
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
});
