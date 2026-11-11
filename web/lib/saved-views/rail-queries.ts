/**
 * The data query behind each kind of page rail, shared by the rail bodies
 * (`components/saved-views/SavedViewRail.tsx`, `system-rails.tsx`, via the
 * hooks in `lib/api/queries.ts`) and the server prefetch
 * (`./prefetch-page-rails.ts`). The rail parameters live here so the two
 * sides cannot ask for different pages of the same data.
 */
import type { QueryClient } from "@tanstack/react-query";

import {
  cblListWindowInfiniteOptions,
  collectionEntriesOptions,
  continueReadingOptions,
  newInSeriesInfiniteOptions,
  onDeckOptions,
  randomPicksOptions,
  recentHistoryOptions,
  recentIssuesOptions,
  savedViewIssueResultsOptions,
  savedViewResultsOptions,
  similarRailInfiniteOptions,
  type JsonFetcher,
} from "@/lib/api/query-options";
import type { SavedViewView } from "@/lib/api/types";

/** Max cards a filter / collection rail shows before the trailing
 *  "View all" tile takes over. */
export const RAIL_PREVIEW_LIMIT = 30;

/** A CBL rail's reading window: 3 finished entries for context, a wider
 *  upcoming tail, 24 per extension page. */
export const CBL_RAIL_WINDOW = { before: 3, after: 24, limit: 24 } as const;

/** What prefetching a rail found: `items` = it has cards to show,
 *  `empty` = it loaded with nothing (a system rail then hides itself),
 *  `failed` = the request failed or the budget ran out, `unsupported` =
 *  this kind is not prefetched (the client fetches it as before). */
export type RailPrefetchResult = "items" | "empty" | "failed" | "unsupported";

/**
 * Prefetch one rail's first page into `qc` under the key its client hook
 * reads. Never throws: TanStack's `prefetch*` swallow errors, and a failed
 * query is left out of the dehydrated state so the client refetches it.
 *
 * Not prefetched: `on_this_day` (keyed by the *browser's* local date, which
 * the server does not know).
 */
export async function prefetchRail(
  qc: QueryClient,
  view: SavedViewView,
  fetch: JsonFetcher,
): Promise<RailPrefetchResult> {
  // Each branch prefetches, then reads the cache back through the same
  // (typed) key: absent data means the request failed.
  if (view.kind === "system") {
    switch (view.system_key) {
      case "continue_reading": {
        const o = continueReadingOptions(fetch);
        await qc.prefetchQuery(o);
        return outcome(qc.getQueryData(o.queryKey));
      }
      case "on_deck": {
        const o = onDeckOptions(fetch);
        await qc.prefetchQuery(o);
        return outcome(qc.getQueryData(o.queryKey));
      }
      case "new_issues": {
        const o = recentIssuesOptions(fetch);
        await qc.prefetchQuery(o);
        return outcome(qc.getQueryData(o.queryKey));
      }
      case "recent_history": {
        const o = recentHistoryOptions(fetch);
        await qc.prefetchQuery(o);
        return outcome(qc.getQueryData(o.queryKey));
      }
      case "random_picks": {
        const o = randomPicksOptions(fetch);
        await qc.prefetchQuery(o);
        return outcome(qc.getQueryData(o.queryKey));
      }
      case "new_in_series": {
        const o = newInSeriesInfiniteOptions(fetch);
        await qc.prefetchInfiniteQuery(o);
        return outcome(qc.getQueryData(o.queryKey)?.pages[0]);
      }
      case "similar_series": {
        const o = similarRailInfiniteOptions(fetch);
        await qc.prefetchInfiniteQuery(o);
        return outcome(qc.getQueryData(o.queryKey)?.pages[0]);
      }
      default:
        return "unsupported";
    }
  }
  if (view.kind === "filter_series") {
    const o = savedViewResultsOptions(fetch, view.id);
    await qc.prefetchQuery(o);
    return outcome(qc.getQueryData(o.queryKey));
  }
  if (view.kind === "filter_issues") {
    const o = savedViewIssueResultsOptions(fetch, view.id);
    await qc.prefetchQuery(o);
    return outcome(qc.getQueryData(o.queryKey));
  }
  if (view.kind === "collection") {
    const o = collectionEntriesOptions(fetch, view.id, {
      limit: RAIL_PREVIEW_LIMIT,
    });
    await qc.prefetchQuery(o);
    return outcome(qc.getQueryData(o.queryKey));
  }
  if (view.cbl_list_id) {
    const o = cblListWindowInfiniteOptions(
      fetch,
      view.cbl_list_id,
      CBL_RAIL_WINDOW,
    );
    await qc.prefetchInfiniteQuery(o);
    return outcome(qc.getQueryData(o.queryKey)?.pages[0]);
  }
  return "unsupported";
}

function outcome(
  page: { items: readonly unknown[] } | undefined,
): RailPrefetchResult {
  if (page === undefined) return "failed";
  return page.items.length > 0 ? "items" : "empty";
}
