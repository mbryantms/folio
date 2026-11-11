/**
 * Query options shared by the client hooks (`./queries`) and the server
 * prefetch (`lib/home/prefetch-page-rails.ts`).
 *
 * A server prefetch only saves the browser a request when the dehydrated
 * query lands under the exact key the client hook reads and holds data
 * of the exact shape the hook's `queryFn` would have produced. So the
 * key, the API path and any response transform live here once, and the
 * caller supplies only the transport: `jsonFetch` in the browser (cookie
 * session + refresh), `apiGet` on the server (the request's cookies,
 * uncached). Hook-only options (`enabled`, `placeholderData`) stay at the
 * hook.
 *
 * Imports no React and no browser API, so a server component can import
 * it; `queryOptions` / `infiniteQueryOptions` are identity helpers.
 */
import { infiniteQueryOptions, queryOptions } from "@tanstack/react-query";

import { buildQuery } from "./build-query";
import { apiPath } from "./path";
import { queryKeys } from "./query-keys";
import type { SavedViewListFilters } from "./queries";
import type {
  CblWindowPageView,
  CollectionEntriesView,
  ContinueReadingView,
  IssueListView,
  LibraryView,
  NewInSeriesView,
  OnDeckView,
  PageListView,
  RandomPicksView,
  RecentHistoryView,
  RecentIssuesView,
  SavedViewListView,
  SeriesListView,
  SimilarSeriesRailView,
} from "./types";

/** GET a JSON API path (without the `/api` prefix) and parse the body. */
export type JsonFetcher = <T>(path: string) => Promise<T>;

const nextCursor = (page: { next_cursor?: string | null }) =>
  page.next_cursor ?? undefined;

// ---------- Pages + pinned views ----------

/** The user's pages (system Home + custom). Server returns
 *  `CursorPage<PageView>`; pages are bounded per user, so the hook
 *  unwraps to `PageView[]`. */
export function mePagesOptions(fetch: JsonFetcher) {
  return queryOptions({
    queryKey: queryKeys.mePages,
    queryFn: async () => (await fetch<PageListView>("/me/pages")).items,
  });
}

export function savedViewsOptions(
  fetch: JsonFetcher,
  filters: SavedViewListFilters = {},
) {
  const params: Record<string, string> = {};
  if (typeof filters.pinned === "boolean") {
    params.pinned = String(filters.pinned);
  }
  if (filters.pinnedOn) {
    params.pinned_on = filters.pinnedOn;
  }
  return queryOptions({
    queryKey: queryKeys.savedViews(filters),
    queryFn: () =>
      fetch<SavedViewListView>(`/me/saved-views${buildQuery(params)}`),
  });
}

export function libraryListOptions(fetch: JsonFetcher) {
  return queryOptions({
    queryKey: queryKeys.libraries,
    queryFn: () => fetch<LibraryView[]>("/libraries"),
  });
}

// ---------- Home system rails ----------

export function continueReadingOptions(fetch: JsonFetcher) {
  return queryOptions({
    queryKey: queryKeys.continueReading,
    queryFn: () => fetch<ContinueReadingView>("/me/continue-reading"),
  });
}

export function onDeckOptions(fetch: JsonFetcher) {
  return queryOptions({
    queryKey: queryKeys.onDeck,
    queryFn: () => fetch<OnDeckView>("/me/on-deck"),
    staleTime: 60_000,
  });
}

export function recentIssuesOptions(fetch: JsonFetcher) {
  return queryOptions({
    queryKey: queryKeys.recentIssues,
    queryFn: () => fetch<RecentIssuesView>("/me/recent-issues"),
    staleTime: 60_000,
  });
}

export function recentHistoryOptions(fetch: JsonFetcher) {
  return queryOptions({
    queryKey: queryKeys.recentHistory,
    queryFn: () => fetch<RecentHistoryView>("/me/recent-history"),
  });
}

export function randomPicksOptions(fetch: JsonFetcher) {
  return queryOptions({
    queryKey: queryKeys.randomPicks,
    queryFn: () => fetch<RandomPicksView>("/me/random-picks"),
    staleTime: 5 * 60_000,
  });
}

/** First page = the home rail; the "see all" page walks the rest under
 *  the same cache entry. */
export function newInSeriesInfiniteOptions(fetch: JsonFetcher) {
  return infiniteQueryOptions({
    queryKey: queryKeys.newInSeries,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      fetch<NewInSeriesView>(
        `/me/new-in-series${buildQuery({ limit: 24, cursor: pageParam })}`,
      ),
    getNextPageParam: nextCursor,
    staleTime: 60_000,
  });
}

export function similarRailInfiniteOptions(fetch: JsonFetcher) {
  return infiniteQueryOptions({
    queryKey: queryKeys.similarRail,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      fetch<SimilarSeriesRailView>(
        `/me/similar-series${buildQuery({ limit: 20, cursor: pageParam })}`,
      ),
    getNextPageParam: nextCursor,
    staleTime: 60_000,
  });
}

// ---------- Saved-view rails ----------

/** A filter view's series results; no cursor = the home rail's page. */
export function savedViewResultsOptions(
  fetch: JsonFetcher,
  id: string,
  cursor?: string,
) {
  const qs = buildQuery(cursor ? { cursor } : {});
  return queryOptions({
    queryKey: queryKeys.savedViewResults(id, cursor),
    queryFn: () =>
      fetch<SeriesListView>(apiPath`/me/saved-views/${id}/results` + qs),
  });
}

/** An issue-level (`filter_issues`) view's first page (WP-5.4). */
export function savedViewIssueResultsOptions(fetch: JsonFetcher, id: string) {
  return queryOptions({
    queryKey: queryKeys.savedViewIssueResults(id),
    queryFn: () =>
      fetch<IssueListView>(apiPath`/me/saved-views/${id}/issue-results`),
  });
}

export function collectionEntriesOptions(
  fetch: JsonFetcher,
  id: string,
  opts?: { cursor?: string; limit?: number },
) {
  const params: Record<string, string> = {};
  if (opts?.cursor) params.cursor = opts.cursor;
  if (opts?.limit) params.limit = String(opts.limit);
  return queryOptions({
    queryKey: queryKeys.collectionEntries(id, opts),
    queryFn: () =>
      fetch<CollectionEntriesView>(
        apiPath`/me/collections/${id}/entries` + buildQuery(params),
      ),
  });
}

/** Page param of the bidirectional CBL window. The initial fetch carries
 *  no cursor; later pages carry the position cursor of the edge page. */
export type CblWindowPageParam =
  | { direction: "initial" }
  | { direction: "after"; cursor: number }
  | { direction: "before"; cursor: number };

/** Bidirectional infinite CBL window: the initial page anchors on the
 *  user's next-unfinished entry; later pages walk forward or backward
 *  from the edges. */
export function cblListWindowInfiniteOptions(
  fetch: JsonFetcher,
  id: string,
  opts?: { before?: number; after?: number; limit?: number },
) {
  const initialParams: Record<string, number> = {};
  if (opts?.before != null) initialParams.before = opts.before;
  if (opts?.after != null) initialParams.after = opts.after;
  const pageLimit = opts?.limit ?? 24;
  return infiniteQueryOptions({
    queryKey: queryKeys.cblListWindowInfinite(id, opts),
    initialPageParam: { direction: "initial" } as CblWindowPageParam,
    queryFn: ({ pageParam }) => {
      const params: Record<string, string | number> = {};
      if (pageParam.direction === "initial") {
        Object.assign(params, initialParams);
      } else {
        params.direction = pageParam.direction;
        params.cursor = pageParam.cursor;
        params.limit = pageLimit;
      }
      return fetch<CblWindowPageView>(
        apiPath`/me/cbl-lists/${id}/window-paginated` + buildQuery(params),
      );
    },
    getNextPageParam: (last): CblWindowPageParam | undefined =>
      last.has_more_after && last.max_position != null
        ? { direction: "after", cursor: last.max_position }
        : undefined,
    getPreviousPageParam: (first): CblWindowPageParam | undefined =>
      first.has_more_before && first.min_position != null
        ? { direction: "before", cursor: first.min_position }
        : undefined,
  });
}
