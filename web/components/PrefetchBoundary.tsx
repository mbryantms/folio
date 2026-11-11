"use client";

import {
  HydrationBoundary,
  hydrate,
  useQueryClient,
  type DehydratedState,
} from "@tanstack/react-query";
import * as React from "react";

/** A replayed snapshot older than this (first seen by this browser) is
 *  not hydrated: its data predates whatever happened since. */
export const SNAPSHOT_FRESH_MS = 30_000;
/** Snapshot ids remembered per tab; the oldest is forgotten first. */
const MAX_REMEMBERED_SNAPSHOTS = 64;
const firstSeen = new Map<string, number>();

/**
 * When this browser first saw snapshot `id` (its own clock), recording
 * `now` on the first call. Module-level on purpose: it must outlive the
 * page's components, since a back/forward replays the same id into a new
 * mount. Bounded; ids are random per server render and carry no data.
 */
export function snapshotFirstSeen(id: string, now: number): number {
  const seen = firstSeen.get(id);
  if (seen !== undefined) return seen;
  firstSeen.set(id, now);
  if (firstSeen.size > MAX_REMEMBERED_SNAPSHOTS) {
    const oldest = firstSeen.keys().next().value;
    if (oldest !== undefined) firstSeen.delete(oldest);
  }
  return now;
}

const NOTHING: DehydratedState = { mutations: [], queries: [] };

/**
 * What to hydrate from a server snapshot, decided on the browser's clock
 * only (never compared with the server's):
 *
 * - First seen more than `SNAPSHOT_FRESH_MS` ago: nothing. Next replays a
 *   page's cached RSC payload on back/forward (the reader exits with
 *   `router.back()`), and once the rail queries have been collected the
 *   stock boundary would hydrate that old snapshot as new data — Home
 *   would show the pre-reading Continue reading until the refetch landed.
 *   Skipping it leaves the hooks to fetch, as before the prefetch existed.
 * - Otherwise: the snapshot with every query's `dataUpdatedAt` restamped
 *   to the moment this browser first saw it. Staleness and the
 *   newer-than-cache test then run on one clock, so a server clock that is
 *   ahead can neither overwrite newer client data nor suppress a refetch.
 *   An optimistic update in flight when Home is opened can be replaced by
 *   the snapshot (fetched before the write landed); the mutation's
 *   `invalidateRails` on settle refetches it.
 */
export function usableSnapshot(
  state: DehydratedState,
  snapshotId: string,
  now: number = Date.now(),
): DehydratedState {
  const seenAt = snapshotFirstSeen(snapshotId, now);
  if (now - seenAt > SNAPSHOT_FRESH_MS) return NOTHING;
  return {
    ...state,
    queries: state.queries.map((q) => ({
      ...q,
      dehydratedAt: seenAt,
      state: { ...q.state, dataUpdatedAt: seenAt },
    })),
  };
}

/**
 * `<HydrationBoundary>` for server-prefetched page data (D18), plus two
 * things the stock boundary does not do: refuse a stale replayed snapshot
 * and restamp a fresh one to the browser's clock (`usableSnapshot`), and
 * hydrate placeholder entries during render.
 *
 * The stock boundary hydrates queries that are *new* to the cache during
 * render, but defers queries that already *exist* to an effect after
 * commit, so it never updates a component that is already showing them.
 * The topbar search creates the Continue reading entry before the page
 * renders, without data: `useSearchActions`
 * (`lib/search/use-search-actions.ts`) calls `useContinueReading` with
 * `enabled: active && !!me.data`, disabled until the search is used. That
 * empty entry counts as existing, so the prefetched Continue reading rail
 * rendered its skeleton on the server and on hydration, then requested
 * the data it had been handed.
 *
 * So a dehydrated query whose cache entry exists but has no data and no
 * request in flight is hydrated during render, exactly like a new one.
 * Nothing is showing data for it yet, only a disabled or loading state,
 * so there is no shown value to tear; and an observer that is already
 * subscribed (the shell hydrates and commits before the page's Suspense
 * boundary) hears about it through TanStack's batched, asynchronous
 * notify, never synchronously inside this render. Everything else
 * (entries with data, in-flight fetches) goes through the stock boundary.
 */
export function PrefetchBoundary({
  state,
  snapshotId,
  children,
}: {
  state: DehydratedState;
  snapshotId: string;
  children: React.ReactNode;
}) {
  const qc = useQueryClient();
  const usable = React.useMemo(() => {
    // Server render: the client is per request; hydrate as given.
    if (typeof window === "undefined") return state;
    const snapshot = usableSnapshot(state, snapshotId);
    const cache = qc.getQueryCache();
    const placeholders = snapshot.queries.filter((q) => {
      const existing = cache.get(q.queryHash);
      return (
        existing !== undefined &&
        existing.state.data === undefined &&
        existing.state.fetchStatus === "idle"
      );
    });
    if (placeholders.length > 0) hydrate(qc, { queries: placeholders });
    return snapshot;
  }, [qc, state, snapshotId]);
  return <HydrationBoundary state={usable}>{children}</HydrationBoundary>;
}
