/** URL ↔ state plumbing for the M4 `/search?category=series` facets.
 *  Round-trip + edge-case coverage for the helpers behind the filter
 *  sheet + sort dropdown. */
import { describe, expect, it } from "vitest";

import {
  EMPTY_SERIES_SEARCH_FILTERS,
  countActiveSeriesFilters,
  parseSeriesSearchFilters,
  seriesSearchFiltersToHook,
  seriesSearchFiltersToParams,
} from "@/lib/search/series-search-filters";

describe("parseSeriesSearchFilters", () => {
  it("returns defaults for empty input", () => {
    expect(parseSeriesSearchFilters({})).toEqual(EMPTY_SERIES_SEARCH_FILTERS);
  });

  it("ignores unknown sort values", () => {
    expect(parseSeriesSearchFilters({ sort: "bogus" }).sort).toBe("relevance");
  });

  it("accepts every supported sort value", () => {
    for (const s of [
      "relevance",
      "name",
      "year",
      "created_at",
      "updated_at",
    ] as const) {
      expect(parseSeriesSearchFilters({ sort: s }).sort).toBe(s);
    }
  });

  it("falls back to 'any' for unknown status", () => {
    expect(parseSeriesSearchFilters({ status: "ghost" }).status).toBe("any");
  });

  it("parses CSV publishers, trims whitespace, drops empties", () => {
    expect(
      parseSeriesSearchFilters({ publisher: "Marvel,, DC ,  Image ,, " })
        .publishers,
    ).toEqual(["Marvel", "DC", "Image"]);
  });

  it("preserves year range strings as-is", () => {
    const out = parseSeriesSearchFilters({
      year_from: "2018",
      year_to: "2024",
    });
    expect(out.yearFrom).toBe("2018");
    expect(out.yearTo).toBe("2024");
  });
});

describe("seriesSearchFiltersToParams", () => {
  it("returns an empty object when state is at defaults", () => {
    expect(seriesSearchFiltersToParams(EMPTY_SERIES_SEARCH_FILTERS)).toEqual(
      {},
    );
  });

  it("emits only non-default keys", () => {
    expect(
      seriesSearchFiltersToParams({
        ...EMPTY_SERIES_SEARCH_FILTERS,
        sort: "year",
        yearFrom: "2020",
        status: "continuing",
        publishers: ["Image", "Marvel"],
      }),
    ).toEqual({
      sort: "year",
      year_from: "2020",
      status: "continuing",
      publisher: "Image,Marvel",
    });
  });

  it("round-trips parse → serialise", () => {
    const params = {
      sort: "name",
      year_from: "2018",
      year_to: "2024",
      status: "ended",
      publisher: "Image",
      library: "abc-123",
    };
    const parsed = parseSeriesSearchFilters(params);
    expect(seriesSearchFiltersToParams(parsed)).toEqual(params);
  });
});

describe("seriesSearchFiltersToHook", () => {
  it("returns empty when state is at defaults", () => {
    expect(seriesSearchFiltersToHook(EMPTY_SERIES_SEARCH_FILTERS)).toEqual({});
  });

  it("converts year range to numbers", () => {
    const out = seriesSearchFiltersToHook({
      ...EMPTY_SERIES_SEARCH_FILTERS,
      yearFrom: "2018",
      yearTo: "2024",
    });
    expect(out.year_from).toBe(2018);
    expect(out.year_to).toBe(2024);
  });

  it("drops sort=relevance (let backend default to ts_rank)", () => {
    expect(
      seriesSearchFiltersToHook({
        ...EMPTY_SERIES_SEARCH_FILTERS,
        sort: "relevance",
      }).sort,
    ).toBeUndefined();
  });
});

describe("countActiveSeriesFilters", () => {
  it("returns 0 for defaults", () => {
    expect(countActiveSeriesFilters(EMPTY_SERIES_SEARCH_FILTERS)).toBe(0);
  });

  it("counts year range as a single facet", () => {
    expect(
      countActiveSeriesFilters({
        ...EMPTY_SERIES_SEARCH_FILTERS,
        yearFrom: "2020",
        yearTo: "2024",
      }),
    ).toBe(1);
  });

  it("counts publishers individually", () => {
    expect(
      countActiveSeriesFilters({
        ...EMPTY_SERIES_SEARCH_FILTERS,
        publishers: ["A", "B", "C"],
      }),
    ).toBe(3);
  });

  it("sort does not count", () => {
    expect(
      countActiveSeriesFilters({
        ...EMPTY_SERIES_SEARCH_FILTERS,
        sort: "year",
      }),
    ).toBe(0);
  });
});
