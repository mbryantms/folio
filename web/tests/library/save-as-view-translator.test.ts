import { describe, it, expect } from "vitest";
import { libraryGridStateToFilterBuilderState } from "@/components/library/libraryGridStateToFilterState";
import { EMPTY_CREDITS } from "@/components/library/library-grid-filters";

const TODAY = "2026-05-18";

function snapshot(overrides: Partial<Parameters<typeof libraryGridStateToFilterBuilderState>[0]> = {}) {
  return {
    status: "any",
    yearFrom: "",
    yearTo: "",
    publishers: [],
    languages: [],
    ageRatings: [],
    genres: [],
    tags: [],
    credits: EMPTY_CREDITS,
    characters: [],
    teams: [],
    locations: [],
    ratingRange: null,
    trimmedQ: "",
    ...overrides,
  };
}

describe("libraryGridStateToFilterBuilderState", () => {
  it("produces an empty conditions array for an empty grid state", () => {
    const result = libraryGridStateToFilterBuilderState(snapshot(), TODAY);
    expect(result.state.conditions).toEqual([]);
    expect(result.state.matchMode).toBe("all");
    expect(result.state.name).toBe("Library filter — 2026-05-18");
    expect(result.droppedFacets).toEqual([]);
  });

  it("maps free-text query to `name contains`", () => {
    const result = libraryGridStateToFilterBuilderState(
      snapshot({ trimmedQ: "Saga" }),
      TODAY,
    );
    expect(result.state.conditions).toEqual([
      { group_id: 0, field: "name", op: "contains", value: "Saga" },
    ]);
  });

  it("maps status (non-any) to `status is`", () => {
    const result = libraryGridStateToFilterBuilderState(
      snapshot({ status: "continuing" }),
      TODAY,
    );
    expect(result.state.conditions).toEqual([
      { group_id: 0, field: "status", op: "is", value: "continuing" },
    ]);
  });

  it("skips `status` when value is `any`", () => {
    const result = libraryGridStateToFilterBuilderState(
      snapshot({ status: "any" }),
      TODAY,
    );
    expect(result.state.conditions).toEqual([]);
  });

  it("emits `year between` when both ends are set", () => {
    const result = libraryGridStateToFilterBuilderState(
      snapshot({ yearFrom: "2018", yearTo: "2024" }),
      TODAY,
    );
    expect(result.state.conditions).toEqual([
      { group_id: 0, field: "year", op: "between", value: [2018, 2024] },
    ]);
  });

  it("emits `year gte` when only yearFrom is set", () => {
    const result = libraryGridStateToFilterBuilderState(
      snapshot({ yearFrom: "2018", yearTo: "" }),
      TODAY,
    );
    expect(result.state.conditions).toEqual([
      { group_id: 0, field: "year", op: "gte", value: 2018 },
    ]);
  });

  it("emits `year lte` when only yearTo is set", () => {
    const result = libraryGridStateToFilterBuilderState(
      snapshot({ yearTo: "2024" }),
      TODAY,
    );
    expect(result.state.conditions).toEqual([
      { group_id: 0, field: "year", op: "lte", value: 2024 },
    ]);
  });

  it("emits scalar facets (publisher / language_code / age_rating) as `in`", () => {
    const result = libraryGridStateToFilterBuilderState(
      snapshot({
        publishers: ["Image", "DC"],
        languages: ["en"],
        ageRatings: ["Teen"],
      }),
      TODAY,
    );
    expect(result.state.conditions).toEqual([
      { group_id: 0, field: "publisher", op: "in", value: ["Image", "DC"] },
      { group_id: 0, field: "language_code", op: "in", value: ["en"] },
      { group_id: 0, field: "age_rating", op: "in", value: ["Teen"] },
    ]);
  });

  it("emits multi-valued facets as `includes_any` (genres / tags / characters / teams / locations)", () => {
    const result = libraryGridStateToFilterBuilderState(
      snapshot({
        genres: ["Horror"],
        tags: ["Indie"],
        characters: ["Spider-Man"],
        teams: ["Avengers"],
        locations: ["Gotham"],
      }),
      TODAY,
    );
    expect(result.state.conditions).toEqual([
      { group_id: 0, field: "genres", op: "includes_any", value: ["Horror"] },
      { group_id: 0, field: "tags", op: "includes_any", value: ["Indie"] },
      {
        group_id: 0,
        field: "characters",
        op: "includes_any",
        value: ["Spider-Man"],
      },
      { group_id: 0, field: "teams", op: "includes_any", value: ["Avengers"] },
      {
        group_id: 0,
        field: "locations",
        op: "includes_any",
        value: ["Gotham"],
      },
    ]);
  });

  it("maps per-credit-role state to the singular DSL field id", () => {
    const result = libraryGridStateToFilterBuilderState(
      snapshot({
        credits: {
          ...EMPTY_CREDITS,
          writers: ["Brian K. Vaughan"],
          cover_artists: ["Fiona Staples"],
        },
      }),
      TODAY,
    );
    expect(result.state.conditions).toEqual([
      {
        group_id: 0,
        field: "writer",
        op: "includes_any",
        value: ["Brian K. Vaughan"],
      },
      {
        group_id: 0,
        field: "cover_artist",
        op: "includes_any",
        value: ["Fiona Staples"],
      },
    ]);
  });

  it("drops `ratingRange` onto droppedFacets when narrower than the default", () => {
    const result = libraryGridStateToFilterBuilderState(
      snapshot({ ratingRange: [2.5, 5] }),
      TODAY,
    );
    expect(result.state.conditions).toEqual([]);
    expect(result.droppedFacets).toEqual(["Rating"]);
  });

  it("does NOT drop `ratingRange` when it spans the full [min, max] range", () => {
    const result = libraryGridStateToFilterBuilderState(
      snapshot({ ratingRange: [0, 5] }),
      TODAY,
    );
    expect(result.droppedFacets).toEqual([]);
  });

  it("kitchen-sink: every facet set produces conditions for each + no drops", () => {
    const result = libraryGridStateToFilterBuilderState(
      snapshot({
        trimmedQ: "v",
        status: "ended",
        yearFrom: "2010",
        yearTo: "2020",
        publishers: ["Marvel"],
        languages: ["en"],
        ageRatings: ["Teen"],
        genres: ["Action"],
        tags: ["Indie"],
        characters: ["Spider-Man"],
        teams: ["X-Men"],
        locations: ["New York"],
        credits: { ...EMPTY_CREDITS, writers: ["Alice"] },
      }),
      TODAY,
    );
    expect(result.state.conditions).toHaveLength(12);
    expect(result.droppedFacets).toEqual([]);
  });
});
