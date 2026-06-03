"use client";

import * as React from "react";

import {
  CREDIT_ROLES,
  EMPTY_CREDITS,
  type CreditKey,
  type CreditState,
  type LibraryGridInitialFilters,
  type LibraryGridMode,
} from "@/components/library/library-grid-filters";
import type {
  IssuesCrossListFilters,
  SeriesListFilters,
} from "@/lib/api/queries";
import type { IssueSort, SeriesSort, SortOrder } from "@/lib/api/types";

/**
 * Hook that owns the library-grid's facet state, the search-input
 * debounce, and the per-mode server-filter assembly. Extracted from
 * the monolithic `LibraryGridView.tsx` in audit-remediation M7.3 to
 * keep the rendering component focused on layout.
 *
 * Lifecycle: callers pass `initialFilters` (URL-derived; see
 * `parseLibraryGridFilters`) once on mount. After that the hook owns
 * the state — flipping URL params doesn't reset the grid.
 */
export function useLibraryGridFilters(
  libraryId: string | null,
  initialFilters: LibraryGridInitialFilters | undefined,
) {
  const init = initialFilters ?? {};

  const [mode, setMode] = React.useState<LibraryGridMode>(
    init.mode ?? "series",
  );
  const [q, setQ] = React.useState("");
  const [debouncedQ, setDebouncedQ] = React.useState("");
  // Sort state is mode-scoped: switching modes should not carry an
  // invalid sort across (e.g. `user_rating` is issue-only). We store
  // both as one union and validate before passing to the query.
  const [seriesSort, setSeriesSort] = React.useState<SeriesSort>("name");
  const [issueSort, setIssueSort] = React.useState<IssueSort>("number");
  const [order, setOrder] = React.useState<SortOrder>("asc");
  const [status, setStatus] = React.useState<string>(init.status ?? "any");
  const [yearFrom, setYearFrom] = React.useState<string>(init.yearFrom ?? "");
  const [yearTo, setYearTo] = React.useState<string>(init.yearTo ?? "");
  const [publishers, setPublishers] = React.useState<string[]>(
    init.publishers ?? [],
  );
  const [languages, setLanguages] = React.useState<string[]>(
    init.languages ?? [],
  );
  const [ageRatings, setAgeRatings] = React.useState<string[]>(
    init.ageRatings ?? [],
  );
  const [genres, setGenres] = React.useState<string[]>(init.genres ?? []);
  const [tags, setTags] = React.useState<string[]>(init.tags ?? []);
  const [credits, setCredits] = React.useState<CreditState>(() => ({
    ...EMPTY_CREDITS,
    ...(init.credits ?? {}),
  }));
  const [anyCredits, setAnyCredits] = React.useState<string[]>(
    init.anyCredits ?? [],
  );
  const [characters, setCharacters] = React.useState<string[]>(
    init.characters ?? [],
  );
  const [teams, setTeams] = React.useState<string[]>(init.teams ?? []);
  const [locations, setLocations] = React.useState<string[]>(
    init.locations ?? [],
  );
  const [ratingRange, setRatingRange] = React.useState<[number, number] | null>(
    init.ratingRange ?? null,
  );

  React.useEffect(() => {
    const t = setTimeout(() => setDebouncedQ(q.trim()), 200);
    return () => clearTimeout(t);
  }, [q]);

  const trimmedQ = debouncedQ;

  // Filters shared by both modes — assembled once, then split per
  // endpoint below. `status` is series-only (issues don't carry
  // status) so it's left out of the issues filter shape.
  const sharedFilters = {
    library: libraryId ?? undefined,
    q: trimmedQ || undefined,
    order: trimmedQ ? undefined : order,
    year_from: parseYear(yearFrom),
    year_to: parseYear(yearTo),
    publisher: csvOrUndef(publishers),
    language: csvOrUndef(languages),
    age_rating: csvOrUndef(ageRatings),
    genres: csvOrUndef(genres),
    tags: csvOrUndef(tags),
    writers: csvOrUndef(credits.writers),
    pencillers: csvOrUndef(credits.pencillers),
    inkers: csvOrUndef(credits.inkers),
    colorists: csvOrUndef(credits.colorists),
    letterers: csvOrUndef(credits.letterers),
    cover_artists: csvOrUndef(credits.cover_artists),
    editors: csvOrUndef(credits.editors),
    translators: csvOrUndef(credits.translators),
    credits: csvOrUndef(anyCredits),
    characters: csvOrUndef(characters),
    teams: csvOrUndef(teams),
    locations: csvOrUndef(locations),
    user_rating_min: ratingRange?.[0],
    user_rating_max: ratingRange?.[1],
    limit: 60,
  };

  const seriesFilters: SeriesListFilters = {
    ...sharedFilters,
    sort: trimmedQ ? undefined : seriesSort,
    status: status === "any" ? undefined : status,
  };
  const issueFilters: IssuesCrossListFilters = {
    ...sharedFilters,
    sort: trimmedQ ? undefined : issueSort,
  };

  const creditCount = CREDIT_ROLES.reduce(
    (sum, c) => sum + credits[c.key].length,
    0,
  );
  const facetCount =
    (status !== "any" ? 1 : 0) +
    (yearFrom || yearTo ? 1 : 0) +
    (ratingRange ? 1 : 0) +
    publishers.length +
    languages.length +
    ageRatings.length +
    genres.length +
    tags.length +
    creditCount +
    anyCredits.length +
    characters.length +
    teams.length +
    locations.length;

  function clearFacets() {
    setStatus("any");
    setYearFrom("");
    setYearTo("");
    setRatingRange(null);
    setPublishers([]);
    setLanguages([]);
    setAgeRatings([]);
    setGenres([]);
    setTags([]);
    setCredits(EMPTY_CREDITS);
    setAnyCredits([]);
    setCharacters([]);
    setTeams([]);
    setLocations([]);
  }

  function setCreditRole(key: CreditKey, values: string[]) {
    setCredits((prev) => ({ ...prev, [key]: values }));
  }

  return {
    // Mode + search + sort
    mode,
    setMode,
    q,
    setQ,
    trimmedQ,
    seriesSort,
    setSeriesSort,
    issueSort,
    setIssueSort,
    order,
    setOrder,
    // Facets (state + setters)
    status,
    setStatus,
    yearFrom,
    setYearFrom,
    yearTo,
    setYearTo,
    publishers,
    setPublishers,
    languages,
    setLanguages,
    ageRatings,
    setAgeRatings,
    genres,
    setGenres,
    tags,
    setTags,
    credits,
    setCreditRole,
    anyCredits,
    setAnyCredits,
    characters,
    setCharacters,
    teams,
    setTeams,
    locations,
    setLocations,
    ratingRange,
    setRatingRange,
    // Derived
    facetCount,
    seriesFilters,
    issueFilters,
    // Helpers
    clearFacets,
  };
}

export type LibraryGridFiltersHookValue = ReturnType<
  typeof useLibraryGridFilters
>;

function csvOrUndef(values: string[]): string | undefined {
  return values.length ? values.join(",") : undefined;
}

function parseYear(raw: string): number | undefined {
  const trimmed = raw.trim();
  if (!trimmed) return undefined;
  if (!/^\d{1,4}$/.test(trimmed)) return undefined;
  const n = Number(trimmed);
  return Number.isFinite(n) ? n : undefined;
}
