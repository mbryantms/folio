/**
 * The query hooks the reader's first load uses: the series and list it
 * reads through (finishing progress, list context), its markers, its
 * spread overrides and the next-up / prev-up resolvers.
 *
 * A shard of `queries.ts` (re-exported from there, so library pages keep
 * importing the barrel): `Reader.tsx` and its first-load hooks import from
 * here so the reader does not ship every other hook in `queries.ts` (see
 * `json-fetch.ts` and `docs/dev/pwa-performance.md`).
 */
import { useQuery } from "@tanstack/react-query";

import type { IssueUrlOpts } from "@/lib/urls";

import { buildQuery } from "./build-query";
import { HttpError, jsonFetch } from "./json-fetch";
import { apiPath } from "./path";
import { queryKeys } from "./query-keys";
import type {
  CblDetailView,
  IssueMarkersView,
  NextUpView,
  PageOverridesView,
  SavedViewListView,
  SeriesView,
} from "./types";

export function useSeries(
  id: string,
  opts: {
    /** The server-rendered series, so a page that already has it reads
     *  the same cache entry progress writes invalidate (series page
     *  read-progress stats) without a second fetch on mount. */
    initialData?: SeriesView;
  } = {},
) {
  return useQuery({
    queryKey: queryKeys.series(id),
    queryFn: () => jsonFetch<SeriesView>(apiPath`/series/${id}`),
    enabled: !!id,
    initialData: opts.initialData,
  });
}

export function useSavedView(id: string) {
  return useQuery({
    queryKey: queryKeys.savedView(id),
    queryFn: async () => {
      const list = await jsonFetch<SavedViewListView>("/me/saved-views");
      const found = list.items.find((v) => v.id === id);
      if (!found) throw new HttpError(404, `saved view ${id} not found`);
      return found;
    },
    enabled: !!id,
  });
}

export function useCblList(id: string) {
  return useQuery({
    queryKey: queryKeys.cblList(id),
    queryFn: () => jsonFetch<CblDetailView>(apiPath`/me/cbl-lists/${id}`),
    enabled: !!id,
  });
}

/** Reader overlay's one-shot fetch — every marker the caller has on
 *  this issue, across all pages, no pagination. */
export function useIssueMarkers(issueId: string) {
  return useQuery({
    queryKey: queryKeys.issueMarkers(issueId),
    queryFn: () =>
      jsonFetch<IssueMarkersView>(apiPath`/me/issues/${issueId}/markers`),
    enabled: !!issueId,
  });
}

/** WP-4.3 — the caller's manual spread controls for one issue (force
 *  spread / force single / shift pairing). `initialData` is the SSR
 *  prefetch from the read page so the first paint already pairs with
 *  the overrides applied; `staleTime: Infinity` because only this user's
 *  own mutations change it, and those write the cache directly.
 *
 *  Offline (audit N-R11) the one request is still fired — the offline
 *  reader's stored copy answers when it fails — but never retried: the
 *  default `online` network mode would park the query and the default
 *  retry would back off three times before the fallback could apply. */
export function useIssuePageOverrides(
  issueId: string,
  initialData?: PageOverridesView | null,
) {
  const offline =
    typeof navigator !== "undefined" && navigator.onLine === false;
  return useQuery({
    queryKey: queryKeys.issuePageOverrides(issueId),
    queryFn: () =>
      jsonFetch<PageOverridesView>(
        apiPath`/me/issues/${issueId}/page-overrides`,
      ),
    enabled: !!issueId,
    initialData: initialData ?? undefined,
    staleTime: Infinity,
    ...(offline ? { retry: false, networkMode: "offlineFirst" as const } : {}),
  });
}

/** Per-issue "what should I read next?" resolver — drives the reader's
 *  `Shift+N` keybind and the end-of-issue card (M4). Picks collection >
 *  CBL > series > none. Pass the list the user is reading through via
 *  `ctx` (`collection` = collection saved-view id, `cbl` = CBL saved-view
 *  id); omit it for series-only resolution.
 *
 *  Cached for 5 minutes — progress flips that move the resolved target
 *  show up via the existing progress-mutation invalidation chain on the
 *  *next* read; not worth a tighter loop since the reader is mostly
 *  read-only during a single session. */
export function useNextUp(
  issueId: string,
  ctx?: IssueUrlOpts | null,
  opts?: { enabled?: boolean },
) {
  const enabled = (opts?.enabled ?? true) && issueId.length > 0;
  return useQuery({
    queryKey: queryKeys.nextUp(issueId, ctx),
    queryFn: () =>
      jsonFetch<NextUpView>(
        apiPath`/issues/${issueId}/next-up` + listContextQuery(ctx),
      ),
    enabled,
    staleTime: 5 * 60 * 1000,
  });
}

/** `?cbl=…&collection=…` for the next-up / prev-up resolvers; empty when
 *  the reader has no list context. */
function listContextQuery(ctx?: IssueUrlOpts | null): string {
  const qs = buildQuery({
    cbl: ctx?.cbl ?? undefined,
    collection: ctx?.collection ?? undefined,
  });
  return qs;
}

/** Per-issue "what came before?" resolver — sibling to `useNextUp` for
 *  the reader's `Shift+P` keybind. Picks CBL > series > none. Pure
 *  sequential navigation; doesn't filter by finished state (a user
 *  pressing `Shift+P` is asking to back up one step, not to find an
 *  unread issue). `fallback_suggestion` is never populated for prev. */
export function usePrevUp(
  issueId: string,
  ctx?: IssueUrlOpts | null,
  opts?: { enabled?: boolean },
) {
  const enabled = (opts?.enabled ?? true) && issueId.length > 0;
  return useQuery({
    queryKey: queryKeys.prevUp(issueId, ctx),
    queryFn: () =>
      jsonFetch<NextUpView>(
        apiPath`/issues/${issueId}/prev-up` + listContextQuery(ctx),
      ),
    enabled,
    staleTime: 5 * 60 * 1000,
  });
}
