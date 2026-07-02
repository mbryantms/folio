/** URL-driven filter + sort state for the `/search?category=series`
 *  grid. Parses + serialises the slice of `SeriesListFilters` the
 *  M4 facet UI exposes today (sort, year range, status, publisher).
 *  Lives in its own module so the server component can call the
 *  parser without dragging in the client search machinery.
 *
 *  Naming mirrors the params accepted by `/api/series`:
 *  - `sort` = `name | created_at | updated_at | year`
 *  - `year_from` / `year_to` = inclusive bounds
 *  - `status` = `continuing | ended | cancelled | hiatus`
 *  - `publisher` = CSV
 *
 *  Absent params resolve to the default surface: no filter, no
 *  sort (backend falls back to ts_rank_cd relevance). */

export type SeriesSearchSort =
  | "relevance"
  | "name"
  | "year"
  | "created_at"
  | "updated_at";

export const SERIES_SEARCH_SORT_OPTIONS: ReadonlyArray<{
  value: SeriesSearchSort;
  label: string;
}> = [
  { value: "relevance", label: "Best match" },
  { value: "name", label: "Name (A → Z)" },
  { value: "year", label: "Year (newest)" },
  { value: "created_at", label: "Recently added" },
  { value: "updated_at", label: "Recently updated" },
];

export const SERIES_STATUS_OPTIONS: ReadonlyArray<{
  value: string;
  label: string;
}> = [
  { value: "any", label: "Any status" },
  { value: "continuing", label: "Continuing" },
  { value: "ended", label: "Ended" },
  { value: "cancelled", label: "Cancelled" },
  { value: "hiatus", label: "Hiatus" },
];

export interface SeriesSearchFilterState {
  sort: SeriesSearchSort;
  yearFrom: string;
  yearTo: string;
  status: string;
  publishers: string[];
  library: string;
}

export const EMPTY_SERIES_SEARCH_FILTERS: SeriesSearchFilterState = {
  sort: "relevance",
  yearFrom: "",
  yearTo: "",
  status: "any",
  publishers: [],
  library: "all",
};

/** Parse the App-Router `searchParams` record into the filter state.
 *  Defaults fill missing keys so callers never have to handle
 *  `undefined`. */
export function parseSeriesSearchFilters(
  raw: Record<string, string | undefined>,
): SeriesSearchFilterState {
  const sortRaw = raw.sort;
  const sort: SeriesSearchSort = isSort(sortRaw) ? sortRaw : "relevance";
  const status =
    raw.status &&
    raw.status !== "any" &&
    SERIES_STATUS_OPTIONS.some((o) => o.value === raw.status)
      ? raw.status
      : "any";
  const publishersCsv = (raw.publisher ?? "").trim();
  const publishers = publishersCsv
    ? publishersCsv
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean)
    : [];
  return {
    sort,
    yearFrom: (raw.year_from ?? "").trim(),
    yearTo: (raw.year_to ?? "").trim(),
    status,
    publishers,
    library: (raw.library ?? "all").trim() || "all",
  };
}

/** Project the filter state into URL params. Default values are
 *  omitted so a fresh state produces a clean URL (no noise like
 *  `?sort=relevance&status=any&library=all`). */
export function seriesSearchFiltersToParams(
  state: SeriesSearchFilterState,
): Record<string, string> {
  const out: Record<string, string> = {};
  if (state.sort !== "relevance") out.sort = state.sort;
  if (state.yearFrom) out.year_from = state.yearFrom;
  if (state.yearTo) out.year_to = state.yearTo;
  if (state.status !== "any") out.status = state.status;
  if (state.publishers.length > 0) out.publisher = state.publishers.join(",");
  if (state.library !== "all") out.library = state.library;
  return out;
}

/** Project the filter state into the `SeriesListFilters` shape the
 *  underlying hook expects. Mirrors `seriesSearchFiltersToParams`
 *  closely — the names align with `/api/series` query params. */
export function seriesSearchFiltersToHook(state: SeriesSearchFilterState): {
  sort?: "name" | "created_at" | "updated_at" | "year";
  year_from?: number;
  year_to?: number;
  status?: string;
  publisher?: string;
  library?: string;
} {
  const out: ReturnType<typeof seriesSearchFiltersToHook> = {};
  if (state.sort !== "relevance") out.sort = state.sort;
  const yFrom = Number(state.yearFrom);
  const yTo = Number(state.yearTo);
  if (Number.isFinite(yFrom) && state.yearFrom) out.year_from = yFrom;
  if (Number.isFinite(yTo) && state.yearTo) out.year_to = yTo;
  if (state.status !== "any") out.status = state.status;
  if (state.publishers.length > 0) out.publisher = state.publishers.join(",");
  if (state.library !== "all") out.library = state.library;
  return out;
}

/** How many facets are active. Drives the "Filter (N)" badge so
 *  the user can see at a glance whether any non-default filter is
 *  applied. Sort doesn't count — it has its own dedicated chip. */
export function countActiveSeriesFilters(
  state: SeriesSearchFilterState,
): number {
  let n = 0;
  if (state.yearFrom || state.yearTo) n += 1;
  if (state.status !== "any") n += 1;
  if (state.publishers.length > 0) n += state.publishers.length;
  if (state.library !== "all") n += 1;
  return n;
}

function isSort(s: string | undefined): s is SeriesSearchSort {
  return SERIES_SEARCH_SORT_OPTIONS.some((o) => o.value === s);
}
