/**
 * Server prefetch for a rails page (D18). The dehydrated queries are only
 * worth anything if they land under the exact keys the client hooks read,
 * so every assertion compares against the `queryKeys` registry (the same
 * builders the hooks use, via `lib/api/query-options.ts`). A failing,
 * unauthorised or slow request must leave the state empty for that query,
 * never throw.
 */
import { hashKey } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";

import { queryKeys } from "@/lib/api/query-keys";
import type { SavedViewView } from "@/lib/api/types";
import {
  createServerQueryClient,
  prefetchPageRails,
} from "@/lib/saved-views/prefetch-page-rails";
import {
  CBL_RAIL_WINDOW,
  RAIL_PREVIEW_LIMIT,
} from "@/lib/saved-views/rail-queries";

const PAGE = "page-1";

function view(partial: Partial<SavedViewView>): SavedViewView {
  return {
    id: "v-1",
    name: "View",
    kind: "filter_series",
    system_key: null,
    cbl_list_id: null,
    ...partial,
  } as SavedViewView;
}

class ApiError extends Error {
  constructor(public status: number) {
    super(`status ${status}`);
  }
}

/** A fake `apiGet`: a path → body table; anything else is a 404. */
function fakeFetch(routes: Record<string, unknown>) {
  const calls: string[] = [];
  const fetch = vi.fn(async <T>(path: string): Promise<T> => {
    calls.push(path);
    if (!(path in routes)) throw new ApiError(404);
    const body = routes[path];
    if (body instanceof Error) throw body;
    return body as T;
  });
  return { fetch: fetch as <T>(path: string) => Promise<T>, calls };
}

const hashes = (state: { queries: { queryHash: string }[] }) =>
  state.queries.map((q) => q.queryHash).sort();

const pinnedPath = `/me/saved-views?pinned_on=${PAGE}`;

describe("prefetchPageRails", () => {
  it("dehydrates the pinned views, the library list and the first rail under the hooks' keys", async () => {
    const { fetch, calls } = fakeFetch({
      [pinnedPath]: {
        items: [
          view({ id: "s1", kind: "system", system_key: "continue_reading" }),
          view({ id: "s2", kind: "system", system_key: "on_deck" }),
        ],
      },
      "/libraries": [{ id: "lib" }],
      "/me/continue-reading": { items: [{ issue: { id: "i1" } }] },
    });
    const { state } = await prefetchPageRails(createServerQueryClient(), {
      pageId: PAGE,
      isSystem: true,
      fetch,
    });
    expect(hashes(state)).toEqual(
      [
        hashKey(queryKeys.savedViews({ pinnedOn: PAGE })),
        hashKey(queryKeys.libraries),
        hashKey(queryKeys.continueReading),
      ].sort(),
    );
    // Only the leading rail: On deck stays the client's (LazyRail).
    expect(calls).not.toContain("/me/on-deck");
  });

  it("walks past a leading system rail that loaded empty (it hides itself)", async () => {
    const { fetch, calls } = fakeFetch({
      [pinnedPath]: {
        items: [
          view({ id: "s1", kind: "system", system_key: "continue_reading" }),
          view({ id: "s2", kind: "system", system_key: "on_deck" }),
          view({ id: "f1", kind: "filter_series" }),
        ],
      },
      "/me/continue-reading": { items: [] },
      "/me/on-deck": { items: [{ kind: "series_next" }] },
    });
    const { state } = await prefetchPageRails(createServerQueryClient(), {
      pageId: PAGE,
      isSystem: false,
      fetch,
    });
    expect(hashes(state)).toContain(hashKey(queryKeys.onDeck));
    expect(calls).not.toContain("/me/saved-views/f1/results");
  });

  it.each([
    [
      "filter_series",
      view({ id: "f1", kind: "filter_series" }),
      "/me/saved-views/f1/results",
      queryKeys.savedViewResults("f1", undefined),
      { items: [{ id: "s" }] },
    ],
    [
      "filter_issues",
      view({ id: "f2", kind: "filter_issues" }),
      "/me/saved-views/f2/issue-results",
      queryKeys.savedViewIssueResults("f2"),
      { items: [{ id: "i" }] },
    ],
    [
      "collection",
      view({ id: "c1", kind: "collection" }),
      `/me/collections/c1/entries?limit=${RAIL_PREVIEW_LIMIT}`,
      queryKeys.collectionEntries("c1", { limit: RAIL_PREVIEW_LIMIT }),
      { items: [{ id: "e" }] },
    ],
    [
      "cbl",
      view({ id: "l1", kind: "cbl", cbl_list_id: "cbl-1" }),
      "/me/cbl-lists/cbl-1/window-paginated?before=3&after=24",
      queryKeys.cblListWindowInfinite("cbl-1", CBL_RAIL_WINDOW),
      { items: [{ position: 1 }] },
    ],
    [
      "new_issues",
      view({ id: "s", kind: "system", system_key: "new_issues" }),
      "/me/recent-issues",
      queryKeys.recentIssues,
      { items: [{ id: "i" }] },
    ],
    [
      "new_in_series (infinite)",
      view({ id: "s", kind: "system", system_key: "new_in_series" }),
      "/me/new-in-series?limit=24",
      queryKeys.newInSeries,
      { items: [{ id: "i" }], next_cursor: null },
    ],
    [
      "similar_series (infinite)",
      view({ id: "s", kind: "system", system_key: "similar_series" }),
      "/me/similar-series?limit=20",
      queryKeys.similarRail,
      { items: [{ id: "s" }], next_cursor: null },
    ],
  ])(
    "prefetches a %s rail from the hook's path under the hook's key",
    async (_kind, first, path, key, body) => {
      const { fetch } = fakeFetch({
        [pinnedPath]: { items: [first] },
        [path]: body,
      });
      const { state } = await prefetchPageRails(createServerQueryClient(), {
        pageId: PAGE,
        isSystem: false,
        fetch,
      });
      expect(hashes(state)).toContain(hashKey(key));
    },
  );

  it("an infinite rail dehydrates as an infinite query with the initial page param", async () => {
    const { fetch } = fakeFetch({
      [pinnedPath]: {
        items: [view({ kind: "cbl", cbl_list_id: "cbl-1" })],
      },
      "/me/cbl-lists/cbl-1/window-paginated?before=3&after=24": {
        items: [{ position: 1 }],
      },
    });
    const { state } = await prefetchPageRails(createServerQueryClient(), {
      pageId: PAGE,
      isSystem: false,
      fetch,
    });
    const q = state.queries.find(
      (x) =>
        x.queryHash ===
        hashKey(queryKeys.cblListWindowInfinite("cbl-1", CBL_RAIL_WINDOW)),
    );
    expect(q?.state.data).toEqual({
      pages: [{ items: [{ position: 1 }] }],
      pageParams: [{ direction: "initial" }],
    });
  });

  it("does not prefetch On this day (keyed by the browser's local date)", async () => {
    const { fetch, calls } = fakeFetch({
      [pinnedPath]: {
        items: [view({ kind: "system", system_key: "on_this_day" })],
      },
    });
    await prefetchPageRails(createServerQueryClient(), {
      pageId: PAGE,
      isSystem: false,
      fetch,
    });
    expect(calls.some((c) => c.startsWith("/me/on-this-day"))).toBe(false);
  });

  it("a 401 everywhere yields an empty dehydrated state, not a throw", async () => {
    const { fetch } = fakeFetch({
      [pinnedPath]: new ApiError(401),
      "/libraries": new ApiError(401),
    });
    const { state } = await prefetchPageRails(createServerQueryClient(), {
      pageId: PAGE,
      isSystem: true,
      fetch,
    });
    expect(state.queries).toEqual([]);
  });

  it("a failing first rail still dehydrates the pinned views", async () => {
    const { fetch } = fakeFetch({
      [pinnedPath]: {
        items: [view({ kind: "system", system_key: "continue_reading" })],
      },
      "/me/continue-reading": new ApiError(500),
    });
    const { state } = await prefetchPageRails(createServerQueryClient(), {
      pageId: PAGE,
      isSystem: false,
      fetch,
    });
    expect(hashes(state)).toEqual([
      hashKey(queryKeys.savedViews({ pinnedOn: PAGE })),
    ]);
  });

  it("gives up after the budget and leaves the slow query to the client", async () => {
    const never = vi.fn(() => new Promise<never>(() => {}));
    const started = Date.now();
    const { state } = await prefetchPageRails(createServerQueryClient(), {
      pageId: PAGE,
      isSystem: false,
      fetch: never as <T>(path: string) => Promise<T>,
      budgetMs: 30,
    });
    expect(Date.now() - started).toBeLessThan(1000);
    expect(state.queries).toEqual([]);
  });

  it("reports the rail it settled on: the first when it has cards, the next when a leading system rail is empty", async () => {
    const views = [
      view({ id: "s1", kind: "system", system_key: "continue_reading" }),
      view({ id: "f1", kind: "filter_series" }),
    ];
    const withCards = fakeFetch({
      [pinnedPath]: { items: views },
      "/me/continue-reading": { items: [{ issue: { id: "i" } }] },
    });
    const a = await prefetchPageRails(createServerQueryClient(), {
      pageId: PAGE,
      isSystem: false,
      fetch: withCards.fetch,
    });
    expect(a.priorityViewId).toBe("s1");

    const empty = fakeFetch({
      [pinnedPath]: { items: views },
      "/me/continue-reading": { items: [] },
      "/me/saved-views/f1/results": { items: [{ id: "s" }] },
    });
    const b = await prefetchPageRails(createServerQueryClient(), {
      pageId: PAGE,
      isSystem: false,
      fetch: empty.fetch,
    });
    expect(b.priorityViewId).toBe("f1");
    expect(b.snapshotId).not.toBe(a.snapshotId);
  });

  it("no settled rail when the first rail failed or is not prefetched (the page keeps its first-rail rule)", async () => {
    const { fetch } = fakeFetch({
      [pinnedPath]: {
        items: [view({ kind: "system", system_key: "on_this_day" })],
      },
    });
    const r = await prefetchPageRails(createServerQueryClient(), {
      pageId: PAGE,
      isSystem: false,
      fetch,
    });
    expect(r.priorityViewId).toBeNull();
  });

  it("at the budget it aborts what is in flight and starts nothing more", async () => {
    vi.useFakeTimers();
    const started: string[] = [];
    const signals: AbortSignal[] = [];
    let releasePinned: () => void = () => {};
    const fetch = <T>(path: string, init?: { signal?: AbortSignal }) => {
      started.push(path);
      if (init?.signal) signals.push(init.signal);
      if (path === pinnedPath)
        // Pinned views answer only after the budget.
        return new Promise<T>((resolve) => {
          releasePinned = () =>
            resolve({
              items: [view({ kind: "system", system_key: "continue_reading" })],
            } as T);
        });
      return new Promise<T>(() => {}); // libraries: never answers
    };
    const done = prefetchPageRails(createServerQueryClient(), {
      pageId: PAGE,
      isSystem: true,
      fetch,
      budgetMs: 50,
    });
    await vi.advanceTimersByTimeAsync(60);
    const r = await done;
    expect(r.state.queries).toEqual([]);
    expect(r.priorityViewId).toBeNull();
    expect(signals.length).toBeGreaterThan(0);
    expect(signals.every((s) => s.aborted)).toBe(true);
    // The late pinned answer must not set off the rail request.
    releasePinned();
    await vi.advanceTimersByTimeAsync(10);
    expect(started).not.toContain("/me/continue-reading");
    vi.useRealTimers();
  });

  it("each request gets its own client (no data shared between users)", () => {
    expect(createServerQueryClient()).not.toBe(createServerQueryClient());
  });
});
