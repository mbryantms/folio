/**
 * Server-side prefetch for a rails page (the system Home today): the
 * page's pinned views and the first rail's data, dehydrated for a
 * `<HydrationBoundary>` so the client hooks start with data instead of a
 * request chain (pinned views → rail data) after hydration. See
 * "Server prefetch + HydrationBoundary" in docs/dev/saved-views.md.
 *
 * Rules this module keeps:
 * - One `QueryClient` per request (`createServerQueryClient`), never a
 *   module-level one: the data is the signed-in user's.
 * - The transport is the caller's `apiGet` (the request's cookies,
 *   `cache: "no-store"`), and the keys + query functions are the client
 *   hooks' own (`lib/api/query-options.ts`).
 * - Best effort and bounded: a failed, 401 or slow request is simply left
 *   out of the dehydrated state and the client fetches it as before; the
 *   whole prefetch gives up after `budgetMs` so a slow rail never becomes
 *   a slow first byte.
 */
import {
  QueryClient,
  dehydrate,
  type DehydratedState,
} from "@tanstack/react-query";

import {
  libraryListOptions,
  savedViewsOptions,
  type JsonFetcher,
} from "@/lib/api/query-options";

import { prefetchRail } from "./rail-queries";

/** Default ceiling on the whole prefetch. A loopback round is ~10-30 ms;
 *  the chain here is two or three of them. */
export const PAGE_RAILS_PREFETCH_BUDGET_MS = 600;

/** How many leading rails to try when the first ones are system rails
 *  that came back empty (they hide, so the next one is the first seen). */
const MAX_LEADING_RAILS = 3;

/** A fresh client for one server render. No retries: a failure falls
 *  through to the client fetch instead of delaying the response. */
export function createServerQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
}

/**
 * Prefetch the pinned views of `pageId` and the first visible rail into
 * `qc`, then dehydrate it (everything already in `qc`, such as the page
 * list, included). Only successful queries are dehydrated. Never throws.
 */
export async function prefetchPageRails(
  qc: QueryClient,
  {
    pageId,
    isSystem,
    fetch,
    budgetMs = PAGE_RAILS_PREFETCH_BUDGET_MS,
  }: {
    pageId: string;
    /** The system Home also shows the no-library card, which reads the
     *  library list; prefetched alongside. */
    isSystem: boolean;
    fetch: JsonFetcher;
    budgetMs?: number;
  },
): Promise<DehydratedState> {
  const work = async () => {
    const pinned = savedViewsOptions(fetch, { pinnedOn: pageId });
    await Promise.all([
      (async () => {
        await qc.prefetchQuery(pinned);
        const views = qc.getQueryData(pinned.queryKey)?.items ?? [];
        // Only the leading rail: the rest are lazy on purpose (`LazyRail`).
        // A system rail that loaded empty hides itself, so the next rail is
        // the one on screen; walk on in that case only.
        for (const view of views.slice(0, MAX_LEADING_RAILS)) {
          const result = await prefetchRail(qc, view, fetch);
          if (result !== "empty" || view.kind !== "system") break;
        }
      })(),
      isSystem ? qc.prefetchQuery(libraryListOptions(fetch)) : undefined,
    ]);
  };

  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      work(),
      new Promise<void>((resolve) => {
        timer = setTimeout(resolve, budgetMs);
      }),
    ]);
  } catch {
    // prefetch* never rejects; this guards a throwing `fetch` wrapper.
  } finally {
    clearTimeout(timer);
  }
  // Default filter: only `success` queries. Pending (over budget) and
  // failed ones are left for the client.
  return dehydrate(qc);
}
