"use client";

import * as React from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";

import {
  CREDIT_ROLES,
  EMPTY_CREDITS,
  parseLibraryGridFilters,
  serializeLibraryGridFilters,
  type CreditKey,
  type CreditState,
  type LibraryGridInitialFilters,
  type LibraryGridMode,
  type LibraryGridUrlState,
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
 * URL ↔ state (audit B2): the facet set + view mode are kept in sync
 * with the query string via a debounced `router.replace`, so the grid
 * is shareable and back-button-restorable. The home page no longer
 * remounts the grid on URL filter changes (its remount key dropped the
 * filter signature); instead this hook applies external navigations
 * (chip deep-links) onto its own state and writes its own changes back.
 * A single `lastUrl` ref keeps the two directions from looping. In-grid
 * search (`q`) is deliberately NOT in the URL — `?q=` on `/` routes to
 * the SearchView. View mode / sort / order persist to localStorage
 * (per-user preferences, not per-link).
 */
const STORAGE_PREFIX = "folio:grid:";
const SERIES_SORTS: readonly SeriesSort[] = [
  "name",
  "created_at",
  "updated_at",
  "year",
];
const ISSUE_SORTS: readonly IssueSort[] = [
  "number",
  "created_at",
  "updated_at",
  "year",
  "page_count",
  "user_rating",
];

function readStored(key: string): string | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage.getItem(STORAGE_PREFIX + key);
  } catch {
    return null;
  }
}
function writeStored(key: string, value: string): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(STORAGE_PREFIX + key, value);
  } catch {
    /* private mode / quota — preferences are best-effort */
  }
}

export function useLibraryGridFilters(
  libraryId: string | null,
  initialFilters: LibraryGridInitialFilters | undefined,
) {
  const init = initialFilters ?? {};
  const router = useRouter();
  const pathname = usePathname();
  const searchParams = useSearchParams();
  // The entry param is preserved verbatim across write-backs. Within a
  // mounted grid it's fixed (the page key still remounts on a library
  // switch); fall back to the prop / "all" if it's somehow absent.
  const libraryParam = searchParams.get("library") ?? libraryId ?? "all";

  const [mode, setModeState] = React.useState<LibraryGridMode>(
    init.mode ?? "series",
  );
  // Only the *debounced* query lives here. The raw per-keystroke value
  // is the toolbar's local state — when it lived in this hook, every
  // keystroke re-rendered the hook's consumer (LibraryGridView) and
  // therefore every mounted card; the 200ms debounce protected the
  // network but not the render.
  const [debouncedQ, setDebouncedQ] = React.useState("");
  const qTimer = React.useRef<ReturnType<typeof setTimeout> | null>(null);
  const setQ = React.useCallback((next: string) => {
    if (qTimer.current) clearTimeout(qTimer.current);
    qTimer.current = setTimeout(() => setDebouncedQ(next.trim()), 200);
  }, []);
  React.useEffect(
    () => () => {
      if (qTimer.current) clearTimeout(qTimer.current);
    },
    [],
  );
  // Sort state is mode-scoped: switching modes should not carry an
  // invalid sort across (e.g. `user_rating` is issue-only). We store
  // both as one union and validate before passing to the query.
  const [seriesSort, setSeriesSortState] = React.useState<SeriesSort>("name");
  const [issueSort, setIssueSortState] = React.useState<IssueSort>("number");
  const [order, setOrderState] = React.useState<SortOrder>("asc");
  // True once the user explicitly picks a sort/order — lets an explicit
  // choice override relevance ranking while a search query is active
  // (a fresh search still defaults to relevance).
  const [sortExplicit, setSortExplicit] = React.useState(false);
  const [status, setStatus] = React.useState<string>(init.status ?? "any");
  // Per-user read state (series mode only): CSV subset of
  // unread/in_progress/read; multiple values OR together.
  const [readStatus, setReadStatus] = React.useState<string[]>(
    init.readStatus ?? [],
  );
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

  // View-preference setters that also persist to localStorage; the sort
  // ones flag the choice explicit so it survives a search.
  const setMode = React.useCallback((next: LibraryGridMode) => {
    setModeState(next);
    writeStored("mode", next);
  }, []);
  const setSeriesSort = React.useCallback((next: SeriesSort) => {
    setSeriesSortState(next);
    setSortExplicit(true);
    writeStored("seriesSort", next);
  }, []);
  const setIssueSort = React.useCallback((next: IssueSort) => {
    setIssueSortState(next);
    setSortExplicit(true);
    writeStored("issueSort", next);
  }, []);
  const setOrder = React.useCallback((next: SortOrder) => {
    setOrderState(next);
    setSortExplicit(true);
    writeStored("order", next);
  }, []);

  // ── One-time localStorage seed of view preferences ──
  // These aren't carried by the URL (mode is, and the URL wins when it
  // sets one), so restore them from storage on mount. Uses the raw
  // state setters so a restored sort isn't treated as an in-session
  // explicit choice (a fresh search should still default to relevance).
  //
  // set-state-in-effect is the correct tool here: this synchronizes an
  // external system (localStorage) into React after mount. Doing it in a
  // lazy useState initializer instead would read localStorage during SSR
  // (window undefined → default) and again on hydration (stored value) →
  // a hydration mismatch. The one-time `seeded` guard bounds it to a
  // single post-mount correction.
  const seeded = React.useRef(false);
  React.useEffect(() => {
    if (seeded.current) return;
    seeded.current = true;
    /* eslint-disable react-hooks/set-state-in-effect -- syncing localStorage prefs in on mount; see note above */
    const sSort = readStored("seriesSort");
    if (sSort && (SERIES_SORTS as string[]).includes(sSort)) {
      setSeriesSortState(sSort as SeriesSort);
    }
    const iSort = readStored("issueSort");
    if (iSort && (ISSUE_SORTS as string[]).includes(iSort)) {
      setIssueSortState(iSort as IssueSort);
    }
    const ord = readStored("order");
    if (ord === "asc" || ord === "desc") setOrderState(ord);
    if (!init.mode) {
      const m = readStored("mode");
      if (m === "series" || m === "issues") setModeState(m);
    }
    /* eslint-enable react-hooks/set-state-in-effect */
  }, [init.mode]);

  const trimmedQ = debouncedQ;

  // Filters shared by both modes — assembled once, then split per
  // endpoint below. `status` is series-only (issues don't carry
  // status) so it's left out of the issues filter shape.
  const sharedFilters = {
    library: libraryId ?? undefined,
    q: trimmedQ || undefined,
    // Explicit sort overrides relevance even when searching (server
    // honours `sort` over `ts_rank` when both are present); otherwise a
    // search defaults to relevance ranking (sort/order omitted).
    order: trimmedQ && !sortExplicit ? undefined : order,
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
    sort: trimmedQ && !sortExplicit ? undefined : seriesSort,
    status: status === "any" ? undefined : status,
    // read_status is a per-series rollup — series mode only.
    read_status: csvOrUndef(readStatus),
  };
  const issueFilters: IssuesCrossListFilters = {
    ...sharedFilters,
    sort: trimmedQ && !sortExplicit ? undefined : issueSort,
  };

  // ── URL ↔ state sync ──
  const urlState: LibraryGridUrlState = {
    library: libraryParam,
    mode,
    status,
    readStatus,
    yearFrom,
    yearTo,
    publishers,
    languages,
    ageRatings,
    genres,
    tags,
    credits,
    anyCredits,
    characters,
    teams,
    locations,
    ratingRange,
  };
  const serialized = serializeLibraryGridFilters(urlState);
  // Canonical serialization of the *current* URL, for comparison. Built
  // through the same parse→serialize round-trip so param order / casing
  // can't cause a spurious mismatch.
  const incoming = React.useMemo(() => {
    const raw: Record<string, string | undefined> = {};
    searchParams.forEach((v, k) => {
      raw[k] = v;
    });
    raw.library = libraryParam;
    return serializeLibraryGridFilters(
      urlStateFromParsed(libraryParam, mode, parseLibraryGridFilters(raw)),
    );
    // `mode` participates because the parser reads it; including it keeps
    // an external `?mode=` change applying. Intentionally excludes the
    // live facet state so this only reflects the URL.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams, libraryParam]);

  // Single ref tracks the last URL we either wrote or observed-and-
  // applied, so the write-back and apply effects can't ping-pong.
  const lastUrl = React.useRef<string | null>(null);
  const urlTimer = React.useRef<ReturnType<typeof setTimeout> | null>(null);

  // Write-back: state → URL (debounced). Skips when the URL already
  // matches state (mount, or an external nav we just applied).
  React.useEffect(() => {
    if (lastUrl.current === null) {
      lastUrl.current = serialized;
      return;
    }
    if (serialized === lastUrl.current) return;
    if (urlTimer.current) clearTimeout(urlTimer.current);
    urlTimer.current = setTimeout(() => {
      lastUrl.current = serialized;
      router.replace(`${pathname}?${serialized}`, { scroll: false });
    }, 300);
    return () => {
      if (urlTimer.current) clearTimeout(urlTimer.current);
    };
  }, [serialized, pathname, router]);

  // Apply external navigation: URL → state. Fires only when the URL
  // changed to something we didn't write (a chip deep-link landing while
  // the grid is already mounted).
  React.useEffect(() => {
    if (lastUrl.current === null) {
      lastUrl.current = incoming;
      return;
    }
    if (incoming === lastUrl.current) return;
    lastUrl.current = incoming;
    const raw: Record<string, string | undefined> = {};
    searchParams.forEach((v, k) => {
      raw[k] = v;
    });
    applyParsedToState(parseLibraryGridFilters(raw) ?? {}, {
      setModeState,
      setStatus,
      setReadStatus,
      setYearFrom,
      setYearTo,
      setPublishers,
      setLanguages,
      setAgeRatings,
      setGenres,
      setTags,
      setCredits,
      setAnyCredits,
      setCharacters,
      setTeams,
      setLocations,
      setRatingRange,
    });
    // searchParams is the trigger; the setters are stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [incoming]);

  const creditCount = CREDIT_ROLES.reduce(
    (sum, c) => sum + credits[c.key].length,
    0,
  );
  const facetCount =
    (status !== "any" ? 1 : 0) +
    readStatus.length +
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
    setReadStatus([]);
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
    // Mode + search + sort. `q` is the debounced value — the live
    // input string is the toolbar's own state (see note at the top).
    mode,
    setMode,
    q: debouncedQ,
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
    readStatus,
    setReadStatus,
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

/** Build a `LibraryGridUrlState` from a parsed initial-filters object,
 *  for canonical comparison of the current URL against live state. */
function urlStateFromParsed(
  library: string,
  fallbackMode: LibraryGridMode,
  parsed: LibraryGridInitialFilters | undefined,
): LibraryGridUrlState {
  const p = parsed ?? {};
  return {
    library,
    mode: p.mode ?? fallbackMode,
    status: p.status,
    readStatus: p.readStatus ?? [],
    yearFrom: p.yearFrom,
    yearTo: p.yearTo,
    publishers: p.publishers ?? [],
    languages: p.languages ?? [],
    ageRatings: p.ageRatings ?? [],
    genres: p.genres ?? [],
    tags: p.tags ?? [],
    credits: { ...EMPTY_CREDITS, ...(p.credits ?? {}) },
    anyCredits: p.anyCredits ?? [],
    characters: p.characters ?? [],
    teams: p.teams ?? [],
    locations: p.locations ?? [],
    ratingRange: p.ratingRange ?? null,
  };
}

type FacetSetters = {
  setModeState: (m: LibraryGridMode) => void;
  setStatus: (v: string) => void;
  setReadStatus: (v: string[]) => void;
  setYearFrom: (v: string) => void;
  setYearTo: (v: string) => void;
  setPublishers: (v: string[]) => void;
  setLanguages: (v: string[]) => void;
  setAgeRatings: (v: string[]) => void;
  setGenres: (v: string[]) => void;
  setTags: (v: string[]) => void;
  setCredits: (v: CreditState) => void;
  setAnyCredits: (v: string[]) => void;
  setCharacters: (v: string[]) => void;
  setTeams: (v: string[]) => void;
  setLocations: (v: string[]) => void;
  setRatingRange: (v: [number, number] | null) => void;
};

/** Apply a parsed URL filter set onto the hook's state, resetting any
 *  dimension the URL omits back to its default — so navigating from a
 *  filtered link to a bare one clears stale facets. */
function applyParsedToState(
  p: LibraryGridInitialFilters,
  s: FacetSetters,
): void {
  if (p.mode) s.setModeState(p.mode);
  s.setStatus(p.status ?? "any");
  s.setReadStatus(p.readStatus ?? []);
  s.setYearFrom(p.yearFrom ?? "");
  s.setYearTo(p.yearTo ?? "");
  s.setPublishers(p.publishers ?? []);
  s.setLanguages(p.languages ?? []);
  s.setAgeRatings(p.ageRatings ?? []);
  s.setGenres(p.genres ?? []);
  s.setTags(p.tags ?? []);
  s.setCredits({ ...EMPTY_CREDITS, ...(p.credits ?? {}) });
  s.setAnyCredits(p.anyCredits ?? []);
  s.setCharacters(p.characters ?? []);
  s.setTeams(p.teams ?? []);
  s.setLocations(p.locations ?? []);
  s.setRatingRange(p.ratingRange ?? null);
}

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
