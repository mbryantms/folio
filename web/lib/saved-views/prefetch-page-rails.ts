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
import { newSnapshotId } from "@/lib/saved-views/snapshot-id";

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

/** A GET that can be cancelled: `apiGet` (`lib/api/fetch.ts`). */
export type CancellableFetcher = <T>(
  path: string,
  init?: { signal?: AbortSignal },
) => Promise<T>;

/** What the page hands to `<PrefetchBoundary>` and `<PageRails>`. */
export type PrefetchedPageRails = {
  state: DehydratedState;
  /** Random per render. The browser remembers when it first saw each id
   *  and refuses to hydrate an old snapshot that Next replays from its
   *  router cache on back/forward (`PrefetchBoundary`). */
  snapshotId: string;
  /** The rail the prefetch settled on (the first one that will show), so
   *  the page renders *that* rail eagerly. `null` when no rail was
   *  prefetched: the page then prioritizes its first rail, as before. */
  priorityViewId: string | null;
};

/**
 * Prefetch the pinned views of `pageId` and the first visible rail into
 * `qc`, then dehydrate it (everything already in `qc`, such as the page
 * list, included). Only successful queries are dehydrated. Never throws.
 *
 * At the budget every request still in flight is aborted and no further
 * one starts: nothing reads `qc` after the response has gone.
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
    fetch: CancellableFetcher;
    budgetMs?: number;
  },
): Promise<PrefetchedPageRails> {
  const budget = new AbortController();
  const guarded: JsonFetcher = <T>(path: string) =>
    budget.signal.aborted
      ? Promise.reject<T>(new Error("prefetch budget spent"))
      : fetch<T>(path, { signal: budget.signal });
  let priorityViewId: string | null = null;

  const work = async () => {
    const pinned = savedViewsOptions(guarded, { pinnedOn: pageId });
    await Promise.all([
      (async () => {
        await qc.prefetchQuery(pinned);
        const views = qc.getQueryData(pinned.queryKey)?.items ?? [];
        // Only the leading rail: the rest are lazy on purpose (`LazyRail`).
        // A system rail that loaded empty hides itself, so the next rail is
        // the one on screen; walk on in that case only.
        for (const view of views.slice(0, MAX_LEADING_RAILS)) {
          if (budget.signal.aborted) return;
          const result = await prefetchRail(qc, view, guarded);
          if (
            result === "items" ||
            (result === "empty" && view.kind !== "system")
          ) {
            priorityViewId = view.id;
          }
          if (result !== "empty" || view.kind !== "system") break;
        }
      })(),
      isSystem ? qc.prefetchQuery(libraryListOptions(guarded)) : undefined,
    ]);
  };

  let timer: ReturnType<typeof setTimeout> | undefined;
  let finished = false;
  try {
    await Promise.race([
      work().then(() => {
        finished = true;
      }),
      new Promise<void>((resolve) => {
        timer = setTimeout(resolve, budgetMs);
      }),
    ]);
  } catch {
    // prefetch* never rejects; this guards a throwing `fetch` wrapper.
  } finally {
    clearTimeout(timer);
    budget.abort();
  }
  // Default filter: only `success` queries. Pending (over budget) and
  // failed ones are left for the client.
  return {
    state: dehydrate(qc),
    snapshotId: newSnapshotId(),
    // Over budget the settled rail may not be the one that shows; fall
    // back to the page's own first-rail rule.
    priorityViewId: finished ? priorityViewId : null,
  };
}
